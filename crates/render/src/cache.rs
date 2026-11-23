//! The pictures of what went through effect nodes, kept by what went into them (plan §8.2):
//! a still background under a blur is blurred once, and found again on every frame after.
//!
//! What a picture is made of is said whole, not as a number reckoned from it: the effect's
//! kind and its parameters as they are on the frame, the view, and for every layer that goes
//! through the drawing itself, where it lies and how it is bent. Two frames that say the same
//! are the same picture.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use iskra_drawing::{Affine, Drawing, Rect};
use iskra_eval::Card;
use iskra_model::{Palette, Projection, Shot, Value, Warp, WorkingSpace};

use crate::{Image, Made, View, draw, effects, finish, seen_of};

type Px = [f32; 4];

/// Of how many palettes, each in a space, pictures are kept at once.
const COLOURS: usize = 4;

/// A drawing, or what else is shared, the same as another only if it is that very one.
/// Holding it keeps its address from being used again.
pub(crate) struct Same<T>(Arc<T>);

impl<T> PartialEq for Same<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// One thing that goes through an effect.
#[derive(PartialEq)]
pub(crate) enum Part {
    Layer { drawing: Same<Drawing>, bounds: Rect, to_frame: Projection, warp: Option<Warp> },
    Through(Key),
}

/// All that the picture of what goes through an effect is made of: two are equal only where
/// the pictures would be.
#[derive(PartialEq)]
pub struct Key {
    /// Which colours, in which space: the number the cache gave them.
    colours: u64,
    kind: String,
    /// The kind's parameters as they are on the frame, in the kind's order.
    values: Vec<Option<Value>>,
    size: (u32, u32),
    from_frame: Affine,
    inside: Vec<Part>,
    mattes: Vec<(String, Vec<Part>)>,
    /// The drawings of the layers a Particles node draws its particles as, all of them,
    /// whichever is exposed on the frame.
    sprites: Vec<(Same<Drawing>, u64)>,
    /// How the camera sees a Particles node's nearest and farthest depths, on the frame and on
    /// the first: what its rain, snow and wind are seen through.
    camera: Vec<Option<Affine>>,
    /// How a splash plays its drawings, which the splash layer's timing says.
    splash: Vec<Option<usize>>,
    /// Which sheet of a Particles node's rain or snow it is, of how many.
    plane: Option<(u32, u32)>,
    /// A burst's particles on the frame, where they are stepped to it: what they bounced off
    /// on every frame before goes into them too.
    stepped: Option<Same<Vec<crate::simulation::Live>>>,
}

/// What `cards` are made of on `frame`, drawn into `view`.
pub(crate) fn parts(colours: u64, palette: &Palette, space: WorkingSpace, shot: &Shot, frame: u32, view: &View, cards: &[Card]) -> Vec<Part> {
    let part = |card: &Card| match card {
        Card::Layer(placed) => seen_of(shot, frame, placed).map(|seen| Part::Layer {
            drawing: Same(seen.drawing.clone()),
            bounds: seen.bounds,
            to_frame: seen.to_frame,
            warp: seen.warp,
        }),
        through @ Card::Through { .. } => Key::of(colours, palette, space, shot, frame, view, through).map(Part::Through),
        Card::Flat { .. } => None,
    };
    // A flat card is the cards in it, in their order.
    cards.iter().flat_map(Card::leaves).filter_map(part).collect()
}

