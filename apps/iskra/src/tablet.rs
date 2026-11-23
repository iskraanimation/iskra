//! Pen pressure, tilt, rotation and the eraser end on macOS and Linux X11.
//!
//! winit drops tablet data on both, so `photocraft-tablet` reads it beside winit (an AppKit local
//! event monitor; XInput2 raw events on a second X connection) and this module writes each
//! sample into the UI's [`StylusFeed`]. Windows needs nothing here: winit forwards pen pressure
//! as touch force, which the UI reads itself. What was set up is said in the log.

#[cfg(any(target_os = "macos", target_os = "linux"))]
use iskra_ui::log::Log;
use iskra_ui::stylus::{PenSample, StylusFeed};
use photocraft_tablet::Sample;

/// Writes tablet samples into `feed` (`None` = a mouse).
#[cfg_attr(not(any(target_os = "macos", target_os = "linux", test)), allow(dead_code))]
fn sink(feed: StylusFeed) -> impl Fn(Option<Sample>) + Send + 'static {
    move |sample| {
        feed.set(sample.map(|s| PenSample { pressure: s.pressure, tilt_x: s.tilt_x, tilt_y: s.tilt_y, rotation: s.rotation, eraser: s.eraser }));
    }
}

/// Install the AppKit tablet monitor. Call on the main thread once winit has created the
/// application (from eframe's app creation), and keep the result while the window lives.
/// If it fails, a pen draws like a mouse.
#[cfg(target_os = "macos")]
pub fn install_macos(feed: &StylusFeed, log: &Log) -> Option<photocraft_tablet::macos::Monitor> {
    match photocraft_tablet::macos::Monitor::install(sink(feed.clone())) {
        Ok(monitor) => {
            log.say("pen: read from AppKit tablet events");
            Some(monitor)
        }
        Err(e) => {
            log.say(format!("pen: {e}"));
            None
        }
    }
}

/// Start the XInput2 reader when the window is on X11 (Xwayland included). On native Wayland
/// `$DISPLAY` is Xwayland, which sees none of this window's input, so nothing starts and a pen
/// has no pressure there yet.
#[cfg(target_os = "linux")]
pub fn spawn_x11(feed: &StylusFeed, cc: &eframe::CreationContext<'_>, log: &Log) {
    use eframe::wgpu::rwh::{HasDisplayHandle, RawDisplayHandle};
    let x11 = cc.display_handle().is_ok_and(|handle| matches!(handle.as_raw(), RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_)));
    if !x11 {
        log.say("pen: pressure is not available on this display server yet");
        return;
    }
    match photocraft_tablet::x11::spawn(None, sink(feed.clone())) {
        Ok(_) => log.say("pen: read from XInput2"),
        Err(e) => log.say(format!("pen: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use photocraft_tablet::{Update, appkit};

    use super::*;

    /// Raw AppKit values → the tablet crate's mapping → the sink → the feed the UI reads.
    #[test]
    fn a_tablet_event_reaches_the_stylus_feed() {
        let feed = StylusFeed::default();
        let sink = sink(feed.clone());
        let mut state = appkit::State::default();
        let mut send = |event: appkit::RawEvent| {
            if let Update::Set(sample) = state.handle(&event) {
                sink(sample);
            }
        };
        send(appkit::RawEvent { kind: appkit::event_type::LEFT_MOUSE_DRAGGED, subtype: appkit::subtype::TABLET_POINT, pressure: 0.25, ..Default::default() });
        assert_eq!(feed.get().map(|s| (s.pressure, s.eraser)), Some((0.25, false)));
        send(appkit::RawEvent { kind: appkit::event_type::LEFT_MOUSE_DRAGGED, subtype: appkit::subtype::MOUSE, pressure: 1.0, ..Default::default() });
        assert_eq!(feed.get(), None, "a mouse clears the pen sample");
    }
}
