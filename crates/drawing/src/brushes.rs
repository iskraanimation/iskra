//! Brush presets and the brush in hand (plan §7.12).
//!
//! A brush says which engine draws it: *raster*, `photocraft-paint`, which stamps a tip into
//! coverage; or *vector* (M11, plan §4.16), which keeps a stroke as its middle line and width.
//! Both are in the same presets, groups and files, each preset written with its kind.
//!
//! The engine's presets are made here and never written. What is the user's, their own presets
//! and the brush in hand, is [`Brushes`], kept in a file beside the preferences.

use std::path::Path;
use std::sync::OnceLock;

use photocraft_geom::Rect;
use photocraft_paint::{BrushSettings, StrokePoint, StrokeRenderer, TipShape, presets};

use crate::VectorBrush;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of the preset file that is written.
const VERSION: u32 = 1;

/// A brush, by the engine that draws it.
// Presets are few and long-lived: boxing the raster brush, much the larger, buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "brush", rename_all = "camelCase")]
pub enum Brush {
    /// Paints pixels: a tip stamped along the stroke.
    Raster(BrushSettings),
    /// Draws vector strokes on vector art.
    Vector(VectorBrush),
}

/// Which engine a brush is for: which kind of art layer it draws on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrushKind {
    Raster,
    Vector,
}

impl Brush {
    pub fn kind(&self) -> BrushKind {
        match self {
            Brush::Raster(_) => BrushKind::Raster,
            Brush::Vector(_) => BrushKind::Vector,
        }
    }
}

/// A brush under a name, in a group of the Brushes panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushPreset {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    #[serde(flatten)]
    pub brush: Brush,
}

/// The raster brush in hand, and the preset it was taken from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldRaster {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
    pub brush: BrushSettings,
}

impl Default for HeldRaster {
    /// A hard round pen whose width follows the pressure.
    fn default() -> Self {
        Self { name: "Pen".to_string(), group: String::new(), brush: BrushSettings { size: 8.0, ..BrushSettings::default() } }
    }
}

/// The vector brush in hand, and the preset it was taken from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeldVector {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub group: String,
    pub brush: VectorBrush,
}

impl Default for HeldVector {
    fn default() -> Self {
        Self { name: "Ink Pen".to_string(), group: String::new(), brush: VectorBrush::default() }
    }
}

/// The brush in hand, one for each kind: going from one kind of art layer to another and back
/// returns to the brush that was in hand there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InHand {
    pub raster: HeldRaster,
    pub vector: HeldVector,
}

impl InHand {
    /// The name of the preset the brush in hand of `kind` was taken from, and its group.
    pub fn named(&self, kind: BrushKind) -> (&str, &str) {
        match kind {
            BrushKind::Raster => (&self.raster.name, &self.raster.group),
            BrushKind::Vector => (&self.vector.name, &self.vector.group),
        }
    }

    fn named_mut(&mut self, kind: BrushKind) -> (&mut String, &mut String) {
        match kind {
            BrushKind::Raster => (&mut self.raster.name, &mut self.raster.group),
            BrushKind::Vector => (&mut self.vector.name, &mut self.vector.group),
        }
    }

    /// The size of the brush in hand of `kind`, and its smoothing (0..1): what the tool's bar
    /// changes, whichever kind it is.
    pub fn size_and_smoothing(&mut self, kind: BrushKind) -> (&mut f32, &mut f32) {
        match kind {
            BrushKind::Raster => (&mut self.raster.brush.size, &mut self.raster.brush.smoothing.amount),
            BrushKind::Vector => (&mut self.vector.brush.size, &mut self.vector.brush.smoothing),
        }
    }
}

/// The user's brushes: the brush in hand and the presets of their own.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Brushes {
    pub in_hand: InHand,
    pub presets: Vec<BrushPreset>,
    /// The engine's presets that the user deleted, by name.
    pub hidden: Vec<String>,
    /// The presets that have a slot, by the slot's place: the group each is listed in and its
    /// name. A slot is what a command, and so a shortcut, takes a preset by ([`SLOTS`]).
    pub slots: Vec<Option<(String, String)>>,
    /// Presets of the file that this version can't read, of a kind that came later. They are
    /// written back as they were.
    unread: Vec<Value>,
}

/// The file as it is written.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct File {
    version: u32,
    #[serde(skip_serializing_if = "Value::is_null")]
    in_hand: Value,
    presets: Vec<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hidden: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    slots: Vec<Option<(String, String)>>,
}

impl Brushes {
    /// The brushes that `text`, the content of a preset file, holds. What can't be read of it
    /// has its default.
    pub fn from_json(text: &str) -> Self {
        let file: File = serde_json::from_str(text).unwrap_or_default();
        let mut slots = file.slots;
        slots.truncate(SLOTS);
        let mut brushes = Self { in_hand: serde_json::from_value(file.in_hand).unwrap_or_default(), hidden: file.hidden, slots, ..Self::default() };
        for value in file.presets {
            match serde_json::from_value(value.clone()) {
                Ok(preset) => brushes.presets.push(preset),
                Err(_) => brushes.unread.push(value),
            }
        }
        brushes
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        let mut presets = self.presets.iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>()?;
        presets.extend(self.unread.iter().cloned());
        let file = File { version: VERSION, in_hand: serde_json::to_value(&self.in_hand)?, presets, hidden: self.hidden.clone(), slots: self.slots.clone() };
        serde_json::to_string_pretty(&file)
    }

    /// How many presets of the file were of a kind this version can't read.
    pub fn unread(&self) -> usize {
        self.unread.len()
    }

    /// Add presets that came from a file, each to its group. One whose name is taken there gets
    /// a number after it.
    pub fn import(&mut self, presets: Vec<BrushPreset>) {
        for preset in presets {
            let name = self.free_name(preset.listed_group(), &preset.name);
            self.presets.push(BrushPreset { name, ..preset });
        }
    }