impl Key {
    /// Of `card` as `view` sees it on `frame`, its colours from `palette` in `space`. `None`
    /// for a card that is no effect's.
    pub(crate) fn of(colours: u64, palette: &Palette, space: WorkingSpace, shot: &Shot, frame: u32, view: &View, card: &Card) -> Option<Key> {
        let Card::Through { effect, inside, mattes, plane, .. } = card else { return None };
        let effect = shot.effect(*effect)?;
        // What goes through is drawn with the room the effect needs, so that is its view.
        let room = effects::reach(effect, frame, view.scale(), view.width.max(view.height) as usize);
        let wide = view.with_room(room);
        let parts = |cards: &[Card]| parts(colours, palette, space, shot, frame, &wide, cards);
        Some(Key {
            colours,
            kind: effect.kind.clone(),
            values: effect.described().map(|kind| kind.params.iter().map(|param| effect.value(param.id, frame)).collect()).unwrap_or_default(),
            size: (view.width, view.height),
            from_frame: view.from_frame,
            inside: parts(inside),
            mattes: mattes.iter().map(|(port, cards)| (port.clone(), parts(cards))).collect(),
            sprites: [iskra_model::SPRITE, iskra_model::SPLASH_SPRITE]
                .into_iter()
                .flat_map(|port| crate::sprite_layers(shot, effect, mattes, port))
                .flat_map(|layer| layer.drawings.iter().map(|entry| (Same(entry.drawing.clone()), layer.density.to_bits())))
                .collect(),
            camera: match effect.kind == iskra_model::PARTICLES {
                true => {
                    let near = effect.number(iskra_model::DEPTH, frame);
                    let far = near - effect.number("depthRange", frame).max(0.0) - 1.0;
                    [(near, frame), (far, frame), (near, 0), (far, 0)].map(|(depth, at)| shot.seen(depth, at)).to_vec()
                }
                false => Vec::new(),
            },
            splash: crate::splash_play(shot, effect, mattes),
            plane: *plane,
            stepped: crate::simulation::stepped(shot, palette, space, effect, crate::particles::held(effect, frame), view.frame).map(Same),
        })
    }
}

struct Entry {
    key: Key,
    picture: Arc<Made>,
    /// The count of the last time it was made or found.
    used: u64,
}

#[derive(Default)]
struct Kept {
    /// The colours and the spaces the pictures were made in, each with the number the keys
    /// say it by: the last few, since a film's shots may each be in a style of their own.
    colours: Vec<(u64, Palette, WorkingSpace)>,
    entries: Vec<Entry>,
    bytes: usize,
    clock: u64,
    made: u64,
    found: u64,
}

/// The pictures of what went through effect nodes, kept from one frame to the next and from
/// one edit to the next, for as long as what they are made of is the same. Several threads
/// can render with one cache; two that want the same picture at once both make it.
pub struct Cache {
    kept: Mutex<Kept>,
    /// How many bytes of pictures are kept at most, or `None` for the shared budget. The ones
    /// not used for longest go first.
    budget: Option<usize>,
}

/// The budget of every cache made with `default`: what the user sets for the app.
static SHARED: AtomicUsize = AtomicUsize::new(512 << 20);

/// Set the budget of every cache made with `default`, in bytes: those there are now keep to
/// it from the next picture they keep on.
pub fn set_shared_budget(bytes: usize) {
    SHARED.store(bytes, Ordering::Relaxed);
}

impl Default for Cache {
    /// A cache that keeps to the shared budget, 512 MB unless [`set_shared_budget`] says else.
    fn default() -> Self {
        Cache { kept: Mutex::default(), budget: None }
    }
}

impl Cache {
    /// A cache that keeps up to `budget` bytes of pictures.
    pub fn new(budget: usize) -> Cache {
        Cache { kept: Mutex::default(), budget: Some(budget) }
    }

    /// How many bytes of pictures it keeps at most.
    pub fn budget(&self) -> usize {
        self.budget.unwrap_or_else(|| SHARED.load(Ordering::Relaxed))
    }

    fn kept(&self) -> std::sync::MutexGuard<'_, Kept> {
        // A thread that fell over while it held the pictures left them whole.
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many pictures were made, and how many were found: for tests and for tuning.
    pub fn counts(&self) -> (u64, u64) {
        let kept = self.kept();
        (kept.made, kept.found)
    }

    /// The number that says these colours in this space in a key. The pictures of colours
    /// that were not asked for for longest are let go.
    pub(crate) fn colours(&self, palette: &Palette, space: WorkingSpace) -> u64 {
        let mut kept = self.kept();
        if let Some(at) = kept.colours.iter().position(|(_, colours, in_space)| colours == palette && *in_space == space) {
            let known = kept.colours.remove(at);
            kept.colours.push(known);
            return kept.colours[kept.colours.len() - 1].0;
        }
        kept.clock += 1;
        let number = kept.clock;
        kept.colours.push((number, palette.clone(), space));
        if kept.colours.len() > COLOURS {
            let (gone, ..) = kept.colours.remove(0);
            kept.entries.retain(|entry| entry.key.colours != gone);
            kept.bytes = kept.entries.iter().map(|entry| std::mem::size_of_val(entry.picture.pixels.as_slice())).sum();
        }
        number
    }

    pub(crate) fn find(&self, key: &Key) -> Option<Arc<Made>> {
        let mut kept = self.kept();
        kept.clock += 1;
        let now = kept.clock;
        let entry = kept.entries.iter_mut().find(|entry| entry.key == *key)?;
        entry.used = now;
        let picture = entry.picture.clone();
        kept.found += 1;
        Some(picture)
    }

