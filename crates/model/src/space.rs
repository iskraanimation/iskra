//! Cards in space (plan §4.4). A drawing is a flat card; its pegs place it in three dimensions:
//! across, down, and towards the viewer. Most cards face the viewer squarely, and for those
//! two dimensions and a depth are all there is. A peg that tilts leans what it carries out of
//! the picture plane, and then the camera sees it in perspective: its far edge smaller than
//! its near one.
//!
//! [`Space`] is a place and a turn in three dimensions, [`Projection`] is how a card's plane
//! lands in a picture.

use iskra_drawing::{Affine, Point};

use crate::{PegId, Shot, Transform};

/// From one space to another, in three dimensions: across, down, and towards the viewer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    /// What the three axes turn into, row by row.
    pub linear: [[f64; 3]; 3],
    pub offset: [f64; 3],
}

impl Space {
    pub const IDENTITY: Space = Space { linear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], offset: [0.0; 3] };

    /// A flat transform, with what it moves brought `depth` nearer the viewer.
    pub fn flat(flat: &Affine, depth: f64) -> Space {
        let [a, b, c, d, e, f] = flat.m;
        Space { linear: [[a, c, 0.0], [b, d, 0.0], [0.0, 0.0, 1.0]], offset: [e, f, depth] }
    }

    /// What it does across the picture plane, leaving out the depth and any tilt.
    pub fn flat_part(&self) -> Affine {
        let (l, o) = (&self.linear, &self.offset);
        Affine { m: [l[0][0], l[1][0], l[0][1], l[1][1], o[0], o[1]] }
    }

    /// A tilt: `back` degrees about the line across, which leans the top away from the viewer,
    /// then `aside` degrees about the line down, which swings the right side away.
    pub fn tilt([back, aside]: [f64; 2]) -> Space {
        let ((sb, cb), (sa, ca)) = (back.to_radians().sin_cos(), aside.to_radians().sin_cos());
        let about_across = Space { linear: [[1.0, 0.0, 0.0], [0.0, cb, -sb], [0.0, sb, cb]], offset: [0.0; 3] };
        let about_down = Space { linear: [[ca, 0.0, sa], [0.0, 1.0, 0.0], [-sa, 0.0, ca]], offset: [0.0; 3] };
        about_down.mul(&about_across)
    }

    /// `self` after `first`.
    pub fn mul(&self, first: &Space) -> Space {
        let (l, o) = (&self.linear, &first.linear);
        let linear = std::array::from_fn(|row| std::array::from_fn(|column| (0..3).map(|k| l[row][k] * o[k][column]).sum()));
        Space { linear, offset: self.apply(first.offset) }
    }

    pub fn apply(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|row| self.linear[row][0] * x + self.linear[row][1] * y + self.linear[row][2] * z + self.offset[row])
    }
}

impl Transform {
    /// Whether the transform leans what it carries out of the picture plane.
    pub fn tilts(&self) -> bool {
        self.tilt != [0.0, 0.0]
    }

    /// The transform in three dimensions, from the peg's own space to the space the peg is in.
    /// Scale and skew are in the card's own plane; the tilt leans that plane about the pivot,
    /// and the rotation then turns it as the viewer sees it.
    pub fn space(&self) -> Space {
        if !self.tilts() {
            return Space::flat(&self.matrix(), self.depth);
        }
        let [sx, sy] = self.scale;
        let [px, py] = self.pivot;
        let scale = Affine { m: [sx, 0.0, 0.0, sy, 0.0, 0.0] };
        let skew = Affine { m: [1.0, 0.0, self.skew.to_radians().tan(), 1.0, 0.0, 0.0] };
        let in_plane = skew.mul(&scale).mul(&Affine::translate(-px, -py));
        let place = Affine::translate(self.position[0] + px, self.position[1] + py).mul(&Affine::rotate(self.rotation.to_radians()));
        Space::flat(&place, self.depth).mul(&Space::tilt(self.tilt)).mul(&Space::flat(&in_plane, 0.0))
    }
}