    /// The brushes in the file at `path`, or the defaults if it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|text| Self::from_json(&text)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(path, self.to_json()?)
    }

    /// Take `preset` as the brush in hand of its kind. It brings its size; the smoothing is the
    /// tool's and stays.
    pub fn take(&mut self, preset: &BrushPreset) {
        match &preset.brush {
            Brush::Raster(brush) => {
                let mut brush = brush.clone().picked_over(&self.in_hand.raster.brush);
                brush.erase = false;
                self.in_hand.raster = HeldRaster { name: preset.name.clone(), group: preset.group.clone(), brush };
            }
            Brush::Vector(brush) => {
                let brush = VectorBrush { smoothing: self.in_hand.vector.brush.smoothing, ..brush.clone() };
                self.in_hand.vector = HeldVector { name: preset.name.clone(), group: preset.group.clone(), brush };
            }
        }
    }
}

/// How many slots there are for presets to be taken by a command, each with a shortcut of
/// its own in the keymap.
pub const SLOTS: usize = 5;

/// The group of the user's presets that name none.
pub const OWN_GROUP: &str = "My Brushes";

impl BrushPreset {
    /// The group the preset is listed in.
    pub fn listed_group(&self) -> &str {
        if self.group.is_empty() { OWN_GROUP } else { &self.group }
    }

    fn is(&self, group: &str, name: &str) -> bool {
        self.listed_group() == group && self.name == name
    }
}

/// Looking after presets. One is named by the group it is listed in and its name, which no two
/// presets share.
impl Brushes {
    /// The presets as the panel lists them: the engine's that were not deleted, then the user's.
    pub fn listed(&self) -> impl Iterator<Item = &BrushPreset> {
        builtin().iter().filter(|preset| !self.hidden.contains(&preset.name)).chain(&self.presets)
    }

    /// The presets of one kind, as they are listed.
    pub fn listed_of(&self, kind: BrushKind) -> impl Iterator<Item = &BrushPreset> {
        self.listed().filter(move |preset| preset.brush.kind() == kind)
    }

    /// Whether the preset is one of the engine's: those can be copied and deleted, not changed.
    pub fn is_builtin(&self, group: &str, name: &str) -> bool {
        !self.presets.iter().any(|preset| preset.is(group, name)) && builtin().iter().any(|preset| preset.is(group, name))
    }

    /// `name`, or `name` with the first number after it that makes it free in `group`.
    fn free_name(&self, group: &str, name: &str) -> String {
        let taken = |name: &str| self.listed().any(|preset| preset.is(group, name));
        if !taken(name) {
            return name.to_string();
        }
        (2..).map(|number| format!("{name} {number}")).find(|numbered| !taken(numbered)).unwrap_or_default()
    }

    /// Keep the brush in hand of `kind` as a preset of the user's, which it then is. A preset
    /// of the user's with that name in that group is replaced: that is how one is changed.
    /// Returns the name it got, which has a number after it if one of the engine's has the name.
    pub fn save_in_hand(&mut self, name: &str, group: &str, kind: BrushKind) -> Option<String> {
        let (name, group) = (name.trim(), if group.trim().is_empty() { OWN_GROUP } else { group.trim() });
        if name.is_empty() {
            return None;
        }
        let brush = match kind {
            BrushKind::Raster => Brush::Raster(BrushSettings { erase: false, ..self.in_hand.raster.brush.clone() }),
            BrushKind::Vector => Brush::Vector(self.in_hand.vector.brush.clone()),
        };
        let name = match self.presets.iter_mut().find(|preset| preset.is(group, name)) {
            Some(own) => {
                own.brush = brush;
                name.to_string()
            }
            None => {
                let name = self.free_name(group, name);
                self.presets.push(BrushPreset { name: name.clone(), group: group.to_string(), brush });
                name
            }
        };
        let (held_name, held_group) = self.in_hand.named_mut(kind);
        (*held_name, *held_group) = (name.clone(), group.to_string());
        Some(name)
    }

    /// Give a preset of the user's another name. Returns the name it got.
    pub fn rename(&mut self, group: &str, name: &str, to: &str) -> Option<String> {
        let to = to.trim();
        if to.is_empty() || to == name {
            return None;
        }
        let to = self.free_name(group, to);
        let preset = self.presets.iter_mut().find(|preset| preset.is(group, name))?;
        preset.name.clone_from(&to);
        self.follow(group, name, group, &to);
        Some(to)
    }

    /// Move a preset of the user's to another group, which is made if it is new.
    pub fn move_to(&mut self, group: &str, name: &str, to: &str) -> Option<String> {
        let to = to.trim();
        if to.is_empty() || to == group {
            return None;
        }
        let renamed = self.free_name(to, name);
        let preset = self.presets.iter_mut().find(|preset| preset.is(group, name))?;
        (preset.group, preset.name) = (to.to_string(), renamed.clone());
        self.follow(group, name, to, &renamed);
        Some(renamed)
    }

    /// Copy a preset, the engine's or the user's. The copy is the user's, in the same group,
    /// after the one it copies if that is the user's too. Returns its name.
    pub fn duplicate(&mut self, group: &str, name: &str) -> Option<String> {
        let copy = self.listed().find(|preset| preset.is(group, name))?.clone();
        let copy = BrushPreset { name: self.free_name(group, &format!("{name} copy")), group: group.to_string(), ..copy };
        let at = self.presets.iter().position(|preset| preset.is(group, name)).map_or(self.presets.len(), |at| at + 1);
        self.presets.insert(at, copy.clone());
        Some(copy.name)
    }

