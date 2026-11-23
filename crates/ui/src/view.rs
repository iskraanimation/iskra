//! The viewport's view of the scene: pan, zoom and canvas rotation.
//!
//! Scene units are shot pixels at z = 0, with the origin at the centre of the camera frame and
//! y down (plan §4.4, §4.10).

use egui::emath::Rot2;
use egui::{Pos2, Rect, Vec2};

pub(crate) const MIN_ZOOM: f32 = 0.01;
pub(crate) const MAX_ZOOM: f32 = 64.0;

/// The zooms that Zoom In and Zoom Out step through, as fractions of 100%.
const STEPS: [f32; 25] =
    [0.01, 0.02, 0.03, 0.05, 0.08, 0.12, 0.17, 0.25, 1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0, 40.0, 48.0, 64.0];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Where the scene origin is, in screen points from the centre of the viewport.
    pub pan: Vec2,
    /// Screen points per scene unit.
    pub zoom: f32,
    /// Canvas rotation in radians, clockwise on screen.
    pub rotation: f32,
}

impl Default for View {
    fn default() -> Self {
        Self { pan: Vec2::ZERO, zoom: 1.0, rotation: 0.0 }
    }
}

impl View {
    /// The screen position of a scene point, in a viewport centred on `center`.
    pub fn to_screen(self, center: Pos2, scene: [f64; 2]) -> Pos2 {
        center + self.pan + Rot2::from_angle(self.rotation) * (Vec2::new(scene[0] as f32, scene[1] as f32) * self.zoom)
    }

    /// Pan so that a scene point is in the middle of the viewport, at the zoom and turn
    /// there are.
    pub fn centre_on(&mut self, scene: [f64; 2]) {
        self.pan = -(Rot2::from_angle(self.rotation) * (Vec2::new(scene[0] as f32, scene[1] as f32) * self.zoom));
    }

    /// The scene point under a screen position.
    pub fn to_scene(self, center: Pos2, screen: Pos2) -> [f64; 2] {
        let v = Rot2::from_angle(-self.rotation) * (screen - center - self.pan) / self.zoom;
        [f64::from(v.x), f64::from(v.y)]
    }