/// How near the camera something may be and still be seen, as a part of how far the camera
/// stands from the plane at depth 0.
pub const NEAREST: f64 = 0.01;

/// How a card's plane lands in a picture: a point (x, y) of the plane goes to (X / W, Y / W),
/// where X, Y and W are the three rows of the matrix applied to (x, y, 1). W is how far in
/// front of the camera the point is, as a part of how far the camera stands from the plane at
/// depth 0: where it is not above [`NEAREST`], the point is not seen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub m: [[f64; 3]; 3],
}

impl Projection {
    /// A card that faces the camera squarely: all of it equally far.
    pub fn flat(flat: &Affine) -> Projection {
        let [a, b, c, d, e, f] = flat.m;
        Projection { m: [[a, c, e], [b, d, f], [0.0, 0.0, 1.0]] }
    }

    /// The projection as a flat transform, if the card faces the camera squarely.
    pub fn as_flat(&self) -> Option<Affine> {
        let [x, y, w] = self.m;
        (w[0] == 0.0 && w[1] == 0.0 && w[2] > 0.0).then(|| Affine { m: [x[0] / w[2], y[0] / w[2], x[1] / w[2], y[1] / w[2], x[2] / w[2], y[2] / w[2]] })
    }

    fn rows(&self, at: Point) -> [f64; 3] {
        std::array::from_fn(|row| self.m[row][0] * at.x + self.m[row][1] * at.y + self.m[row][2])
    }

    /// How far in front of the camera a point of the plane is (see [`Projection`]).
    pub fn away(&self, at: Point) -> f64 {
        self.rows(at)[2]
    }

    /// Where a point of the plane is in the picture. `None` for one the camera can't see: at
    /// the camera, or behind it.
    pub fn apply(&self, at: Point) -> Option<Point> {
        let [x, y, w] = self.rows(at);
        (w > NEAREST).then(|| Point::new(x / w, y / w))
    }

    /// The projection of a plane that `first` puts into this one's plane.
    pub fn of(&self, first: &Affine) -> Projection {
        let [a, b, c, d, e, f] = first.m;
        let m = self.m.map(|[x, y, w]| [x * a + y * b, x * c + y * d, x * e + y * f + w]);
        Projection { m }
    }

    /// The projection into a picture that `after` makes of this one's.
    pub fn then(&self, after: &Affine) -> Projection {
        let [a, b, c, d, e, f] = after.m;
        let [x, y, w] = self.m;
        let row = |p: f64, q: f64, r: f64| std::array::from_fn(|i| p * x[i] + q * y[i] + r * w[i]);
        Projection { m: [row(a, c, e), row(b, d, f), w] }
    }

    /// From the picture back to the plane, as a matrix to apply to (X, Y, 1) of a picture's
    /// point: its three rows give x·s, y·s and s, where s is one over how far in front of the
    /// camera the plane's point is. `None` for a card seen edge on.
    pub fn back(&self) -> Option<[[f64; 3]; 3]> {
        let m = &self.m;
        let minor = |r: usize, c: usize| {
            let (r0, r1, c0, c1) = ((r + 1) % 3, (r + 2) % 3, (c + 1) % 3, (c + 2) % 3);
            m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
        };
        let det: f64 = (0..3).map(|c| m[0][c] * minor(0, c)).sum();
        // Against the size of the matrix: a card seen edge on has nearly none.
        let size: f64 = m.iter().flatten().map(|v| v.abs()).fold(0.0, f64::max);
        if !det.is_finite() || det.abs() <= 1e-12 * size.powi(3).max(f64::MIN_POSITIVE) {
            return None;
        }
        Some(std::array::from_fn(|r| std::array::from_fn(|c| minor(c, r) / det)))
    }
}

impl Shot {
    /// Whether, on `frame`, a peg from `parent` up tilts what is parented to `parent`.
    pub fn tilted(&self, parent: Option<PegId>, frame: u32) -> bool {
        self.ancestors(parent).any(|peg| peg.transform_at(frame).tilts())
    }