    /// Delete a preset. One of the engine's is only taken out of the list, and comes back with
    /// [`Brushes::restore_builtin`].
    pub fn delete(&mut self, group: &str, name: &str) -> bool {
        if let Some(at) = self.presets.iter().position(|preset| preset.is(group, name)) {
            self.presets.remove(at);
            return true;
        }
        let engines = builtin().iter().any(|preset| preset.is(group, name)) && !self.hidden.iter().any(|hidden| hidden == name);
        if engines {
            self.hidden.push(name.to_string());
        }
        engines
    }

    /// Put the presets back as `before` has them: the user's own, which of the engine's are
    /// deleted, and which preset the brush in hand says it is of. The brush in hand itself
    /// stays as it is. This is how a change to the presets is undone.
    pub fn restore(&mut self, before: &Brushes) {
        self.presets.clone_from(&before.presets);
        self.hidden.clone_from(&before.hidden);
        self.slots.clone_from(&before.slots);
        (self.in_hand.raster.name, self.in_hand.raster.group) = (before.in_hand.raster.name.clone(), before.in_hand.raster.group.clone());
        (self.in_hand.vector.name, self.in_hand.vector.group) = (before.in_hand.vector.name.clone(), before.in_hand.vector.group.clone());
    }

    /// How much the user's presets hold in sampled tips and patterns, in bytes, roughly: what
    /// a copy of them costs to keep.
    pub fn weight(&self) -> usize {
        let of = |preset: &BrushPreset| {
            let Brush::Raster(brush) = &preset.brush else { return 0 };
            let tip = |tip: &photocraft_paint::TipShape| match tip {
                photocraft_paint::TipShape::Sampled(tile) => tile.data.len() * 2,
                _ => 0,
            };
            let pattern = match &brush.texture.pattern {
                photocraft_paint::Pattern::Tile(tile) => tile.data.len() * 2,
                _ => 0,
            };
            tip(&brush.tip) + tip(&brush.dual_brush.tip) + pattern
        };
        self.presets.iter().map(of).sum()
    }

    /// The groups of the user's presets, in the order they are listed in.
    pub fn own_groups(&self) -> Vec<&str> {
        let mut groups: Vec<&str> = Vec::new();
        for preset in &self.presets {
            if !groups.contains(&preset.listed_group()) {
                groups.push(preset.listed_group());
            }
        }
        groups
    }

    /// Give a group another name: the user's presets in it go to the group of that name,
    /// which may be there already, each keeping its name unless it is taken there. The
    /// engine's presets stay in the group as it was called. Returns how many went.
    pub fn rename_group(&mut self, group: &str, to: &str) -> usize {
        let to = to.trim();
        if to.is_empty() || to == group {
            return 0;
        }
        let names: Vec<String> = self.presets.iter().filter(|preset| preset.listed_group() == group).map(|preset| preset.name.clone()).collect();
        names.iter().filter(|name| self.move_to(group, name, to).is_some()).count()
    }

    /// Delete a group as a whole: the user's presets in it, and the engine's, which come back
    /// with [`Brushes::restore_builtin`]. Returns how many went.
    pub fn delete_group(&mut self, group: &str) -> usize {
        let names: Vec<String> = self.listed().filter(|preset| preset.listed_group() == group).map(|preset| preset.name.clone()).collect();
        names.iter().filter(|name| self.delete(group, name)).count()
    }

    /// Put a preset of the user's where another of the user's is, before it, in that one's
    /// group; or with `before` naming a group only, at the end of that group. Returns the name
    /// it has there, which has a number after it if the group had one of that name.
    pub fn place(&mut self, group: &str, name: &str, to_group: &str, before: Option<&str>) -> Option<String> {
        if before == Some(name) && to_group == group {
            return None;
        }
        let from = self.presets.iter().position(|preset| preset.is(group, name))?;
        let mut moved = self.presets.remove(from);
        if to_group != group {
            moved.name = self.free_name(to_group, name);
            moved.group = to_group.to_string();
        }
        let named = moved.name.clone();
        // Before the one that is named; else after the last of the group; else at the end.
        let at = before.and_then(|before| self.presets.iter().position(|preset| preset.is(to_group, before)));
        let last = self.presets.iter().rposition(|preset| preset.listed_group() == to_group).map(|last| last + 1);
        self.presets.insert(at.or(last).unwrap_or(self.presets.len()), moved);
        self.follow(group, name, to_group, &named);
        Some(named)
    }

    /// Put a group of the user's presets before another in the list, or with no other at the
    /// end. The groups of the engine's presets come first and are not moved.
    pub fn place_group(&mut self, group: &str, before: Option<&str>) {
        if before == Some(group) {
            return;
        }
        let (moved, mut rest): (Vec<BrushPreset>, Vec<BrushPreset>) =
            std::mem::take(&mut self.presets).into_iter().partition(|preset| preset.listed_group() == group);
        let at = before.and_then(|before| rest.iter().position(|preset| preset.listed_group() == before)).unwrap_or(rest.len());
        rest.splice(at..at, moved);
        self.presets = rest;
    }

    /// Bring back the engine's presets that were deleted.
    pub fn restore_builtin(&mut self) {
        self.hidden.clear();
    }

    /// The slot a preset has, counted from 0, if it has one.
    pub fn slot_of(&self, group: &str, name: &str) -> Option<usize> {
        self.slots.iter().position(|slot| slot.as_ref().is_some_and(|(of, named)| of == group && named == name))
    }

    /// The preset in a slot, if the slot has one that is still listed.
    pub fn in_slot(&self, slot: usize) -> Option<&BrushPreset> {
        let (group, name) = self.slots.get(slot)?.as_ref()?;
        self.listed().find(|preset| preset.is(group, name))
    }

