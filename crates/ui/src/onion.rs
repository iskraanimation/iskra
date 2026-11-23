//! Onion skin, light table and shift-and-trace (plan §4.13): which other drawings to show
//! faintly around the one being drawn. They are viewport aids only and are never rendered.

use std::collections::{BTreeMap, BTreeSet};

use egui::Color32;
use iskra_drawing::ArtLayerKind;
use iskra_model::{DrawingId, Layer, LayerId, PegId, Shot, Warp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnionMode {
    /// The previous and next *different* drawings, skipping held frames: what inbetweening needs.
    Drawing,
    /// Every frame in the range: shows spacing, holds included.
    Frame,
}

/// A temporary move of the onion-skinned drawings, to trace over them somewhere else
/// (shift-and-trace). The drawings themselves don't change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shift {
    /// In scene units.
    pub offset: [f64; 2],
    /// In radians, about the scene origin, applied before the offset.
    pub rotation: f64,
}

impl Shift {
    /// Where a scene point of a shifted drawing shows.
    pub fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.rotation.sin_cos();
        [x * cos - y * sin + self.offset[0], x * sin + y * cos + self.offset[1]]
    }

    /// This shift and then `after`, as one: a drawing's own move, and then the move of all.
    pub fn then(&self, after: &Shift) -> Shift {
        Shift { offset: after.apply(self.offset), rotation: self.rotation + after.rotation }
    }
}

/// What the viewport shows besides the frame itself. These are the user's settings, not the
/// project's.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub onion: bool,
    /// The view follows the layer in hand: on each frame it is panned so that the pivot of
    /// the peg that carries the layer is in the middle.
    pub follow: bool,
    pub mode: OnionMode,
    /// How many drawings (or frames) to show before and after the current one.
    pub before: u32,
    pub after: u32,
    /// Looks shown whatever the frame, for example key 1 and key 9 while drawing breakdown 5.
    pub pinned: Vec<Pin>,
    /// Show only the line art of onion-skinned drawings.
    pub line_art_only: bool,
    /// Opacity of the nearest onion-skinned drawing, 0..1. Each step further away is fainter.
    pub opacity: f32,
    /// Show only the edges of what an onion-skinned drawing covers: a filled shape is its
    /// outline, so that what is drawn over it is seen.
    pub outline: bool,
    /// The move of all the onion-skinned drawings together.
    pub shift: Shift,
    /// The moves of single drawings, each made before the move of all: to trace two drawings
    /// that lie apart over each other.
    pub shifts: BTreeMap<DrawingId, Shift>,
    /// The drawing that shift-and-trace moves alone; without one it moves all together.
    pub shifting: Option<DrawingId>,
    /// Fade the other layers while drawing on one.
    pub light_table: bool,
    /// Of the drawing in hand, show this art layer alone: the lines without their fills, or
    /// the fills without the lines. `None` shows them all.
    pub shown_art: Option<ArtLayerKind>,
    /// Show it so of every drawing, and not only of the one in hand.
    pub shown_art_all: bool,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            onion: false,
            follow: false,
            mode: OnionMode::Drawing,
            before: 1,
            after: 1,
            pinned: Vec::new(),
            line_art_only: true,
            opacity: 0.5,
            outline: false,
            shift: Shift::default(),
            shifts: BTreeMap::new(),
            shifting: None,
            light_table: false,
            shown_art: None,
            shown_art_all: false,
        }
    }
}

impl Display {
    /// How an onion-skinned drawing is moved: by its own move, if it has one, and then by
    /// the move of all.
    pub fn shift_of(&self, drawing: DrawingId) -> Shift {
        self.shifts.get(&drawing).map_or(self.shift, |own| own.then(&self.shift))
    }

    /// The move that shift-and-trace changes now: of the one drawing that is chosen, or of all.
    pub fn shift_in_hand(&mut self) -> &mut Shift {
        match self.shifting {
            Some(drawing) => self.shifts.entry(drawing).or_default(),
            None => &mut self.shift,
        }
    }

    /// Whether anything is moved.
    pub fn is_shifted(&self) -> bool {
        self.shift != Shift::default() || self.shifts.values().any(|shift| *shift != Shift::default())
    }

    /// Put every onion-skinned drawing back where it is.
    pub fn reset_shifts(&mut self) {
        (self.shift, self.shifts) = (Shift::default(), BTreeMap::new());
    }
}