    /// Zoom by `factor`, keeping the scene point under `anchor` where it is.
    pub fn zoom_about(&mut self, center: Pos2, anchor: Pos2, factor: f32) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let from_origin = anchor - center - self.pan;
        self.pan += from_origin - from_origin * (zoom / self.zoom);
        self.zoom = zoom;
    }

    /// Rotate the canvas by `radians`, keeping the scene point under `anchor` where it is.
    pub fn rotate_about(&mut self, center: Pos2, anchor: Pos2, radians: f32) {
        let from_origin = anchor - center - self.pan;
        self.pan += from_origin - Rot2::from_angle(radians) * from_origin;
        self.rotation += radians;
    }

    /// Zoom in to the next of the steps above the zoom there is, or out to the next below,
    /// keeping the scene point under `anchor` where it is.
    pub fn step_about(&mut self, center: Pos2, anchor: Pos2, inward: bool) {
        // A zoom a hair off a step, as a drag leaves it, counts as on it.
        let near = |step: f32| (step / self.zoom - 1.0).abs() < 1e-3;
        let next = if inward {
            STEPS.iter().copied().find(|&step| step > self.zoom && !near(step)).unwrap_or(MAX_ZOOM)
        } else {
            STEPS.iter().rev().copied().find(|&step| step < self.zoom && !near(step)).unwrap_or(MIN_ZOOM)
        };
        self.zoom_about(center, anchor, next / self.zoom);
    }

    /// Zoom so that `area`, a rectangle on screen, fills the viewport `viewport` as far as it
    /// can and is in its middle.
    pub fn zoom_to(&mut self, viewport: Rect, area: Rect) {
        let factor = (viewport.width() / area.width()).min(viewport.height() / area.height());
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.pan += viewport.center() - area.center();
        self.zoom_about(viewport.center(), viewport.center(), factor);
    }

    /// Show a `width` × `height` camera frame whole and upright in `viewport`, with a margin.
    pub fn fit(&mut self, viewport: Rect, width: f32, height: f32) {
        let zoom = (viewport.width() / width).min(viewport.height() / height) * 0.9;
        *self = Self { zoom: if zoom.is_finite() { zoom.clamp(MIN_ZOOM, MAX_ZOOM) } else { 1.0 }, ..Self::default() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER: Pos2 = Pos2::new(400.0, 300.0);

    fn close(a: Pos2, b: Pos2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn screen_and_scene_round_trip() {
        let view = View { pan: Vec2::new(30.0, -12.0), zoom: 2.5, rotation: 0.7 };
        let screen = Pos2::new(123.0, 456.0);
        assert!(close(view.to_screen(CENTER, view.to_scene(CENTER, screen)), screen));
        assert!(close(View::default().to_screen(CENTER, [0.0, 0.0]), CENTER), "the scene origin starts at the centre");
    }

    #[test]
    fn zooming_and_rotating_keep_the_point_under_the_pointer() {
        let mut view = View { pan: Vec2::new(30.0, -12.0), zoom: 2.5, rotation: 0.7 };
        let pointer = Pos2::new(600.0, 120.0);
        let under = view.to_scene(CENTER, pointer);
        view.zoom_about(CENTER, pointer, 1.8);
        assert!((view.zoom - 4.5).abs() < 1e-4);
        assert!(close(view.to_screen(CENTER, under), pointer));
        view.rotate_about(CENTER, pointer, -1.1);
        assert!(close(view.to_screen(CENTER, under), pointer));
        view.zoom_about(CENTER, pointer, 1e9);
        assert_eq!(view.zoom, MAX_ZOOM);
        assert!(close(view.to_screen(CENTER, under), pointer), "also when the zoom hits its limit");
    }

    #[test]
    fn fit_shows_the_whole_frame() {
        let mut view = View { pan: Vec2::new(99.0, 99.0), zoom: 7.0, rotation: 1.0 };
        let viewport = Rect::from_center_size(CENTER, Vec2::new(800.0, 600.0));
        view.fit(viewport, 1920.0, 1080.0);
        assert_eq!((view.pan, view.rotation), (Vec2::ZERO, 0.0));
        let corner = view.to_screen(CENTER, [960.0, 540.0]);
        assert!(viewport.contains(corner) && corner.x > 700.0, "{corner:?}");
    }

    #[test]
    fn steps_go_to_the_next_zoom_each_way() {
        let mut view = View::default();
        let pointer = Pos2::new(600.0, 120.0);
        let under = view.to_scene(CENTER, pointer);
        view.step_about(CENTER, pointer, true);
        assert_eq!(view.zoom, 1.5);
        assert!(close(view.to_screen(CENTER, under), pointer), "the point under the pointer stays");
        view.zoom = 0.7;
        view.step_about(CENTER, CENTER, false);
        assert_eq!(view.zoom, 2.0 / 3.0);
        view.step_about(CENTER, CENTER, false);
        assert_eq!(view.zoom, 0.5, "from a step, to the one below it");
        view.zoom = 1.0005;
        view.step_about(CENTER, CENTER, true);
        assert_eq!(view.zoom, 1.5, "a hair off a step counts as on it");
        view.zoom = MAX_ZOOM;
        view.step_about(CENTER, CENTER, true);
        assert_eq!(view.zoom, MAX_ZOOM);
    }

    #[test]
    fn zooming_to_an_area_fills_the_viewport_with_it() {
        let mut view = View { pan: Vec2::new(30.0, -12.0), zoom: 2.0, rotation: 0.3 };
        let viewport = Rect::from_center_size(CENTER, Vec2::new(800.0, 600.0));
        let area = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(100.0, 50.0));
        let middle = view.to_scene(CENTER, area.center());
        view.zoom_to(viewport, area);
        assert!((view.zoom - 16.0).abs() < 1e-4, "eight times: the width fills, the height has room");
        assert!(close(view.to_screen(CENTER, middle), CENTER), "what was in the box's middle is in the viewport's");
        let before = view;
        view.zoom_to(viewport, Rect::from_min_size(area.min, Vec2::ZERO));
        assert_eq!(view, before, "an empty box changes nothing");
    }
}
