//! The camera (plan §4.4). A shot has one, made with the shot. The drawings are flat cards in
//! space, each at the depth its pegs give it, and the camera looks at them from in front:
//! what is nearer to it than the picture plane is larger, what is farther is smaller, and when
//! the camera moves, the near things move faster across the frame than the far ones.
//!
//! The camera at home sees the plane at depth 0 exactly as it is drawn: a scene unit there is
//! a pixel of the shot. So a shot in which nothing has a depth and the camera has not moved
//! looks as it would without a camera.
//!
//! The camera is moved as drawings are: by a peg it is parented to. The peg's position pans
//! it, its rotation rolls it, its scale lets it see more or less, and its depth moves it
//! towards the scene or back.

use iskra_drawing::Affine;
use serde::{Deserialize, Serialize};

use crate::{CameraId, PegId, Shot, Track};

/// The field of view, top to bottom, of the camera a new shot gets, in degrees.
pub const FIELD_OF_VIEW: f64 = 41.112;

use crate::space::NEAREST;

/// The keyframes of a camera's lens. A value with keys is what they make it on each frame; one
/// without is the same on every frame, as for a peg.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LensAnimation {
    pub zoom: Track<f64>,
    pub distance: Track<f64>,
}

impl LensAnimation {
    pub fn is_empty(&self) -> bool {
        self.zoom.is_empty() && self.distance.is_empty()
    }