    pub(crate) fn keep(&self, key: Key, picture: Arc<Made>) {
        let bytes = |picture: &Made| std::mem::size_of_val(picture.pixels.as_slice());
        let mut kept = self.kept();
        kept.made += 1;
        let size = bytes(&picture);
        // One that is larger than all that may be kept is not kept at all.
        let budget = self.budget();
        if size > budget || kept.entries.iter().any(|entry| entry.key == key) {
            return;
        }
        kept.clock += 1;
        let used = kept.clock;
        kept.entries.push(Entry { key, picture, used });
        kept.bytes += size;
        while kept.bytes > budget {
            let Some(oldest) = kept.entries.iter().enumerate().min_by_key(|(_, entry)| entry.used).map(|(at, _)| at) else { break };
            kept.bytes -= bytes(&kept.entries.swap_remove(oldest).picture);
        }
    }

    /// [`crate::composite`], with what went through effects found here where it was made before.
    pub fn composite(&self, shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View) -> Vec<Px> {
        let palette = &palette.styled(&shot.styles);
        let colours = self.colours(palette, space);
        let mut picture = vec![[0.0f32; 4]; view.width as usize * view.height as usize];
        let evaluated = iskra_eval::evaluate(shot, frame);
        draw(shot, palette, space, frame, view, &evaluated.cards, &mut picture, Some((self, colours)));
        picture
    }

    /// [`crate::render`], with what went through effects found here where it was made before.
    pub fn render(&self, shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View) -> Image {
        finish(view.width, view.height, &self.composite(shot, palette, space, frame, view), space)
    }

    /// [`crate::card`], with what went through effects found here where it was made before.
    pub fn card(&self, shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View, card: &Card) -> Image {
        finish(view.width, view.height, &self.through(shot, palette, space, frame, view, card), space)
    }

    /// What goes through the effect node of `card`, changed by it: the part of the view where
    /// it changes anything, with where that is. `None` for a card that is no effect node's.
    /// It is what the GPU compositor sends to the card of an effect it does not make there.
    pub fn through_part(&self, shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View, card: &Card) -> Option<Arc<Made>> {
        let palette = &palette.styled(&shot.styles);
        let colours = self.colours(palette, space);
        crate::through(shot, palette, space, frame, view, card, Some((self, colours)))
    }

    /// What a Particles node's `card` draws on `frame`, as `view` sees it, worked out for the
    /// card to lay as the reference would. `None` for a card of another kind of node.
    pub fn particles(&self, shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View, card: &Card) -> Option<crate::Particles> {
        let palette = &palette.styled(&shot.styles);
        let colours = self.colours(palette, space);
        crate::particles_of(shot, palette, space, frame, view, card, Some((self, colours)))
    }

    /// What the picture of what goes through the effect node of `card` is made of, for a
    /// compositor to keep a picture of its own by. `None` for a card that is no effect's.
    pub fn key(&self, shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View, card: &Card) -> Option<Key> {
        let palette = &palette.styled(&shot.styles);
        Key::of(self.colours(palette, space), palette, space, shot, frame, view, card)
    }

    /// [`Cache::card`], as the compositor has the picture before it is handed over at 8 bits:
    /// for each pixel, in rows from the top, its colour times its alpha, and its alpha, in
    /// `space`.
    pub fn through(&self, shot: &Shot, palette: &Palette, space: WorkingSpace, frame: u32, view: &View, card: &Card) -> Vec<Px> {
        let palette = &palette.styled(&shot.styles);
        let colours = self.colours(palette, space);
        let mut picture = vec![[0.0f32; 4]; view.width as usize * view.height as usize];
        draw(shot, palette, space, frame, view, std::slice::from_ref(card), &mut picture, Some((self, colours)));
        picture
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_cache_keeps_to_the_shared_budget_and_one_of_its_own_to_its_own() {
        // Only made larger here, and put back: other tests render beside this one.
        let (own, shared) = (Cache::new(1 << 20), Cache::default());
        assert_eq!((own.budget(), shared.budget()), (1 << 20, 512 << 20));
        set_shared_budget(1 << 30);
        assert_eq!((own.budget(), shared.budget(), Cache::default().budget()), (1 << 20, 1 << 30, 1 << 30));
        set_shared_budget(512 << 20);
    }
}