    /// What the pegs from `parent` up do, on `frame`, to what is parented to `parent`, in
    /// three dimensions: from that node's own space to the scene.
    pub fn space(&self, parent: Option<PegId>, frame: u32) -> Space {
        let pegs: Vec<&crate::Peg> = self.ancestors(parent).collect();
        // As `world` has it: what is above the parent, then the parent's own transform.
        let mut parent = (Space::IDENTITY, Space::IDENTITY);
        for peg in pegs.into_iter().rev() {
            let (over_parent, own) = parent;
            // A peg that does not take all of its parent takes the parent's own transform
            // flat, at its depth: a tilt of the parent's is left out with it.
            let above = match peg.inherit.is_all() {
                true => over_parent.mul(&own).mul(&Space::flat(&self.followed(peg, frame), 0.0)),
                false => {
                    let flat = self.inherited(peg, (Affine::IDENTITY, own.flat_part()), frame);
                    over_parent.mul(&Space::flat(&flat, own.offset[2]))
                }
            };
            parent = (above, peg.transform_at(frame).space());
        }
        parent.0.mul(&parent.1)
    }

    /// How the camera sees, on `frame`, the plane of what is parented to `parent`: from that
    /// node's own space to the frame, whose origin is the frame's middle and whose unit is a
    /// pixel of the shot. For what no peg tilts this is [`Shot::seen`] at its depth.
    ///
    /// The camera itself is not tilted: of the pegs that move it, the position, rotation,
    /// scale and depth are used.
    pub fn projected(&self, parent: Option<PegId>, frame: u32) -> Option<Projection> {
        let camera = &self.camera;
        let space = self.space(parent, frame);
        let from_scene = self.world(camera.parent, frame).inverse()?;
        // The card's plane, as the camera has it: across and down, and how far in front.
        let [a, b, c, d, e, f] = from_scene.m;
        let (l, o) = (&space.linear, &space.offset);
        let across = [a * l[0][0] + c * l[1][0], a * l[0][1] + c * l[1][1], a * o[0] + c * o[1] + e];
        let down = [b * l[0][0] + d * l[1][0], b * l[0][1] + d * l[1][1], b * o[0] + d * o[1] + f];
        let (zoom, distance) = (camera.zoom_at(frame), camera.distance_at(frame));
        let stands = distance + self.depth(camera.parent, frame);
        let away = [-l[2][0] / distance, -l[2][1] / distance, (stands - o[2]) / distance];
        Some(Projection { m: [across.map(|v| v * zoom), down.map(|v| v * zoom), away] })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Peg, Project};

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    /// A shot of 1920 × 1080 with one peg, which has `transform`.
    fn shot_with(transform: Transform) -> (Shot, PegId) {
        let mut project = Project::default();
        let id = PegId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.pegs.push(Arc::new(Peg {
            id,
            name: "Card".to_string(),
            parent: None,
            transform,
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        (shot.clone(), id)
    }

    #[test]
    fn a_peg_that_does_not_tilt_is_in_space_what_it_is_flat() {
        let transform =
            Transform { position: [30.0, -10.0], rotation: 25.0, scale: [2.0, 0.5], skew: 10.0, pivot: [5.0, 7.0], depth: 40.0, ..Transform::default() };
        let (shot, peg) = shot_with(transform);
        let at = Point::new(13.0, -4.0);
        let flat = transform.matrix().apply(at);
        assert_eq!(transform.space().apply([at.x, at.y, 0.0]), [flat.x, flat.y, 40.0]);
        // Seen by the camera, it is where the flat reckoning has it.
        let seen = shot.seen(40.0, 0).unwrap().mul(&shot.world(Some(peg), 0));
        let projected = shot.projected(Some(peg), 0).unwrap();
        assert!(close(projected.apply(at).unwrap(), seen.apply(at)));
        assert!(close(projected.as_flat().unwrap().apply(at), seen.apply(at)), "and it is flat");
        assert!(!shot.tilted(Some(peg), 0));
    }

    #[test]
    fn a_card_leant_back_is_seen_smaller_at_its_top_and_about_its_pivot() {
        let (shot, peg) = shot_with(Transform { tilt: [60.0, 0.0], pivot: [0.0, 100.0], ..Transform::default() });
        assert!(shot.tilted(Some(peg), 0));
        let projected = shot.projected(Some(peg), 0).unwrap();
        assert_eq!(projected.as_flat(), None);
        let at = |x: f64, y: f64| projected.apply(Point::new(x, y)).unwrap();
        // The pivot's line stays where it is, and as wide as it is.
        assert!(close(at(-200.0, 100.0), Point::new(-200.0, 100.0)) && close(at(200.0, 100.0), Point::new(200.0, 100.0)));
        // 400 above it, the card is farther away: narrower, and less high than it is drawn.
        let (left, right) = (at(-200.0, -300.0), at(200.0, -300.0));
        assert!(left.y == right.y && right.x - left.x < 380.0 && right.x - left.x > 250.0, "{left:?} to {right:?}");
        assert!(left.y > -300.0 && left.y < 0.0, "foreshortened: {}", left.y);
        assert!(projected.away(Point::new(0.0, -300.0)) > 1.0 && (projected.away(Point::new(0.0, 100.0)) - 1.0).abs() < 1e-12);

        // Swung aside, its right edge goes away and its left comes nearer.
        let (shot, peg) = shot_with(Transform { tilt: [0.0, 45.0], ..Transform::default() });
        let projected = shot.projected(Some(peg), 0).unwrap();
        assert!(projected.away(Point::new(300.0, 0.0)) > 1.0 && projected.away(Point::new(-300.0, 0.0)) < 1.0);

        // What the pegs above do is done to the tilted card: twice as large is twice as large.
        let (mut shot, peg) = shot_with(Transform { tilt: [60.0, 0.0], ..Transform::default() });
        let above = PegId(peg.0 + 1000);
        Arc::make_mut(&mut shot.pegs[0]).parent = Some(above);
        let twice = Transform { scale: [2.0, 2.0], position: [50.0, 0.0], ..Transform::default() };
        shot.pegs.push(Arc::new(Peg {
            id: above,
            name: "Above".to_string(),
            parent: None,
            transform: twice,
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        let projected = shot.projected(Some(peg), 0).unwrap();
        assert!(close(projected.apply(Point::new(100.0, 0.0)).unwrap(), Point::new(250.0, 0.0)));
    }

    #[test]
    fn a_projection_goes_back_to_the_plane_and_joins_flat_transforms() {
        let (shot, peg) = shot_with(Transform { tilt: [50.0, 20.0], position: [40.0, 10.0], depth: -200.0, ..Transform::default() });
        let projected = shot.projected(Some(peg), 0).unwrap();
        let at = Point::new(120.0, -80.0);
        let seen = projected.apply(at).unwrap();
        let back = projected.back().unwrap();
        let [x, y, s] = back.map(|row| row[0] * seen.x + row[1] * seen.y + row[2]);
        assert!(close(Point::new(x / s, y / s), at) && (1.0 / s - projected.away(at)).abs() < 1e-9);

        let (first, after) = (Affine::scale(0.5), Affine::translate(960.0, 540.0).mul(&Affine::scale(2.0)));
        assert!(close(projected.of(&first).apply(at).unwrap(), projected.apply(first.apply(at)).unwrap()));
        assert!(close(projected.then(&after).apply(at).unwrap(), after.apply(seen)));
        assert_eq!(projected.then(&after).away(at), projected.away(at), "joining a flat transform leaves how far away it is");

        // Edge on, there is no way back; behind the camera, nothing is seen.
        let (shot, peg) = shot_with(Transform { tilt: [90.0, 0.0], ..Transform::default() });
        assert_eq!(shot.projected(Some(peg), 0).unwrap().back(), None);
        let (shot, peg) = shot_with(Transform { depth: 1.0e6, ..Transform::default() });
        assert_eq!(shot.projected(Some(peg), 0).unwrap().apply(at), None);
    }
}