    /// Give a preset a slot, taking it from whichever had it, or with `None` take its slot
    /// away. A slot past [`SLOTS`] is none.
    pub fn set_slot(&mut self, group: &str, name: &str, slot: Option<usize>) {
        for held in &mut self.slots {
            if held.as_ref().is_some_and(|(of, named)| of == group && named == name) {
                *held = None;
            }
        }
        if let Some(slot) = slot.filter(|slot| *slot < SLOTS) {
            if self.slots.len() <= slot {
                self.slots.resize(slot + 1, None);
            }
            self.slots[slot] = Some((group.to_string(), name.to_string()));
        }
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
    }

    /// The preset of `kind` that is listed `by` places after the one the brush in hand of that
    /// kind is of, round from the last to the first: the next with 1, the one before with -1.
    /// The first, if the brush in hand is of none that is listed.
    pub fn stepped(&self, by: i64, kind: BrushKind) -> Option<&BrushPreset> {
        let listed: Vec<&BrushPreset> = self.listed_of(kind).collect();
        let (name, group) = self.in_hand.named(kind);
        let at = listed.iter().position(|preset| preset.name == name && (preset.group == group || preset.listed_group() == group));
        let to = match at {
            Some(at) => (at as i64 + by).rem_euclid(listed.len().max(1) as i64) as usize,
            None => 0,
        };
        listed.get(to).copied()
    }

    /// The brush in hand goes on saying which preset it is of when that one is renamed or
    /// moved, and so does the slot it has.
    fn follow(&mut self, group: &str, name: &str, to_group: &str, to_name: &str) {
        for held in self.slots.iter_mut().flatten() {
            if held.0 == group && held.1 == name {
                *held = (to_group.to_string(), to_name.to_string());
            }
        }
        for kind in [BrushKind::Raster, BrushKind::Vector] {
            let (held_name, held_group) = self.in_hand.named_mut(kind);
            if held_name == name && (held_group == group || (held_group.is_empty() && group == OWN_GROUP)) {
                (*held_group, *held_name) = (to_group.to_string(), to_name.to_string());
            }
        }
    }
}

/// Presets as a file of their own, to be read on another machine: the form of the user's file,
/// without a brush in hand.
pub fn presets_to_json(presets: &[BrushPreset]) -> serde_json::Result<String> {
    let presets = presets.iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string_pretty(&File { version: VERSION, in_hand: Value::Null, presets, hidden: Vec::new(), slots: Vec::new() })
}

/// The engine's presets, in the order of its groups.
pub fn builtin() -> &'static [BrushPreset] {
    static PRESETS: OnceLock<Vec<BrushPreset>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        let raster = presets::builtin().into_iter().map(|preset| BrushPreset { name: preset.name, group: preset.group, brush: Brush::Raster(preset.brush) });
        raster.chain(vector_builtin()).collect()
    })
}

/// The group of the engine's vector brushes.
pub const VECTOR_GROUP: &str = "Vector";

/// The group of the engine's vector brushes made for surfaces.
pub const SURFACE_GROUP: &str = "Vector Surface";

/// The engine's vector brushes.
fn vector_builtin() -> Vec<BrushPreset> {
    let base = VectorBrush::default();
    let vector = |name: &str, brush: VectorBrush| BrushPreset { name: name.to_string(), group: VECTOR_GROUP.to_string(), brush: Brush::Vector(brush) };
    let surface = |name: &str, brush: VectorBrush| BrushPreset { name: name.to_string(), group: SURFACE_GROUP.to_string(), brush: Brush::Vector(brush) };
    vec![
        vector("Ink Pen", base.clone()),
        vector("Brush Pen", VectorBrush { size: 14.0, thinnest: 0.02, taper_start: 2.0, taper_end: 4.0, ..base.clone() }),
        vector("Fine Liner", VectorBrush { size: 3.0, pressure_size: false, ..base.clone() }),
        vector("Marker", VectorBrush { size: 24.0, pressure_size: false, ..base.clone() }),
        vector("Tapered Line", VectorBrush { size: 6.0, pressure_size: false, taper_start: 6.0, taper_end: 6.0, ..base.clone() }),
        vector("Calligraphy", VectorBrush { size: 18.0, thinnest: 0.5, angle: 45.0, roundness: 0.2, ..base.clone() }),
        // Faint where it is pressed lightly, building up where it is gone over (the user's of
        // 2026-11-04).
        vector("Soft Pencil", VectorBrush { size: 5.0, thinnest: 0.6, pressure_opacity: true, faintest: 0.08, fade_end: 3.0, ..base.clone() }),
        // Drawn anew every other frame, as a line traced again by hand shimmers.
        vector("Boiling Line", VectorBrush { size: 5.0, thinnest: 0.3, boil: 1.2, boil_every: 2, ..base.clone() }),
        // For the Brush set to leave surfaces, as paint does: areas built up and carved.
        surface("Round Brush", VectorBrush { size: 20.0, thinnest: 0.15, taper_start: 1.0, taper_end: 3.0, ..base.clone() }),
        surface("Flat Brush", VectorBrush { size: 30.0, thinnest: 0.4, angle: 30.0, roundness: 0.3, ..base.clone() }),
        surface("Ink Brush", VectorBrush { size: 12.0, thinnest: 0.0, taper_start: 3.0, taper_end: 6.0, ..base.clone() }),
        surface("Detail Brush", VectorBrush { size: 5.0, thinnest: 0.3, ..base.clone() }),
        surface("Block-In", VectorBrush { size: 60.0, pressure_size: false, smoothing: 0.3, ..base.clone() }),
        // Rough edges, as of ink bled into paper and of a crayon's grain.
        surface("Rough Ink", VectorBrush { size: 12.0, thinnest: 0.05, taper_start: 2.0, taper_end: 4.0, roughness: 0.35, bumps: 0.3, ..base.clone() }),
        surface("Crayon", VectorBrush { size: 14.0, thinnest: 0.5, smoothing: 0.2, roughness: 0.55, bumps: 0.12, ..base.clone() }),
        // Strands that break up as the paint runs out.
        surface("Dry Brush", VectorBrush { size: 30.0, thinnest: 0.3, roughness: 0.15, bumps: 0.2, bristles: 8, dryness: 0.6, ..base.clone() }),
        // Holes, more with a lighter touch.
        surface(
            "Chalk",
            VectorBrush { size: 20.0, thinnest: 0.4, angle: 30.0, roundness: 0.5, roughness: 0.3, bumps: 0.15, holes: 0.4, hole_size: 0.08, ..base.clone() },
        ),
        surface("Sponge", VectorBrush { size: 48.0, pressure_size: false, roughness: 0.4, bumps: 0.3, holes: 0.75, hole_size: 0.1, ..base.clone() }),
        // Faint with a light touch and at its ends, as a wash is: strokes build up.
        surface(
            "Wash",
            VectorBrush {
                size: 36.0,
                thinnest: 0.5,
                pressure_opacity: true,
                faintest: 0.05,
                fade_start: 1.0,
                fade_end: 3.0,
                roughness: 0.2,
                bumps: 0.6,
                ..base
            },
        ),
    ]
}

