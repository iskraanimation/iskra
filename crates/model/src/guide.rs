//! Drawing guides (plan §5, M7): a ruler, and perspective with one, two or three vanishing
//! points. A guide belongs to a shot and lies on its picture; with one in hand, a stroke goes
//! along a line the guide gives through the point where the stroke began.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GuideId(pub u64);

/// What a guide is. Places are in the units of the shot's picture, from its middle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum GuideKind {
    /// A straight edge through `at`, turned by `angle` degrees: strokes go parallel to it.
    Ruler { at: [f64; 2], angle: f64 },
    /// One to three vanishing points. Strokes go towards one of them; with one point also
    /// level and upright, with two also upright.
    Perspective { points: Vec<[f64; 2]> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DrawGuide {
    pub id: GuideId,
    pub name: String,
    #[serde(flatten)]
    pub kind: GuideKind,
}

// Places and angles are always finite: the commands that set them see to it.
impl Eq for DrawGuide {}

impl GuideKind {
    /// A ruler through the middle of the picture, level.
    pub fn ruler() -> Self {
        GuideKind::Ruler { at: [0.0, 0.0], angle: 0.0 }
    }

    /// Perspective with `points` vanishing points, one to three, for a picture `width` by
    /// `height`: one in the middle; two on the horizon, past the picture's sides; the third
    /// far below.
    pub fn perspective(points: usize, width: f64, height: f64) -> Self {
        let all = [[-width, 0.0], [width, 0.0], [0.0, height * 2.0]];
        GuideKind::Perspective { points: if points <= 1 { vec![[0.0, 0.0]] } else { all[..points.min(3)].to_vec() } }
    }

    /// The directions a stroke that begins at `from` may go in, as unit vectors.
    pub fn directions(&self, from: [f64; 2]) -> Vec<[f64; 2]> {
        match self {
            GuideKind::Ruler { angle, .. } => vec![[angle.to_radians().cos(), angle.to_radians().sin()]],
            GuideKind::Perspective { points } => {
                let towards = points.iter().filter_map(|point| {
                    let (dx, dy) = (point[0] - from[0], point[1] - from[1]);
                    let length = dx.hypot(dy);
                    (length > 1e-6).then(|| [dx / length, dy / length])
                });
                let square: &[[f64; 2]] = match points.len() {
                    0 | 1 => &[[1.0, 0.0], [0.0, 1.0]],
                    2 => &[[0.0, 1.0]],
                    _ => &[],
                };
                towards.chain(square.iter().copied()).collect()
            }
        }
    }

    /// The direction for a stroke that began at `from` and is heading for `to`: the one of
    /// [`GuideKind::directions`] that lies nearest to where it is heading.
    pub fn direction(&self, from: [f64; 2], to: [f64; 2]) -> Option<[f64; 2]> {
        let heading = [to[0] - from[0], to[1] - from[1]];
        let along = |direction: &[f64; 2]| (direction[0] * heading[0] + direction[1] * heading[1]).abs();
        self.directions(from).into_iter().max_by(|a, b| along(a).total_cmp(&along(b)))
    }

    /// The points that are dragged to place the guide: a ruler's own point and one along it,
    /// or the vanishing points.
    pub fn handles(&self) -> Vec<[f64; 2]> {
        match self {
            GuideKind::Ruler { at, angle } => {
                let (sin, cos) = angle.to_radians().sin_cos();
                vec![*at, [at[0] + RULER_ARM * cos, at[1] + RULER_ARM * sin]]
            }
            GuideKind::Perspective { points } => points.clone(),
        }
    }

    /// The guide with its handle `index` dragged to `to`: a ruler's own point moves it, the
    /// other turns it; a vanishing point is moved.
    pub fn with_handle(&self, index: usize, to: [f64; 2]) -> GuideKind {
        match self {
            GuideKind::Ruler { angle, .. } if index == 0 => GuideKind::Ruler { at: to, angle: *angle },
            GuideKind::Ruler { at, .. } => GuideKind::Ruler { at: *at, angle: (to[1] - at[1]).atan2(to[0] - at[0]).to_degrees() },
            GuideKind::Perspective { points } => {
                let mut points = points.clone();
                if let Some(point) = points.get_mut(index) {
                    *point = to;
                }
                GuideKind::Perspective { points }
            }
        }
    }

    /// Whether every number of the guide is finite, and it has the points its kind has.
    pub fn is_sound(&self) -> bool {
        match self {
            GuideKind::Ruler { at, angle } => at.iter().chain([angle]).all(|v| v.is_finite()),
            GuideKind::Perspective { points } => (1..=3).contains(&points.len()) && points.iter().flatten().all(|v| v.is_finite()),
        }
    }

    /// What a new guide of this kind is called.
    pub fn title(&self) -> &'static str {
        match self {
            GuideKind::Ruler { .. } => "Ruler",
            GuideKind::Perspective { points } => match points.len() {
                0 | 1 => "1-Point Perspective",
                2 => "2-Point Perspective",
                _ => "3-Point Perspective",
            },
        }
    }
}

/// How far from a ruler's own point the handle that turns it lies, in the picture's units.
pub const RULER_ARM: f64 = 200.0;