/// A look of a layer kept in the onion skin on every frame: its drawing, where the pegs and
/// bones had it on the frame it was pinned on. So a drawing held all through a shot while its
/// pegs move it can be pinned in one pose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pin {
    pub layer: LayerId,
    pub drawing: DrawingId,
    pub frame: u32,
}

/// The layers whose onion skin shows: the layer in hand, or the peg in hand, and with it what
/// is picked when it is among that; of a peg, every layer under it.
pub fn onion_layers(shot: &Shot, layer: Option<LayerId>, peg: Option<PegId>, picked: &BTreeSet<u64>) -> BTreeSet<LayerId> {
    let in_hand = peg.map(|peg| peg.0).or(layer.map(|layer| layer.0));
    let nodes: BTreeSet<u64> = match in_hand {
        Some(node) if picked.contains(&node) => picked.clone(),
        _ => in_hand.into_iter().collect(),
    };
    let under = |layer: &Layer| shot.ancestors(layer.parent).any(|peg| nodes.contains(&peg.id.0));
    shot.layers.iter().filter(|layer| nodes.contains(&layer.id.0) || under(layer)).map(|layer| layer.id).collect()
}

/// How wide an outline is on screen, in points, whatever the zoom.
const OUTLINE: f32 = 2.0;

/// The most a drawing's pixels an outline is wide, where the drawing is shown very small.
const WIDEST_OUTLINE: usize = 16;

/// How far in from its edge what a drawing covers is shown as its outline, in the drawing's
/// pixels, where one of them is `shown` points wide on screen: the outline is about as wide
/// on screen at every zoom, but never less than one of the drawing's pixels.
pub fn outline_reach(shown: f32) -> usize {
    if shown.is_nan() || shown <= 0.0 {
        return 1;
    }
    ((OUTLINE / shown).round() as usize).clamp(1, WIDEST_OUTLINE)
}

/// The edges of what is covered: `alpha` is the coverage of `width` by `height` pixels, in
/// rows, and what comes back is the coverage of the same pixels where one within `reach` of
/// them, across or down, is not covered, and nothing where all of those are. So a filled
/// shape is its outline, and a line no wider than twice the reach stays whole. Past the
/// picture's own edge nothing is taken to be uncovered: give it with `reach` more pixels all
/// round than are wanted.
pub fn edges(alpha: &[u8], width: usize, height: usize, reach: usize) -> Vec<u8> {
    let at = |x: usize, y: usize| alpha[y * width + x];
    let mut edge = vec![0u8; alpha.len()];
    for y in 0..height {
        for x in 0..width {
            let own = at(x, y);
            if own == 0 {
                continue;
            }
            // The least covered pixel within reach, in the row and in the column.
            let (x0, x1, y0, y1) = (x.saturating_sub(reach), (x + reach).min(width - 1), y.saturating_sub(reach), (y + reach).min(height - 1));
            let least = (x0..=x1).map(|x| at(x, y)).chain((y0..=y1).map(|y| at(x, y))).min().unwrap_or(own);
            edge[y * width + x] = (u16::from(own) * u16::from(255 - least) / 255) as u8;
        }
    }
    edge
}

/// How much fainter each step away from the current drawing is.
const FALLOFF: f32 = 0.6;

/// The opacity of the other layers under the light table.
pub const LIGHT_TABLE_OPACITY: f32 = 0.35;

/// One onion-skinned drawing: what the layer looked like on another frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ghost {
    pub drawing: DrawingId,
    /// The frame it is shown as on: the pegs above the layer place it as they are there.
    pub frame: u32,
    /// Steps from the current drawing: negative before it, positive after, 0 for a pinned one.
    pub distance: i32,
}

impl Ghost {
    /// Of `before` before, `after` after (red and green, unless the user set others), blue
    /// pinned; fainter the further away.
    pub fn color(&self, opacity: f32, [before, after]: [Color32; 2]) -> Color32 {
        let [r, g, b, _] = match self.distance {
            ..0 => before,
            0 => Color32::from_rgb(60, 110, 230),
            1.. => after,
        }
        .to_srgba_unmultiplied();
        let alpha = opacity.clamp(0.0, 1.0) * FALLOFF.powi((self.distance.abs() - 1).max(0));
        Color32::from_rgba_unmultiplied(r, g, b, (alpha * 255.0) as u8)
    }
}

/// How the pegs above a layer and the bones that bend it move it: the frames they have keys
/// on, and where they put the layer on a frame and how they bend it, as something to compare.
pub struct Motion<'a> {
    pub keys: BTreeSet<u32>,
    pub placed: &'a dyn Fn(u32) -> [f64; 6],
    pub bent: &'a dyn Fn(u32) -> Option<Warp>,
}