/// The largest side of a tip or a pattern that is made from a picture, in pixels: a larger
/// picture is made smaller to fit.
pub const MAX_TILE: u32 = 1024;

/// A picture as a brush's tip, or as the pattern of its texture: `width` by `height` pixels of
/// straight RGBA bytes, the top row first. As a tip, what is dark and solid in the picture is
/// where the tip paints, as ink on paper: white and what is see-through paint nothing. As a
/// pattern it is the picture's lightness, as a texture's grain is. A picture larger than
/// [`MAX_TILE`] a side is made smaller, keeping its shape. `None` if there is no picture.
pub fn tile_of_picture(width: u32, height: u32, rgba: &[u8], tip: bool) -> Option<photocraft_paint::GrayTile> {
    if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
        return None;
    }
    let shrink = (width.max(height) as f32 / MAX_TILE as f32).max(1.0);
    let (wide, high) = (((width as f32 / shrink).round() as u32).max(1), ((height as f32 / shrink).round() as u32).max(1));
    let value = |x: u32, y: u32| {
        let at = (y as usize * width as usize + x as usize) * 4;
        let [r, g, b, a] = [rgba[at], rgba[at + 1], rgba[at + 2], rgba[at + 3]].map(|byte| f32::from(byte) / 255.0);
        let light = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        // What is see-through is paper: white.
        if tip { (1.0 - light) * a } else { light * a + (1.0 - a) }
    };
    // Each pixel of the tile is the mean of the pixels of the picture it covers.
    Some(photocraft_paint::GrayTile::from_fn(wide, high, |x, y| {
        let (x0, y0) = ((x as f32 * shrink) as u32, (y as f32 * shrink) as u32);
        let (x1, y1) = ((((x + 1) as f32 * shrink).ceil() as u32).clamp(x0 + 1, width), (((y + 1) as f32 * shrink).ceil() as u32).clamp(y0 + 1, height));
        let (mut sum, mut count) = (0.0, 0.0);
        for py in y0.min(height - 1)..y1 {
            for px in x0.min(width - 1)..x1 {
                (sum, count) = (sum + value(px, py), count + 1.0);
            }
        }
        if count > 0.0 { sum / count } else { 0.0 }
    }))
}

/// How many points the stroke of a picture has.
const PICTURE_POINTS: u32 = 48;

/// A stroke drawn by `brush`, as it would look: a wave from left to right, pressed lightly at
/// its ends and fully in the middle. The picture is `width` by `height` pixels of coverage, 0 to
/// 255. A brush too large for it is drawn smaller.
pub fn stroke_picture(brush: &Brush, width: u32, height: u32) -> Vec<u8> {
    let brush = match brush {
        Brush::Raster(brush) => brush,
        Brush::Vector(brush) => return vector_picture(brush, width, height),
    };
    let mut brush = brush.clone();
    // A round tip's soft edge paints past its size.
    let across = |brush: &BrushSettings| match brush.tip {
        TipShape::Round => 2.0 * photocraft_paint::tip_reach(brush.size / 2.0, brush.hardness),
        _ => brush.size,
    };
    let scale = (height as f32 * 0.45 / across(&brush)).min(1.0);
    brush.size *= scale;
    brush.dual_brush.size *= scale;
    brush.smoothing.amount = 0.0;
    (brush.erase, brush.seed) = (false, 1);
    let margin = f64::from(across(&brush)) / 2.0 + 2.0;
    let points = picture_points(width, height, margin);
    let mut renderer = StrokeRenderer::new(&brush, None, 1.0);
    renderer.push(&points);
    renderer.finish();
    let opacity = brush.opacity.clamp(0.0, 1.0);
    let coverage = renderer.coverage_in(Rect::new(0, 0, width as i32, height as i32));
    coverage.into_iter().map(|c| (c * opacity * 255.0).round().clamp(0.0, 255.0) as u8).collect()
}

/// The points of the stroke a picture shows: a wave from left to right, pressed lightly at its
/// ends and fully in the middle, kept `margin` from the sides.
fn picture_points(width: u32, height: u32, margin: f64) -> Vec<StrokePoint> {
    let (w, h) = (f64::from(width), f64::from(height));
    (0..=PICTURE_POINTS)
        .map(|step| {
            let along = f64::from(step) / f64::from(PICTURE_POINTS);
            let (x, y) = (margin + (w - 2.0 * margin) * along, h / 2.0 - (std::f64::consts::TAU * along).sin() * h * 0.2);
            let pressure = (std::f64::consts::PI * along).sin() as f32;
            StrokePoint { time: along * 800.0, ..StrokePoint::new(x, y, pressure.max(0.02)) }
        })
        .collect()
}