/// A stroke held to a guide: it goes along one line through the point where it began, chosen
/// by where the pen first heads.
#[derive(Clone, Debug, PartialEq)]
pub struct Snap {
    guide: GuideKind,
    from: [f64; 2],
    along: Option<[f64; 2]>,
    /// How far the pen has to go before its heading is taken.
    slack: f64,
    /// The stroke has its first point, where it began.
    begun: bool,
}

impl Snap {
    pub fn new(guide: GuideKind, from: [f64; 2], slack: f64) -> Self {
        Self { guide, from, along: None, slack, begun: false }
    }

    /// Where the stroke is when the pen is at `pen`: on the stroke's line. Until the pen has
    /// gone far enough to say where it is heading, the stroke is where it began, once, and
    /// then `None`.
    pub fn at(&mut self, pen: [f64; 2]) -> Option<[f64; 2]> {
        let offset = [pen[0] - self.from[0], pen[1] - self.from[1]];
        if self.along.is_none() && offset[0].hypot(offset[1]) >= self.slack {
            self.along = self.guide.direction(self.from, pen);
        }
        let Some(along) = self.along else {
            return (!std::mem::replace(&mut self.begun, true)).then_some(self.from);
        };
        let reach = offset[0] * along[0] + offset[1] * along[1];
        Some([self.from[0] + along[0] * reach, self.from[1] + along[1] * reach])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6
    }

    #[test]
    fn a_ruler_holds_a_stroke_parallel_to_it_wherever_the_stroke_begins() {
        let ruler = GuideKind::Ruler { at: [0.0, 0.0], angle: 90.0 };
        let mut snap = Snap::new(ruler.clone(), [300.0, 50.0], 4.0);
        assert_eq!((snap.at([301.0, 51.0]), snap.at([301.0, 52.0])), (Some([300.0, 50.0]), None), "where it began, until it has a heading");
        assert!(near(snap.at([330.0, 90.0]).unwrap(), [300.0, 90.0]));
        assert!(near(snap.at([250.0, 10.0]).unwrap(), [300.0, 10.0]), "and back along the same line");
        // Its handles: the first moves it, the second turns it.
        assert_eq!(ruler.handles(), [[0.0, 0.0], [RULER_ARM * 90f64.to_radians().cos(), RULER_ARM]]);
        assert_eq!(ruler.with_handle(0, [5.0, 6.0]), GuideKind::Ruler { at: [5.0, 6.0], angle: 90.0 });
        assert_eq!(ruler.with_handle(1, [10.0, 10.0]), GuideKind::Ruler { at: [0.0, 0.0], angle: 45.0 });
    }

    #[test]
    fn perspective_holds_a_stroke_towards_a_vanishing_point_or_square() {
        let one = GuideKind::perspective(1, 1920.0, 1080.0);
        assert_eq!((one.title(), one.directions([100.0, 100.0]).len()), ("1-Point Perspective", 3));
        // Heading mostly for the vanishing point, the stroke goes to it; mostly level, level.
        let mut inward = Snap::new(one.clone(), [100.0, 100.0], 2.0);
        assert!(near(inward.at([60.0, 70.0]).unwrap(), [65.0, 65.0]));
        let mut level = Snap::new(one.clone(), [100.0, 100.0], 2.0);
        assert!(near(level.at([160.0, 110.0]).unwrap(), [160.0, 100.0]));
        // A stroke that begins on the vanishing point has only level and upright to go.
        assert_eq!(one.directions([0.0, 0.0]).len(), 2);

        let two = GuideKind::perspective(2, 1920.0, 1080.0);
        assert_eq!(two, GuideKind::Perspective { points: vec![[-1920.0, 0.0], [1920.0, 0.0]] });
        let mut upright = Snap::new(two.clone(), [0.0, 200.0], 2.0);
        assert!(near(upright.at([8.0, 300.0]).unwrap(), [0.0, 300.0]));
        let mut right = Snap::new(two.clone(), [0.0, 200.0], 2.0);
        let held = right.at([192.0, 150.0]).unwrap();
        assert!((held[1] - (200.0 - held[0] * 200.0 / 1920.0)).abs() < 1e-6 && held[0] > 150.0, "on the line to the right point: {held:?}");
        let three = GuideKind::perspective(3, 1920.0, 1080.0);
        assert_eq!((three.directions([0.0, 0.0]).len(), three.handles().len(), three.is_sound()), (3, 3, true));
        assert!(!GuideKind::Perspective { points: vec![] }.is_sound() && !GuideKind::Ruler { at: [f64::NAN, 0.0], angle: 0.0 }.is_sound());
        assert_eq!(two.with_handle(1, [900.0, 20.0]), GuideKind::Perspective { points: vec![[-1920.0, 0.0], [900.0, 20.0]] });
    }

    #[test]
    fn a_guide_is_written_with_its_kind_beside_its_name() {
        let guide = DrawGuide { id: GuideId(7), name: "Street".to_string(), kind: GuideKind::Ruler { at: [1.0, 2.0], angle: 30.0 } };
        let json = serde_json::to_value(&guide).unwrap();
        assert_eq!(json, serde_json::json!({ "id": 7, "name": "Street", "kind": "ruler", "at": [1.0, 2.0], "angle": 30.0 }));
        assert_eq!(serde_json::from_value::<DrawGuide>(json).unwrap(), guide);
    }
}
