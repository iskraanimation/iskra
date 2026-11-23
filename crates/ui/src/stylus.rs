//! Pen input: pressure, and tilt, barrel rotation and the eraser end where the platform reports
//! them. A mouse reads as full pressure with no tilt.
//!
//! Where samples come from:
//!
//! - **macOS and Linux X11**: winit drops tablet data, so the desktop app reads it beside winit
//!   (the vendored `photocraft-tablet` crate) and writes each sample into the [`StylusFeed`].
//! - **Windows**: winit turns `WM_POINTER` pen input into touch events with a force, which egui
//!   forwards as `Event::Touch`; [`Stylus::update`] reads it. Tilt and rotation are not
//!   reported there: the Windows backend is M13's (plan §6).
//! - **Linux Wayland**: no pen input yet (plan §6).

use std::sync::{Arc, Mutex};

/// One pen reading. Tilt is in degrees (-90..90: +x to the right, +y towards the user), rotation
/// in degrees 0..360.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    /// Tip or eraser pressure 0..1; 0 while hovering.
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    /// The eraser end of the pen is in use.
    pub eraser: bool,
}

/// The slot a platform backend writes the current pen sample into (`None` when the pointer is
/// not a pen). Cloning shares the slot.
#[derive(Clone, Debug, Default)]
pub struct StylusFeed(Arc<Mutex<Option<PenSample>>>);

impl StylusFeed {
    pub fn set(&self, sample: Option<PenSample>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = sample;
        }
    }

    pub fn get(&self) -> Option<PenSample> {
        self.0.lock().ok().and_then(|slot| *slot)
    }
}

/// The app's pen state.
#[derive(Clone, Debug, Default)]
pub struct Stylus {
    pub feed: StylusFeed,
    /// Force of the pen or touch contact that is down, from egui's touch events.
    touch: Option<f32>,
    /// Touch events' force is not read: on Windows, when the pen is read through WinTab alone.
    pub ignore_touch: bool,
}

impl Stylus {
    /// Read this frame's touch events. Call once a frame, before [`Stylus::sample`].
    pub fn update(&mut self, events: &[egui::Event]) {
        for event in events {
            if let egui::Event::Touch { phase, force, .. } = event
                && !self.ignore_touch
            {
                match phase {
                    egui::TouchPhase::Start | egui::TouchPhase::Move => {
                        if let Some(force) = force.filter(|f| f.is_finite()) {
                            self.touch = Some(force.clamp(0.0, 1.0));
                        }
                    }
                    egui::TouchPhase::End | egui::TouchPhase::Cancel => self.touch = None,
                }
            }
        }
    }

    /// The current pen sample, or `None` when the pointer is a mouse.
    pub fn sample(&self) -> Option<PenSample> {
        self.feed.get().or_else(|| self.touch.map(|pressure| PenSample { pressure, tilt_x: 0.0, tilt_y: 0.0, rotation: 0.0, eraser: false }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(phase: egui::TouchPhase, force: Option<f32>) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(0), phase, pos: egui::Pos2::ZERO, force }
    }

    #[test]
    fn the_platform_feed_wins_over_touch_force() {
        let mut stylus = Stylus::default();
        assert_eq!(stylus.sample(), None, "a mouse");
        stylus.update(&[touch(egui::TouchPhase::Start, Some(0.4))]);
        assert_eq!(stylus.sample().map(|s| s.pressure), Some(0.4));
        let pen = PenSample { pressure: 0.7, tilt_x: 20.0, tilt_y: -5.0, rotation: 90.0, eraser: true };
        stylus.feed.clone().set(Some(pen));
        assert_eq!(stylus.sample(), Some(pen));
    }

    #[test]
    fn touch_force_is_clamped_and_ends_with_the_contact() {
        let mut stylus = Stylus::default();
        stylus.update(&[touch(egui::TouchPhase::Start, Some(3.0)), touch(egui::TouchPhase::Move, Some(f32::NAN))]);
        assert_eq!(stylus.sample().map(|s| s.pressure), Some(1.0), "clamped; the NaN is ignored");
        stylus.update(&[touch(egui::TouchPhase::Move, None)]);
        assert_eq!(stylus.sample().map(|s| s.pressure), Some(1.0), "a touch without force keeps the last one");
        stylus.update(&[touch(egui::TouchPhase::End, None)]);
        assert_eq!(stylus.sample(), None);
    }

    #[test]
    fn touch_force_is_left_out_when_the_pen_is_read_through_wintab_alone() {
        let mut stylus = Stylus { ignore_touch: true, ..Stylus::default() };
        stylus.update(&[touch(egui::TouchPhase::Start, Some(0.4))]);
        assert_eq!(stylus.sample(), None, "a mouse, as Windows Ink is not read");
        stylus.ignore_touch = false;
        stylus.update(&[touch(egui::TouchPhase::Move, Some(0.4))]);
        assert_eq!(stylus.sample().map(|s| s.pressure), Some(0.4));
    }
}
