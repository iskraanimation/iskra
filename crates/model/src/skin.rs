//! Skins (plan §4.8): a named set of replacements, drawing for drawing, as a palette style is
//! colour for swatch. The day drawings are the drawings themselves; a skin gives a variant of
//! some of them, and the others show as they are. The exposure names the day drawing, so a
//! change of skin retimes nothing.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{DrawingId, Layer, Palette, PoolDrawing, Project, Shot};

/// A skin's permanent id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkinId(pub u64);

/// A skin of the project: *Night*, as a palette style of that name is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skin {
    pub id: SkinId,
    pub name: String,
}

/// A layer's drawing in a skin: what shows in place of the day drawing `of` where `skin` is
/// shown. It is a drawing of its own, under its own id, kept as the pool's are.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    pub skin: SkinId,
    pub of: DrawingId,
    pub drawing: PoolDrawing,
}

impl Palette {
    pub fn skin(&self, id: SkinId) -> Option<&Skin> {
        self.skins.iter().find(|skin| skin.id == id)
    }

    pub fn skin_named(&self, name: &str) -> Option<&Skin> {
        self.skins.iter().find(|skin| skin.name == name)
    }
}

impl Project {
    /// The skins of `from`, another project's palette, as this one's of the same names, made
    /// where this one has none so called: what a template's variants, or a shot's brought
    /// back, are put in.
    pub(crate) fn skins_like(&mut self, from: &Palette) -> HashMap<SkinId, SkinId> {
        let mut like = HashMap::new();
        for skin in &from.skins {
            let here = match self.palette.skin_named(&skin.name) {
                Some(here) => here.id,
                None => {
                    let id = SkinId(self.new_id());
                    Arc::make_mut(&mut self.palette).skins.push(Skin { id, name: skin.name.clone() });
                    id
                }
            };
            like.insert(skin.id, here);
        }
        like
    }

    /// The skins are the palette's, beside its styles.
    pub fn skins(&self) -> &[Skin] {
        &self.palette.skins
    }

    pub fn skin(&self, id: SkinId) -> Option<&Skin> {
        self.palette.skin(id)
    }

    pub fn skin_named(&self, name: &str) -> Option<&Skin> {
        self.palette.skin_named(name)
    }

    /// The project as its shots' skins show it: in each shot shown in a skin, every drawing
    /// that has a variant there holds the variant's pixels under its own id. What reads a
    /// frame's drawing by the exposure, as the window does to draw on it, then reads what is
    /// shown. The project itself where no shot shows a variant.
    pub fn as_skins_show(self: &Arc<Self>) -> Arc<Project> {
        let skinned: Vec<(usize, Shot)> = self.shots.iter().enumerate().filter_map(|(at, shot)| Some((at, shot.skinned()?))).collect();
        if skinned.is_empty() {
            return self.clone();
        }
        let mut shown = Project::clone(self);
        for (at, shot) in skinned {
            shown.shots[at] = Arc::new(shot);
        }
        Arc::new(shown)
    }
}

impl Layer {
    /// The variant of the day drawing `of` in `skin`, if the skin has one.
    pub fn variant(&self, skin: SkinId, of: DrawingId) -> Option<&Variant> {
        self.variants.iter().find(|variant| variant.skin == skin && variant.of == of)
    }

    /// The drawing that shows for the day drawing `of` where `skin` is shown: its variant, or
    /// else the day drawing itself.
    pub fn shown(&self, skin: Option<SkinId>, of: DrawingId) -> DrawingId {
        skin.and_then(|skin| self.variant(skin, of)).map_or(of, |variant| variant.drawing.id)
    }

    /// The day drawing that `id` shows in place of: of a variant, the drawing it is a variant
    /// of; of a day drawing, itself.
    pub fn day_of(&self, id: DrawingId) -> DrawingId {
        self.variants.iter().find(|variant| variant.drawing.id == id).map_or(id, |variant| variant.of)
    }

    /// The drawing exposed on `frame` as `skin` shows it: its variant there, or the day one.
    pub fn shown_at(&self, skin: Option<SkinId>, frame: u32) -> Option<&PoolDrawing> {
        let day = self.exposure.at(frame)?;
        self.drawing(self.shown(skin, day))
    }

    /// Give every drawing of the pool, and every variant, a new id from `new_id`, with the
    /// exposure and the variants following: for a copy of the layer, whose drawings must not
    /// share ids with the original's. The old ids with the new.
    pub fn renumber_drawings(&mut self, mut new_id: impl FnMut() -> u64) -> HashMap<DrawingId, DrawingId> {
        let mut renamed = HashMap::new();
        for entry in &mut self.drawings {
            let new = DrawingId(new_id());
            self.exposure.replace_drawing(entry.id, new);
            renamed.insert(entry.id, new);
            entry.id = new;
        }
        self.variants.retain(|variant| renamed.contains_key(&variant.of));
        for variant in &mut self.variants {
            let new = DrawingId(new_id());
            renamed.insert(variant.drawing.id, new);
            (variant.of, variant.drawing.id) = (renamed[&variant.of], new);
        }
        renamed
    }
}

impl Shot {
    /// The drawing `layer` shows for its day drawing `of`, in the skin the shot is shown in.
    pub fn shown(&self, layer: &Layer, of: DrawingId) -> DrawingId {
        layer.shown(self.skin, of)
    }

    /// The shot as its skin shows it (see [`Project::as_skins_show`]), with no variants left to
    /// show. None where it is shown in no skin, or the skin has no variant in it.
    pub fn skinned(&self) -> Option<Shot> {
        let skin = self.skin?;
        self.variants_in(skin).next()?;
        let mut shot = self.clone();
        for layer in &mut shot.layers {
            if !layer.variants.iter().any(|variant| variant.skin == skin) {
                continue;
            }
            let layer = Arc::make_mut(layer);
            for entry in &mut layer.drawings {
                if let Some(variant) = layer.variants.iter().find(|variant| variant.skin == skin && variant.of == entry.id) {
                    entry.drawing = variant.drawing.drawing.clone();
                }
            }
            layer.variants.clear();
        }
        Some(shot)
    }

    /// The shot's variants, and the skin it is shown in, in the skins `like` says in place of
    /// theirs; a variant of a skin it does not name is let go.
    pub(crate) fn reskin(&mut self, like: &HashMap<SkinId, SkinId>) {
        self.skin = self.skin.and_then(|skin| like.get(&skin).copied());
        for layer in &mut self.layers {
            if layer.variants.is_empty() {
                continue;
            }
            let layer = Arc::make_mut(layer);
            layer.variants.retain(|variant| like.contains_key(&variant.skin));
            for variant in &mut layer.variants {
                variant.skin = like[&variant.skin];
            }
        }
    }

    /// Every variant of the shot's layers in `skin`, with its layer.
    pub fn variants_in(&self, skin: SkinId) -> impl Iterator<Item = (&Arc<Layer>, &Variant)> {
        self.layers.iter().flat_map(move |layer| layer.variants.iter().filter(move |variant| variant.skin == skin).map(move |variant| (layer, variant)))
    }
}