impl Motion<'_> {
    /// A layer that nothing moves.
    #[cfg(test)]
    pub const STILL: Motion<'static> = Motion { keys: BTreeSet::new(), placed: &|_| [0.0; 6], bent: &|_| None };

    /// The frames on which what moves `layer` in `shot` has keys: the pegs above it, and the
    /// bones or curve that bend it, of its own or through its peg, or that a peg above follows.
    pub fn keys_of(shot: &Shot, layer: &Layer) -> BTreeSet<u32> {
        let pegs = shot.ancestors(layer.parent);
        let followed = shot.ancestors(layer.parent).filter_map(|peg| Some(peg.follows?.deformer));
        let bending = layer.deformer.into_iter().chain(shot.bending(layer)).chain(followed);
        let bones = bending.filter_map(|id| shot.deformer(id)).flat_map(|deformer| deformer.key_frames());
        pegs.flat_map(|peg| peg.animation.key_frames()).chain(bones).collect()
    }
}

/// What to onion-skin around `frame` of `layer`, farthest first so the nearest is drawn on
/// top. A ghost is how the layer looked on another frame: the drawing exposed there, where the
/// pegs put it there. So a drawing that is held while a peg moves it is its own ghost. What
/// looks just as the layer does on `frame` is never among them, and no look is listed twice.
///
/// By drawing, the steps are the frames something starts on: a drawing, or a keyframe of a peg
/// above the layer. By frame, every frame in the range is a step.
pub fn ghosts(layer: &Layer, frame: u32, display: &Display, motion: &Motion) -> Vec<Ghost> {
    if !display.onion {
        return Vec::new();
    }
    let look = |at: u32| layer.exposure.at(at).map(|drawing| (drawing, (motion.placed)(at), (motion.bent)(at)));
    let current = look(frame);
    let mut found: Vec<Ghost> = Vec::new();
    let add = |found: &mut Vec<Ghost>, at: u32, distance: i32| {
        let Some((drawing, placed, bent)) = look(at) else { return false };
        let seen = |g: &Ghost| g.drawing == drawing && (motion.placed)(g.frame) == placed && (motion.bent)(g.frame) == bent;
        let fresh = current.as_ref().is_none_or(|current| *current != (drawing, placed, bent.clone())) && !found.iter().any(seen);
        if fresh {
            found.push(Ghost { drawing, frame: at, distance });
        }
        fresh
    };
    match display.mode {
        OnionMode::Drawing => {
            let mut starts: BTreeSet<u32> = layer.exposure.runs().iter().map(|run| run.start).collect();
            starts.extend(&motion.keys);
            // Nearest first on each side. A start that looks like the current frame, as the
            // start of the hold the frame is in does, is no step.
            let before = starts.range(..frame).rev().copied();
            let after = starts.range(frame.saturating_add(1)..).copied();
            for (frames, limit, sign) in [(Box::new(before) as Box<dyn Iterator<Item = u32>>, display.before, -1), (Box::new(after), display.after, 1)] {
                let mut step = 0;
                for at in frames {
                    if step == limit as i32 {
                        break;
                    }
                    if add(&mut found, at, sign * (step + 1)) {
                        step += 1;
                    }
                }
            }
        }
        OnionMode::Frame => {
            for step in 1..=display.before.min(frame) {
                add(&mut found, frame - step, -(step as i32));
            }
            for step in 1..=display.after {
                if let Some(at) = frame.checked_add(step) {
                    add(&mut found, at, step as i32);
                }
            }
        }
    }
    // A pinned look shows as it was on its frame, unless it is the current one or shown already.
    for pin in display.pinned.iter().filter(|pin| pin.layer == layer.id && layer.drawing(pin.drawing).is_some()) {
        let look = (pin.drawing, (motion.placed)(pin.frame), (motion.bent)(pin.frame));
        let seen = |g: &Ghost| g.drawing == pin.drawing && (motion.placed)(g.frame) == look.1 && (motion.bent)(g.frame) == look.2;
        if current.as_ref() != Some(&look) && !found.iter().any(seen) {
            found.push(Ghost { drawing: pin.drawing, frame: pin.frame, distance: 0 });
        }
    }
    found.sort_by_key(|g| std::cmp::Reverse(g.distance.abs()));
    found
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{PoolDrawing, Project};

    use super::*;

    /// A layer whose cells are `cells`, one letter per frame ('-' is empty): drawing 'a' has
    /// id 1, 'b' id 2 and so on.
    fn layer(cells: &str) -> Layer {
        let mut layer = Project::default().new_layer("e", iskra_model::Role::Animation);
        for (frame, cell) in cells.chars().enumerate().filter(|(_, c)| *c != '-') {
            let id = DrawingId(u64::from(cell) - u64::from('a') + 1);
            if layer.drawing(id).is_none() {
                layer.drawings.push(PoolDrawing { id, name: cell.to_string(), drawing: Arc::default() });
            }
            layer.exposure.set(frame as u32, 1, Some(id));
        }
        layer
    }

    /// The ghosts as "letter:distance", nearest last.
    fn list(cells: &str, frame: u32, display: &Display) -> String {
        let letter = |g: &Ghost| char::from(b'a' + g.drawing.0 as u8 - 1);
        ghosts(&layer(cells), frame, display, &Motion::STILL).iter().map(|g| format!("{}:{}", letter(g), g.distance)).collect::<Vec<_>>().join(" ")
    }

    fn on(mode: OnionMode, before: u32, after: u32) -> Display {
        Display { onion: true, mode, before, after, ..Default::default() }
    }

    #[test]
    fn an_outline_is_as_wide_on_screen_at_every_zoom() {
        assert_eq!(outline_reach(1.0), 2, "two of the drawing's pixels where each is a point");
        assert_eq!(outline_reach(0.25), 8, "eight where the drawing is shown at a quarter");
        assert_eq!(outline_reach(4.0), 1, "zoomed in, one pixel at least");
        assert_eq!(outline_reach(0.001), WIDEST_OUTLINE);
        assert_eq!(outline_reach(0.0), 1);
    }

    #[test]
    fn the_edges_of_what_is_covered_are_its_outline() {
        // A filled square of six in a field of ten, and a line one pixel wide beside it.
        let (width, height) = (12, 10);
        let covered = |x: usize, y: usize| ((2..8).contains(&x) && (2..8).contains(&y)) || x == 10;
        let alpha: Vec<u8> = (0..width * height).map(|at| if covered(at % width, at / width) { 255 } else { 0 }).collect();
        let edge = edges(&alpha, width, height, 1);
        let at = |x: usize, y: usize| edge[y * width + x];
        // The square's rim is kept and its inside is empty; outside it nothing is added.
        assert_eq!((at(2, 2), at(2, 5), at(7, 4), at(4, 7)), (255, 255, 255, 255));
        assert_eq!((at(3, 3), at(4, 5), at(6, 6), at(0, 0), at(8, 4)), (0, 0, 0, 0, 0));
        // A thin line is all edge: it stays as it is. With more reach the rim is wider.
        assert_eq!((at(10, 0), at(10, 5), at(10, 9)), (255, 255, 255));
        let wide = edges(&alpha, width, height, 2);
        assert_eq!((wide[3 * width + 3], wide[4 * width + 4]), (255, 0));
        // A soft edge keeps its softness: half covered beside nothing is half.
        assert_eq!(edges(&[0, 128, 255, 255, 255], 5, 1, 1), [0, 128, 127, 0, 0]);
    }

    #[test]
    fn a_drawing_is_moved_alone_and_then_with_all() {
        let (one, two) = (DrawingId(1), DrawingId(2));
        let mut display = Display::default();
        assert!(!display.is_shifted());
        // With no drawing chosen, shift-and-trace moves all together.
        display.shift_in_hand().offset = [10.0, 0.0];
        assert_eq!((display.shift_of(one), display.shift_of(two)), (display.shift, display.shift));
        // With one chosen it moves that one, before the move of all: the others stay.
        display.shifting = Some(one);
        display.shift_in_hand().offset = [0.0, 5.0];
        assert_eq!((display.shift_of(one).offset, display.shift_of(two).offset), ([10.0, 5.0], [10.0, 0.0]));
        // Turned as a whole, a drawing's own move turns with it.
        display.shift.rotation = std::f64::consts::FRAC_PI_2;
        let moved = display.shift_of(one);
        assert!((moved.offset[0] - 5.0).abs() < 1e-9 && moved.offset[1].abs() < 1e-9 && moved.rotation == display.shift.rotation, "{moved:?}");
        assert!(display.is_shifted());
        display.reset_shifts();
        assert!(!display.is_shifted() && display.shift_of(one) == Shift::default());
    }

    #[test]
    fn by_drawing_skips_holds_and_empty_cells() {
        // On the hold of c: the previous two drawings are b and a, however long they are held.
        assert_eq!(list("aaabb-ccc-dd", 7, &on(OnionMode::Drawing, 2, 1)), "a:-2 b:-1 d:1");
        assert_eq!(list("aaabb-ccc-dd", 7, &on(OnionMode::Drawing, 1, 0)), "b:-1");
        // In an empty cell the neighbours on both sides show.
        assert_eq!(list("aaabb-ccc-dd", 5, &on(OnionMode::Drawing, 1, 1)), "b:-1 c:1");
        assert_eq!(list("aaabb-ccc-dd", 0, &on(OnionMode::Drawing, 3, 0)), "", "nothing before the first drawing");
    }

    #[test]
    fn by_frame_counts_frames_and_lists_a_held_drawing_once() {
        assert_eq!(list("aaabb-ccc-dd", 7, &on(OnionMode::Frame, 4, 3)), "b:-3 d:3", "c is current, the empty cells show nothing");
        assert_eq!(list("abcde", 2, &on(OnionMode::Frame, 2, 2)), "a:-2 e:2 b:-1 d:1");
        assert_eq!(list("abcde", 1, &on(OnionMode::Frame, 5, 0)), "a:-1", "the range stops at the first frame");
    }

    #[test]
    fn a_drawing_exposed_twice_is_never_its_own_ghost_or_listed_twice() {
        assert_eq!(list("abab", 2, &on(OnionMode::Drawing, 3, 3)), "b:-1");
        assert_eq!(list("abcb", 2, &on(OnionMode::Drawing, 2, 2)), "a:-2 b:-1", "b is already shown as the previous drawing");
    }

    #[test]
    fn a_drawing_held_under_a_moving_peg_is_its_own_ghost() {
        // One drawing held for twelve frames, under a peg keyed on frames 0, 4 and 8 that moves
        // it until frame 8 and then rests.
        let placed = |frame: u32| [f64::from(frame.min(8)), 0.0, 0.0, 0.0, 0.0, 0.0];
        let motion = Motion { keys: BTreeSet::from([0, 4, 8]), placed: &placed, bent: &|_| None };
        let list = |frame: u32, display: &Display| {
            let found = ghosts(&layer("aaaaaaaaaaaa"), frame, display, &motion);
            found.iter().map(|g| format!("{}:{}", g.frame, g.distance)).collect::<Vec<_>>().join(" ")
        };
        // By frame: the frames around, each where the peg had the drawing then.
        assert_eq!(list(5, &on(OnionMode::Frame, 2, 1)), "3:-2 4:-1 6:1");
        // By drawing: the peg's keyframes are the steps, also from between two of them.
        assert_eq!(list(5, &on(OnionMode::Drawing, 2, 1)), "0:-2 4:-1 8:1");
        assert_eq!(list(4, &on(OnionMode::Drawing, 1, 1)), "0:-1 8:1", "on a keyframe, that pose is the current one");
        // Where the peg rests, the frames look alike and none is a ghost of another.
        assert_eq!(list(10, &on(OnionMode::Frame, 1, 1)), "");
        assert_eq!(list(10, &on(OnionMode::Frame, 3, 1)), "7:-3");
        assert_eq!(ghosts(&layer("aaaa"), 2, &on(OnionMode::Frame, 2, 2), &Motion::STILL), [], "and a still layer has none, as before");
    }

    #[test]
    fn pinned_looks_show_on_every_frame_but_their_own() {
        let layer = layer("abcde");
        let pin = |drawing, frame| Pin { layer: layer.id, drawing: DrawingId(drawing), frame };
        let elsewhere = Pin { layer: LayerId(layer.id.0 + 1), ..pin(2, 1) };
        let pinned = Display { pinned: vec![pin(1, 0), pin(99, 0), elsewhere], ..on(OnionMode::Drawing, 0, 0) };
        let listed =
            |frame, display: &Display| ghosts(&layer, frame, display, &Motion::STILL).iter().map(|g| (g.drawing.0, g.frame, g.distance)).collect::<Vec<_>>();
        assert_eq!(listed(3, &pinned), [(1, 0, 0)], "a, and not what is not of this layer");
        assert_eq!(listed(0, &pinned), [], "not over itself");
        assert_eq!(listed(3, &Display { onion: false, ..pinned }), [], "off is off");
    }

    #[test]
    fn a_drawing_held_under_a_moving_peg_is_pinned_in_one_pose() {
        // As above: held for twelve frames, the peg moving it until frame 8.
        let layer = layer("aaaaaaaaaaaa");
        let placed = |frame: u32| [f64::from(frame.min(8)), 0.0, 0.0, 0.0, 0.0, 0.0];
        let motion = Motion { keys: BTreeSet::from([0, 4, 8]), placed: &placed, bent: &|_| None };
        let display = Display { pinned: vec![Pin { layer: layer.id, drawing: DrawingId(1), frame: 2 }], ..on(OnionMode::Drawing, 0, 0) };
        let frames = |frame| ghosts(&layer, frame, &display, &motion).iter().map(|g| (g.frame, g.distance)).collect::<Vec<_>>();
        assert_eq!(frames(6), [(2, 0)], "the pose of frame 2, though the drawing is the current one");
        assert_eq!(frames(10), [(2, 0)]);
        assert_eq!(frames(2), [], "not over itself");
        // Shown as a step already, it is not shown twice.
        let near = Display { before: 1, pinned: vec![Pin { frame: 4, ..display.pinned[0] }], ..display.clone() };
        assert_eq!(ghosts(&layer, 6, &near, &motion).iter().map(|g| (g.frame, g.distance)).collect::<Vec<_>>(), [(4, -1)]);
    }

    #[test]
    fn the_onion_skin_is_of_what_is_picked_and_what_a_peg_carries() {
        let mut project = Project::default();
        let mut shot = project.new_shot("s", 10);
        let peg = |id: u64, parent: Option<u64>| {
            Arc::new(iskra_model::Peg {
                id: PegId(id),
                name: String::new(),
                parent: parent.map(PegId),
                transform: Default::default(),
                animation: Default::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        };
        // Pegs 900 over 901; layers under each, and one under none.
        shot.pegs = vec![peg(900, None), peg(901, Some(900))];
        let mut layers = Vec::new();
        for parent in [Some(900), Some(901), None] {
            let mut layer = project.new_layer("l", iskra_model::Role::Animation);
            layer.parent = parent.map(PegId);
            layers.push(layer.id);
            shot.layers.push(Arc::new(layer));
        }
        let [top, low, free] = [layers[0], layers[1], layers[2]];
        let set = |ids: &[LayerId]| ids.iter().copied().collect::<BTreeSet<_>>();
        let none = BTreeSet::new();
        assert_eq!(onion_layers(&shot, Some(low), None, &none), set(&[low]), "the layer in hand alone");
        assert_eq!(onion_layers(&shot, None, None, &none), set(&[]));
        let both = BTreeSet::from([low.0, free.0]);
        assert_eq!(onion_layers(&shot, Some(low), None, &both), set(&[low, free]), "and those picked with it");
        assert_eq!(onion_layers(&shot, Some(top), None, &both), set(&[top]), "not when it is not among them");
        assert_eq!(onion_layers(&shot, None, Some(PegId(900)), &none), set(&[top, low]), "a peg's, all the way down");
        assert_eq!(onion_layers(&shot, None, Some(PegId(901)), &none), set(&[low]));
        let mixed = BTreeSet::from([901, free.0]);
        assert_eq!(onion_layers(&shot, Some(free), None, &mixed), set(&[low, free]), "a picked peg's too");
    }

    #[test]
    fn ghosts_fade_with_distance_and_are_coloured_by_side() {
        let theme = crate::theme::Theme::dark();
        let sides = [theme.colour(crate::theme::Role::OnionBefore), theme.colour(crate::theme::Role::OnionAfter)];
        let alpha = |distance| Ghost { drawing: DrawingId(1), frame: 0, distance }.color(0.5, sides).a();
        assert_eq!((alpha(-1), alpha(1), alpha(0)), (127, 127, 127));
        assert!(alpha(-2) < alpha(-1) && alpha(-3) < alpha(-2));
        let before = Ghost { drawing: DrawingId(1), frame: 0, distance: -1 }.color(1.0, sides);
        let after = Ghost { drawing: DrawingId(1), frame: 0, distance: 1 }.color(1.0, sides);
        assert!(before.r() > before.g() && after.g() > after.r(), "red before, green after");
    }

    #[test]
    fn a_shift_moves_and_turns_about_the_scene_origin() {
        let shift = Shift { offset: [10.0, 0.0], rotation: std::f64::consts::FRAC_PI_2 };
        let [x, y] = shift.apply([1.0, 0.0]);
        assert!((x - 10.0).abs() < 1e-9 && (y - 1.0).abs() < 1e-9);
        assert_eq!(Shift::default().apply([3.0, 4.0]), [3.0, 4.0]);
    }
}