/// [`stroke_picture`] of a vector brush.
fn vector_picture(brush: &VectorBrush, width: u32, height: u32) -> Vec<u8> {
    let brush = VectorBrush { size: brush.size.min(height as f32 * 0.45), smoothing: 0.0, simplify: 0.25, ..brush.clone() };
    let points = picture_points(width, height, f64::from(brush.size) / 2.0 + 2.0);
    let Some(stroke) = crate::vector_stroke(&points, &brush) else { return vec![0; width as usize * height as usize] };
    // A rough, dry or holed brush is shown as the surface it leaves, which alone has its edge,
    // its strands and its holes.
    let kind = match brush.roughness > 0.0 || brush.bristles >= 2 || brush.holes > 0.0 {
        true => {
            let tracks = if stroke.marks() { vec![stroke.clone()] } else { Vec::new() };
            crate::ShapeKind::Fill(crate::VectorFill { tracks, boil: stroke.boil, ..crate::VectorFill::plain(crate::surface(&stroke, &brush)) })
        }
        false => crate::ShapeKind::Stroke(stroke),
    };
    let art = crate::VectorArt::new(vec![crate::VectorShape { id: crate::ShapeId(1), swatch: crate::SwatchId(1), kind }]);
    let pixels = art.raster().pixels(Rect::new(0, 0, width as i32, height as i32));
    pixels.into_iter().map(|pixel| (u32::from(pixel.coverage) * 255 / 65535) as u8).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn preset(name: &str) -> &'static BrushPreset {
        builtin().iter().find(|preset| preset.name == name).expect(name)
    }

    #[test]
    fn the_file_says_each_presets_kind_and_keeps_what_it_cant_read() {
        let mut brushes = Brushes::default();
        brushes.presets.push(BrushPreset { group: "Mine".to_string(), ..preset("Soft Pencil").clone() });
        brushes.take(preset("Hard Round"));
        let written: Value = serde_json::from_str(&brushes.to_json().unwrap()).unwrap();
        assert_eq!((&written["version"], &written["presets"][0]["kind"], &written["presets"][0]["group"]), (&json!(1), &json!("raster"), &json!("Mine")));
        assert_eq!(written["inHand"]["raster"]["name"], "Hard Round");
        assert_eq!(Brushes::from_json(&brushes.to_json().unwrap()), brushes);

        // A preset of a kind that a later version writes stays in the file.
        let mut later = written.clone();
        later["presets"].as_array_mut().unwrap().push(json!({ "name": "Ribbon", "kind": "particle", "brush": { "count": 5 } }));
        let read = Brushes::from_json(&later.to_string());
        assert_eq!(read.presets, brushes.presets);
        let again: Value = serde_json::from_str(&read.to_json().unwrap()).unwrap();
        assert_eq!(again["presets"][1]["name"], "Ribbon");

        assert_eq!(Brushes::from_json("not json"), Brushes::default());
        assert_eq!(Brushes::from_json("{}"), Brushes::default());
    }

    #[test]
    fn vector_presets_are_taken_kept_and_drawn_apart_from_the_raster_ones() {
        let mut brushes = Brushes::default();
        brushes.in_hand.vector.brush.smoothing = 0.7;
        brushes.take(preset("Calligraphy"));
        let held = &brushes.in_hand.vector;
        assert_eq!((held.name.as_str(), held.brush.roundness, held.brush.smoothing), ("Calligraphy", 0.2, 0.7), "its nib, and the smoothing that was");
        assert_eq!(brushes.in_hand.raster, HeldRaster::default(), "the raster brush in hand stays");
        // Stepping goes through the presets of one kind.
        let vector: Vec<&str> = brushes.listed_of(BrushKind::Vector).map(|preset| preset.name.as_str()).collect();
        assert_eq!(vector.first(), Some(&"Ink Pen"));
        assert!(brushes.listed_of(BrushKind::Raster).all(|preset| matches!(preset.brush, Brush::Raster(_))));
        assert_eq!(
            brushes.stepped(1, BrushKind::Vector).map(|preset| preset.name.as_str()),
            vector.get(vector.iter().position(|n| *n == "Calligraphy").unwrap() + 1).copied().or(vector.first().copied())
        );
        // Saved as the user's, written with its kind, and read back.
        brushes.in_hand.vector.brush.taper_end = 3.0;
        assert_eq!(brushes.save_in_hand("Nib", "", BrushKind::Vector), Some("Nib".to_string()));
        let written: Value = serde_json::from_str(&brushes.to_json().unwrap()).unwrap();
        assert_eq!((&written["presets"][0]["kind"], &written["presets"][0]["brush"]["taperEnd"]), (&json!("vector"), &json!(3.0)));
        assert_eq!(Brushes::from_json(&brushes.to_json().unwrap()), brushes);
        // Its stroke is drawn by the vector engine.
        let picture = stroke_picture(&preset("Brush Pen").brush, 180, 36);
        assert!(picture.iter().filter(|c| **c > 128).count() > 200, "a stroke is drawn");
    }

    #[test]
    fn taking_a_preset_takes_its_size_and_leaves_the_smoothing() {
        let mut brushes = Brushes::default();
        brushes.in_hand.raster.brush.smoothing.amount = 0.6;
        brushes.take(preset("Soft Round"));
        let held = &brushes.in_hand.raster;
        assert_eq!((held.name.as_str(), held.group.as_str()), ("Soft Round", "General"));
        assert_eq!((held.brush.size, held.brush.hardness, held.brush.smoothing.amount), (45.0, 0.0, 0.6));
    }

    #[test]
    fn presets_are_saved_renamed_copied_moved_and_deleted_and_the_engines_come_back() {
        let mut brushes = Brushes::default();
        let names = |brushes: &Brushes, group: &str| brushes.listed().filter(|p| p.listed_group() == group).map(|p| p.name.clone()).collect::<Vec<_>>();
        brushes.in_hand.raster.brush.size = 12.0;
        assert_eq!(brushes.save_in_hand(" Inker ", "", BrushKind::Raster), Some("Inker".to_string()));
        assert_eq!(
            (names(&brushes, OWN_GROUP), brushes.in_hand.raster.name.as_str(), brushes.in_hand.raster.group.as_str()),
            (vec!["Inker".to_string()], "Inker", OWN_GROUP)
        );
        // Saved again under its name, it is changed, not doubled.
        brushes.in_hand.raster.brush.size = 14.0;
        brushes.save_in_hand("Inker", OWN_GROUP, BrushKind::Raster);
        assert!(matches!(&brushes.presets[..], [BrushPreset { brush: Brush::Raster(brush), .. }] if brush.size == 14.0));
        assert_eq!(brushes.save_in_hand("", OWN_GROUP, BrushKind::Raster), None);
        // One of the engine's can't be written over: the user's gets a number.
        assert_eq!(brushes.save_in_hand("Hard Round", "General", BrushKind::Raster), Some("Hard Round 2".to_string()));
        assert!(brushes.is_builtin("General", "Hard Round") && !brushes.is_builtin("General", "Hard Round 2"));

        assert_eq!(brushes.rename("General", "Hard Round 2", "Liner"), Some("Liner".to_string()));
        assert_eq!(brushes.in_hand.raster.name, "Liner", "the brush in hand follows its preset");
        assert_eq!(brushes.rename("General", "Hard Round", "Mine"), None, "the engine's keep their names");
        assert_eq!(brushes.move_to("General", "Liner", "Inks"), Some("Liner".to_string()));
        assert_eq!((names(&brushes, "Inks"), brushes.in_hand.raster.group.as_str()), (vec!["Liner".to_string()], "Inks"));
        assert_eq!(brushes.duplicate("Inks", "Liner"), Some("Liner copy".to_string()));
        assert_eq!(brushes.duplicate("Dry Media", "Soft Pencil"), Some("Soft Pencil copy".to_string()));
        assert_eq!(names(&brushes, "Inks"), ["Liner", "Liner copy"]);
        assert!(names(&brushes, "Dry Media").contains(&"Soft Pencil copy".to_string()));

        assert!(brushes.delete("Inks", "Liner copy") && !brushes.delete("Inks", "Liner copy"));
        let engines = names(&brushes, "General").len();
        assert!(brushes.delete("General", "Soft Round") && !brushes.delete("General", "Soft Round"));
        assert_eq!(names(&brushes, "General").len(), engines - 1);
        assert_eq!(Brushes::from_json(&brushes.to_json().unwrap()), brushes, "what was deleted of the engine's is kept in the file");
        brushes.restore_builtin();
        assert_eq!(names(&brushes, "General").len(), engines);
    }

    #[test]
    fn groups_are_renamed_and_deleted_whole_and_presets_and_groups_are_put_in_order() {
        let mut brushes = Brushes::default();
        let own = |brushes: &Brushes| brushes.presets.iter().map(|preset| format!("{}/{}", preset.listed_group(), preset.name)).collect::<Vec<_>>();
        for (name, group) in [("A", "Inks"), ("B", "Inks"), ("C", "Paints"), ("D", "Paints"), ("E", "")] {
            brushes.save_in_hand(name, group, BrushKind::Raster);
        }
        assert_eq!(brushes.own_groups(), ["Inks", "Paints", OWN_GROUP]);
        // A preset is put before another, in its own group or in that one's; or at a group's end.
        assert_eq!(brushes.place("Inks", "B", "Inks", Some("A")), Some("B".to_string()));
        assert_eq!(brushes.place("Paints", "D", "Inks", Some("A")), Some("D".to_string()));
        assert_eq!(brushes.place(OWN_GROUP, "E", "Paints", None), Some("E".to_string()));
        assert_eq!(own(&brushes), ["Inks/B", "Inks/D", "Inks/A", "Paints/C", "Paints/E"]);
        assert_eq!((brushes.in_hand.raster.group.as_str(), brushes.in_hand.raster.name.as_str()), ("Paints", "E"), "the brush in hand follows");
        assert_eq!(brushes.place("Inks", "A", "Inks", Some("A")), None);
        assert_eq!(brushes.place("General", "Hard Round", "Inks", None), None, "the engine's stay where they are");
        // A name that is taken where it goes gets a number.
        brushes.save_in_hand("C", "Inks", BrushKind::Raster);
        assert_eq!(brushes.place("Inks", "C", "Paints", Some("C")), Some("C 2".to_string()));
        assert_eq!(own(&brushes), ["Inks/B", "Inks/D", "Inks/A", "Paints/C 2", "Paints/C", "Paints/E"]);
        // A group is put before another, or at the end.
        brushes.place_group("Paints", Some("Inks"));
        assert_eq!(brushes.own_groups(), ["Paints", "Inks"]);
        brushes.place_group("Paints", None);
        assert_eq!(own(&brushes), ["Inks/B", "Inks/D", "Inks/A", "Paints/C 2", "Paints/C", "Paints/E"]);
        // Renamed, a group's presets are in the group of the new name, with what is there.
        assert_eq!((brushes.rename_group("Inks", "Paints"), brushes.rename_group("Inks", "")), (3, 0));
        assert_eq!((brushes.own_groups(), brushes.presets.len()), (vec!["Paints"], 6));
        assert_eq!(brushes.rename_group("General", "Mine"), 0, "the engine's keep their group");
        // Deleted whole: the user's are gone, the engine's taken out of the list until restored.
        let listed = brushes.listed().count();
        let general = brushes.listed().filter(|preset| preset.listed_group() == "General").count();
        assert_eq!((brushes.delete_group("Paints"), brushes.delete_group("General")), (6, general));
        assert_eq!((brushes.presets.len(), brushes.listed().count()), (0, listed - 6 - general));
        brushes.restore_builtin();
        assert_eq!(brushes.listed().count(), listed - 6);
    }

    #[test]
    fn a_preset_has_a_slot_to_be_taken_by_and_the_presets_are_stepped_through() {
        let mut brushes = Brushes::default();
        brushes.save_in_hand("Inker", "Inks", BrushKind::Raster);
        brushes.set_slot("Inks", "Inker", Some(1));
        brushes.set_slot("General", "Hard Round", Some(0));
        assert_eq!(
            (brushes.slot_of("Inks", "Inker"), brushes.slot_of("General", "Hard Round"), brushes.slot_of("General", "Soft Round")),
            (Some(1), Some(0), None)
        );
        assert_eq!(brushes.in_slot(1).map(|preset| preset.name.as_str()), Some("Inker"));
        // Renamed or moved, a preset keeps its slot; the file keeps the slots.
        brushes.rename("Inks", "Inker", "Liner");
        brushes.move_to("Inks", "Liner", "Pens");
        assert_eq!((brushes.slot_of("Pens", "Liner"), brushes.in_slot(1).map(|preset| preset.name.as_str())), (Some(1), Some("Liner")));
        assert_eq!(Brushes::from_json(&brushes.to_json().unwrap()), brushes);
        // A slot has one preset: given to another, the first has none. Past the last is none.
        brushes.set_slot("General", "Soft Round", Some(1));
        assert_eq!((brushes.slot_of("Pens", "Liner"), brushes.in_slot(1).map(|preset| preset.name.as_str())), (None, Some("Soft Round")));
        brushes.set_slot("Pens", "Liner", Some(SLOTS));
        assert_eq!(brushes.slot_of("Pens", "Liner"), None);
        // One that is deleted is in no slot; taken away, the list of slots is as short as it can be.
        brushes.delete("General", "Soft Round");
        assert!(brushes.in_slot(1).is_none() && brushes.in_slot(7).is_none());
        brushes.set_slot("General", "Soft Round", None);
        brushes.set_slot("General", "Hard Round", None);
        assert!(brushes.slots.is_empty());

        // Stepping goes through the list as it is shown, and round at its ends.
        let listed: Vec<String> = brushes.listed().map(|preset| preset.name.clone()).collect();
        let first = brushes.listed().next().cloned().unwrap();
        brushes.take(&first);
        assert_eq!(brushes.stepped(1, BrushKind::Raster).map(|preset| &preset.name), listed.get(1));
        assert_eq!(brushes.stepped(-1, BrushKind::Raster).map(|preset| &preset.name), listed.last());
        brushes.in_hand.raster.name = "None Such".to_string();
        assert_eq!(brushes.stepped(1, BrushKind::Raster).map(|preset| &preset.name), listed.first());
    }

    #[test]
    fn a_picture_is_made_a_tip_or_a_pattern() {
        // Black, white, half-solid black and see-through, side by side.
        let rgba = [[0, 0, 0, 255], [255, 255, 255, 255], [0, 0, 0, 128], [0, 0, 0, 0]].concat();
        let tip = tile_of_picture(4, 1, &rgba, true).unwrap();
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert_eq!((tip.width, tip.height), (4, 1));
        assert!(near(tip.get(0, 0), 1.0) && near(tip.get(1, 0), 0.0) && near(tip.get(2, 0), 0.5) && near(tip.get(3, 0), 0.0), "{:?}", tip.to_f32());
        // As a pattern it is how light the picture is, and what is see-through is white.
        let pattern = tile_of_picture(4, 1, &rgba, false).unwrap();
        assert!(near(pattern.get(0, 0), 0.0) && near(pattern.get(1, 0), 1.0) && near(pattern.get(3, 0), 1.0), "{:?}", pattern.to_f32());
        // A large picture is made smaller, keeping its shape; none at all is no tile.
        let large = vec![0u8; 4096 * 2048 * 4];
        let small = tile_of_picture(4096, 2048, &large, true).unwrap();
        assert_eq!((small.width, small.height), (MAX_TILE, MAX_TILE / 2));
        assert!(tile_of_picture(0, 4, &[], true).is_none() && tile_of_picture(2, 2, &[0; 8], true).is_none());
    }

    #[test]
    fn every_preset_has_a_picture_of_its_stroke_that_fits() {
        let (width, height) = (240, 48);
        for preset in builtin() {
            let picture = stroke_picture(&preset.brush, width, height);
            assert_eq!(picture.len(), (width * height) as usize);
            assert!(picture.iter().filter(|c| **c > 0).count() > 40, "{} leaves a mark", preset.name);
        }
        // A brush far larger than the picture is drawn smaller, so the wave is still seen: the
        // top row stays clear.
        let large = preset("Soft Round Large");
        assert!(matches!(&large.brush, Brush::Raster(brush) if brush.size > 200.0));
        let picture = stroke_picture(&large.brush, width, height);
        assert!(picture[..width as usize].iter().all(|c| *c == 0));
        // The stroke is thin where it starts and wide in the middle, for a brush that pressure sizes.
        let picture = stroke_picture(&preset("Hard Round Pressure Size").brush, width, height);
        let column = |x: u32| (0..height).filter(|y| picture[(y * width + x) as usize] > 127).count();
        assert!(column(width / 2) > column(12) + 4, "{} against {}", column(width / 2), column(12));
    }
}