    /// The frames on which the lens has a key.
    pub fn frames(&self) -> std::collections::BTreeSet<u32> {
        self.zoom.keys().iter().chain(self.distance.keys()).map(|key| key.frame).collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Camera {
    pub id: CameraId,
    /// The peg the camera is parented to, in the same shot: what moves it.
    pub parent: Option<PegId>,
    /// How far from the plane at depth 0 the camera stands at home, in scene units. It is the
    /// lens: from close by, depth shows strongly; from far away, hardly. Whatever it is, the
    /// plane at depth 0 fills the frame.
    pub distance: f64,
    /// How much larger than drawn the plane at depth 0 is seen. 1 at home.
    pub zoom: f64,
    /// The lens's keyframes: a zoom in, or a field of view that widens, over the shot.
    #[serde(default, skip_serializing_if = "LensAnimation::is_empty")]
    pub animation: LensAnimation,
}

// As for `Layer`: the camera's numbers are always finite.
impl Eq for Camera {}

impl Camera {
    /// The camera of a shot whose frame is `height` scene units high: at home, with the
    /// field of view new shots have.
    pub fn new(id: CameraId, height: u32) -> Self {
        Self { id, parent: None, distance: Self::distance_for(FIELD_OF_VIEW, height), zoom: 1.0, animation: LensAnimation::default() }
    }

    /// How far from the plane at depth 0 a camera stands that sees a frame `height` units
    /// high under `field_of_view` degrees.
    pub fn distance_for(field_of_view: f64, height: u32) -> f64 {
        f64::from(height.max(1)) / (2.0 * (field_of_view.to_radians() / 2.0).tan())
    }

    /// The field of view on `frame`, top to bottom, in degrees, over a frame `height` units high.
    pub fn field_of_view(&self, height: u32, frame: u32) -> f64 {
        2.0 * (f64::from(height.max(1)) / (2.0 * self.distance_at(frame))).atan().to_degrees()
    }

    /// The zoom on `frame`.
    pub fn zoom_at(&self, frame: u32) -> f64 {
        self.animation.zoom.at(frame, false).unwrap_or(self.zoom)
    }

    /// How far from the plane at depth 0 the camera stands at home, on `frame`.
    pub fn distance_at(&self, frame: u32) -> f64 {
        self.animation.distance.at(frame, false).unwrap_or(self.distance)
    }
}

impl Shot {
    /// The frames on which the camera has a keyframe: of its lens, or of the peg that moves it.
    pub fn camera_keys(&self) -> std::collections::BTreeSet<u32> {
        let mut frames = self.camera.animation.frames();
        frames.extend(self.camera.parent.and_then(|peg| self.peg(peg)).into_iter().flat_map(|peg| peg.animation.key_frames()));
        frames
    }

    /// Whether the camera moves on after `frame`, towards a later key: its lens, or the peg
    /// that moves it.
    pub fn camera_moves_after(&self, frame: u32) -> bool {
        let lens = &self.camera.animation;
        let peg = self.camera.parent.and_then(|peg| self.peg(peg));
        lens.zoom.moves_after(frame) || lens.distance.moves_after(frame) || peg.is_some_and(|peg| peg.animation.moves_after(frame))
    }

    /// How the camera's keys on `frame` ease, in and out: those of its peg if it has a key
    /// there, else those of its lens.
    pub fn camera_interp(&self, frame: u32) -> Option<(crate::Interp, crate::Interp)> {
        let lens = &self.camera.animation;
        let of_lens = || lens.zoom.key(frame).or(lens.distance.key(frame)).map(|key| (key.into, key.out));
        self.camera.parent.and_then(|peg| self.peg(peg)).and_then(|peg| peg.animation.interp(frame)).or_else(of_lens)
    }

    /// How the camera sees, on `frame`, what is flat in the scene at `depth`: the transform
    /// from the scene to the frame, whose origin is the frame's middle and whose unit is a
    /// pixel of the shot. `None` for what is at the camera or behind it.
    pub fn seen(&self, depth: f64, frame: u32) -> Option<Affine> {
        let camera = &self.camera;
        let (zoom, distance) = (camera.zoom_at(frame), camera.distance_at(frame));
        // How far in front of the camera the card is.
        let away = distance + self.depth(camera.parent, frame) - depth;
        if away <= distance * NEAREST {
            return None;
        }
        let from_scene = self.world(camera.parent, frame).inverse()?;
        Some(Affine::scale(zoom * distance / away).mul(&from_scene))
    }
}

impl Shot {
    /// What `peg`'s position and scale have to be on `frame` for what it carries to look the
    /// same to the camera with the peg at `depth` as it does now: as large, and in the same
    /// place of the frame. Only its parallax then differs. `None` if the camera can't see it
    /// at one of the two depths, or if the peg is one that moves the camera.
    pub fn kept_in_view(&self, peg: PegId, depth: f64, frame: u32) -> Option<([f64; 2], [f64; 2])> {
        let (peg, camera) = (self.peg(peg)?, &self.camera);
        if self.ancestors(camera.parent).any(|above| above.id == peg.id) {
            return None;
        }
        let pose = peg.transform_at(frame);
        // How far in front of the camera it is, and would be.
        let distance = camera.distance_at(frame);
        let away = distance + self.depth(camera.parent, frame) - self.depth(peg.parent, frame);
        let (now, then) = (away - pose.depth, away - depth);
        if now <= distance * NEAREST || then <= distance * NEAREST {
            return None;
        }
        // Farther by a half is larger by a half, about the point the camera looks at.
        let larger = then / now;
        let (above, look) = (self.world(peg.parent, frame), self.world(camera.parent, frame));
        let [px, py] = pose.pivot;
        let seen = look.inverse()?.apply(above.apply(iskra_drawing::Point::new(pose.position[0] + px, pose.position[1] + py)));
        let kept = above.inverse()?.apply(look.apply(iskra_drawing::Point::new(seen.x * larger, seen.y * larger)));
        Some(([kept.x - px, kept.y - py], [pose.scale[0] * larger, pose.scale[1] * larger]))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_drawing::Point;

    use super::*;
    use crate::{Animation, Peg, Project, Transform};

    fn close(a: Point, b: [f64; 2]) -> bool {
        (a.x - b[0]).abs() < 1e-9 && (a.y - b[1]).abs() < 1e-9
    }

    #[test]
    fn at_home_the_camera_sees_depth_zero_as_drawn_and_what_is_nearer_larger() {
        let mut project = Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        let distance = shot.camera.distance;
        assert!((shot.camera.field_of_view(1080, 0) - FIELD_OF_VIEW).abs() < 1e-9 && (distance - 1440.0).abs() < 1.0, "{distance}");
        assert_eq!(shot.seen(0.0, 0), Some(Affine::IDENTITY));
        let at = |shot: &Shot, depth: f64| shot.seen(depth, 0).map(|seen| seen.apply(Point::new(100.0, 50.0)));
        // Half way to the camera is twice the size; as far again behind the plane, half.
        assert!(close(at(shot, distance / 2.0).unwrap(), [200.0, 100.0]) && close(at(shot, -distance).unwrap(), [50.0, 25.0]));
        assert!(at(shot, distance).is_none() && at(shot, distance * 2.0).is_none(), "at the camera, and behind it");

        // A peg moves the camera: panned right, the scene goes left, and what is near goes
        // farther left than what is far. Pulled back, everything is smaller.
        let peg = PegId(900);
        let pan = Transform { position: [30.0, 0.0], ..Transform::default() };
        shot.pegs.push(Arc::new(Peg {
            id: peg,
            name: "Camera-P".to_string(),
            parent: None,
            transform: pan,
            animation: Animation::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        shot.camera.parent = Some(peg);
        assert!(
            close(at(shot, 0.0).unwrap(), [70.0, 50.0])
                && close(at(shot, distance / 2.0).unwrap(), [140.0, 100.0])
                && close(at(shot, -distance).unwrap(), [35.0, 25.0])
        );
        Arc::make_mut(&mut shot.pegs[0]).transform = Transform { depth: distance, ..Transform::default() };
        assert!(close(at(shot, 0.0).unwrap(), [50.0, 25.0]));
        // Rolled a quarter turn clockwise, the scene turns the other way; zoomed in, it grows.
        Arc::make_mut(&mut shot.pegs[0]).transform = Transform { rotation: 90.0, ..Transform::default() };
        shot.camera.zoom = 2.0;
        assert!(close(at(shot, 0.0).unwrap(), [100.0, -200.0]), "{:?}", at(shot, 0.0));
    }

    #[test]
    fn a_peg_moved_in_depth_can_keep_how_it_looks() {
        let mut project = Project::default();
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (far, rig, distance) = (PegId(900), PegId(901), shot.camera.distance);
        let peg = |id, name: &str, transform| {
            Arc::new(Peg {
                id,
                name: name.to_string(),
                parent: None,
                transform,
                animation: Animation::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        };
        let placed = Transform { position: [200.0, -50.0], scale: [1.0, 2.0], pivot: [10.0, 10.0], ..Transform::default() };
        shot.pegs = vec![peg(far, "Hills", placed), peg(rig, "Camera-P", Transform { position: [40.0, 0.0], ..Transform::default() })];
        shot.camera.parent = Some(rig);
        // Where the camera sees a point of what the peg carries.
        let seen = |shot: &Shot| {
            let pose = shot.peg(far).unwrap().transform_at(0);
            let at = pose.matrix().apply(Point::new(30.0, 60.0));
            shot.seen(pose.depth, 0).unwrap().apply(at)
        };
        let before = seen(shot);
        // As far behind depth 0 as the camera is in front of it: twice as far away.
        let (position, scale) = shot.kept_in_view(far, -distance, 0).unwrap();
        assert_eq!(scale, [2.0, 4.0], "twice as large, to look the same");
        let moved = Transform { position, scale, depth: -distance, ..placed };
        Arc::make_mut(&mut shot.pegs[0]).transform = moved;
        assert!(close(seen(shot), [before.x, before.y]), "{before:?} {:?}", seen(shot));
        assert!(
            shot.kept_in_view(far, distance * 2.0, 0).is_none() && shot.kept_in_view(rig, 5.0, 0).is_none(),
            "not behind the camera, nor the camera's own peg"
        );
    }
}
