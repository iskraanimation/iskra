//! The log: what the app did and what went wrong, kept to be copied into a report. The Log
//! panel shows it; the desktop app also writes it to a file beside the preferences, which is
//! still there after a crash, and is the only place a Windows release build can say anything
//! (it has no console).
//!
//! What is said: how the app started (the graphics card, the sound output, where pen input
//! comes from), every command that is run, every problem the status bar shows, and what the pen
//! reports at each touch of the canvas.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use crate::stylus::PenSample;

/// How many lines are kept; the oldest go first.
const KEPT: usize = 2000;

/// The app and the machine it runs on, as the first line of every report.
pub fn about() -> String {
    format!("Iskra {} on {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH)
}

struct Line {
    /// Seconds since the app started.
    at: f32,
    text: String,
    /// How often it was said in a row.
    times: u32,
}

impl Line {
    fn shown(&self) -> String {
        match self.times {
            1 => format!("{:9.3}  {}", self.at, self.text),
            times => format!("{:9.3}  {} (×{times})", self.at, self.text),
        }
    }
}

struct Inner {
    started: Instant,
    lines: VecDeque<Line>,
    file: Option<(PathBuf, std::fs::File)>,
}

/// The log. Cloning shares it, and any thread may write to it.
#[derive(Clone)]
pub struct Log(Arc<Mutex<Inner>>);

impl Default for Log {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Inner { started: Instant::now(), lines: VecDeque::new(), file: None })))
    }
}

impl Log {
    /// The log is read and written even after a thread panicked while holding it: the panic is
    /// what it is there to report.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Add a line. The same line said again at once is counted, not repeated.
    pub fn say(&self, text: impl Into<String>) {
        let text = text.into();
        let mut inner = self.inner();
        let at = inner.started.elapsed().as_secs_f32();
        if let Some((_, file)) = &mut inner.file {
            // A log that can't be written is not worth stopping for.
            let _ = writeln!(file, "{at:9.3}  {text}");
        }
        match inner.lines.back_mut() {
            Some(last) if last.text == text => last.times += 1,
            _ => {
                if inner.lines.len() == KEPT {
                    inner.lines.pop_front();
                }
                inner.lines.push_back(Line { at, text, times: 1 });
            }
        }
    }

    /// Write the log to `path` as well, from its first line: the file is begun anew at each
    /// start of the app. Without the file the log is only in the panel.
    pub fn to_file(&self, path: &Path) {
        let begun = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::File::create(path));
        match begun {
            Ok(mut file) => {
                let _ = writeln!(file, "{}", self.text());
                self.inner().file = Some((path.to_path_buf(), file));
            }
            Err(why) => self.say(format!("the log is not written to {}: {why}", path.display())),
        }
    }

    /// The file the log is written to.
    pub fn file(&self) -> Option<PathBuf> {
        self.inner().file.as_ref().map(|(path, _)| path.clone())
    }

    fn lines(&self) -> Vec<String> {
        self.inner().lines.iter().map(Line::shown).collect()
    }

    /// The whole log as it is copied: what the app is and runs on, then the lines.
    pub fn text(&self) -> String {
        std::iter::once(about()).chain(self.lines()).collect::<Vec<_>>().join("\n")
    }

    pub fn clear(&self) {
        self.inner().lines.clear();
    }

    /// The Log panel: the lines, the newest in view, and a button that copies them all.
    /// A part of the bar over the Log panel: Copy and Clear, or the file it is written to.
    pub fn bar_part(&self, ui: &mut egui::Ui, part: &str) {
        match part {
            "logcopy" => {
                if ui.button("Copy").on_hover_text("Copy the whole log, to paste it into a report").clicked() {
                    ui.ctx().copy_text(self.text());
                }
                if ui.button("Clear").clicked() {
                    self.clear();
                }
            }
            "logfile" => match self.file() {
                Some(path) => {
                    let path = path.display().to_string();
                    ui.weak(&path).on_hover_text(format!("The log is also written to {path}, where it is still to be found after a crash"));
                }
                None => drop(ui.weak("Not written to a file")),
            },
            _ => {}
        }
    }

    pub fn show(&self, ui: &mut egui::Ui) {
        let mut lines = vec![about()];
        lines.extend(self.lines());
        let row = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::both().auto_shrink(false).stick_to_bottom(true).show_rows(ui, row, lines.len(), |ui, rows| {
            for line in &lines[rows] {
                ui.add(egui::Label::new(egui::RichText::new(line).monospace()).extend());
            }
        });
    }
}

/// One touch of a pen or a finger, from down to up.
#[derive(Default)]
struct Contact {
    moves: u32,
    /// How many of its events came with no force.
    forceless: u32,
    /// The least and the most force it reported.
    force: Option<(f32, f32)>,
}

impl Contact {
    fn take(&mut self, force: Option<f32>) {
        match force {
            Some(force) => self.force = Some(self.force.map_or((force, force), |(least, most)| (least.min(force), most.max(force)))),
            None => self.forceless += 1,
        }
    }
}

/// Says in the log what pen input arrives, a line or two a stroke: enough to tell on a machine
/// nobody has tried whether the pen is seen at all, and whether it has pressure.
#[derive(Default)]
pub struct PenLog {
    contact: Option<Contact>,
}

impl PenLog {
    /// Read this frame's events. `sample` is the pen as the app has it now (`None` = a mouse).
    pub fn watch(&mut self, log: &Log, events: &[egui::Event], sample: Option<PenSample>) {
        let force_of = |force: Option<f32>| force.map_or("no force".to_string(), |force| format!("force {force:.3}"));
        for event in events {
            match event {
                egui::Event::Touch { phase: egui::TouchPhase::Start, force, .. } => {
                    let mut contact = Contact::default();
                    contact.take(*force);
                    self.contact = Some(contact);
                    log.say(format!("touch down: {}", force_of(*force)));
                }
                egui::Event::Touch { phase: egui::TouchPhase::Move, force, .. } => {
                    if let Some(contact) = &mut self.contact {
                        contact.moves += 1;
                        contact.take(*force);
                    }
                }
                egui::Event::Touch { phase: egui::TouchPhase::End | egui::TouchPhase::Cancel, .. } => {
                    if let Some(contact) = self.contact.take() {
                        let force = match contact.force {
                            Some((least, most)) => format!("force {least:.3} to {most:.3}"),
                            None => "no force".to_string(),
                        };
                        log.say(format!("touch up: {} moves, {force}, {} events without force", contact.moves, contact.forceless));
                    }
                }
                egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: true, .. } => log.say(match sample {
                    Some(pen) => format!(
                        "pointer down: {} at {:.0}%, tilt {:.0} {:.0}, rotation {:.0}",
                        if pen.eraser { "the eraser end" } else { "a pen" },
                        pen.pressure * 100.0,
                        pen.tilt_x,
                        pen.tilt_y,
                        pen.rotation
                    ),
                    None => "pointer down: a mouse (no pen is reported)".to_string(),
                }),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(phase: egui::TouchPhase, force: Option<f32>) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(0), phase, pos: egui::Pos2::ZERO, force }
    }

    /// The lines without their times.
    fn said(log: &Log) -> Vec<String> {
        log.lines().iter().map(|line| line[11..].to_string()).collect()
    }

    #[test]
    fn a_playback_is_said_with_how_fast_it_went_and_what_it_went_past() {
        // From frame 0, at 40 ms a frame, then one wait of 200 ms that went past 4 frames.
        let mut played = Playback::new(10.0, 0, false);
        assert_eq!(played.said("`Shot_01`", 25.0), None, "nothing shown, nothing said");
        for frame in 1..=10 {
            played.frame(10.0 + f64::from(frame) * 0.04, frame);
        }
        played.frame(10.4, 10);
        played.frame(10.6, 15);
        let said = played.said("`Shot_01`", 25.0).unwrap();
        assert_eq!(
            said,
            "played `Shot_01`: 11 frames in 0.6 s, 18.3 fps of 25, 4 frames gone past, the longest wait 200 ms (to frame 16), drawn on the processor"
        );
        // Round the loop, back to the first frame, is no frame gone past.
        played.frame(10.64, 0);
        let again = played.said("x", 25.0).unwrap();
        assert!(again.contains("12 frames in 0.6 s") && again.contains("4 frames gone past"), "{again}");
    }

    #[test]
    fn a_line_said_again_is_counted_and_the_oldest_lines_go() {
        let log = Log::default();
        log.say("one");
        log.say("two");
        log.say("two");
        assert_eq!(said(&log), ["one", "two (×2)"]);
        assert!(log.text().starts_with("Iskra "), "a report begins with what the app is");
        for line in 0..KEPT + 5 {
            log.say(format!("line {line}"));
        }
        assert_eq!(log.lines().len(), KEPT);
        assert_eq!(said(&log)[0], "line 5");
        log.clear();
        assert_eq!(log.text(), about());
    }

    #[test]
    fn the_file_has_what_was_said_before_it_and_after() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("made/iskra.log");
        let log = Log::default();
        log.say("before");
        log.to_file(&path);
        log.say("after");
        let written = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], about());
        assert!(lines[1].ends_with("before") && lines[2].ends_with("after"));
        assert_eq!(log.file(), Some(path));
    }

    #[test]
    fn a_stroke_of_the_pen_is_two_lines_with_the_range_of_its_force() {
        let (log, mut pen) = (Log::default(), PenLog::default());
        pen.watch(&log, &[touch(egui::TouchPhase::Start, Some(0.1)), touch(egui::TouchPhase::Move, Some(0.75))], None);
        pen.watch(&log, &[touch(egui::TouchPhase::Move, None), touch(egui::TouchPhase::End, None)], None);
        assert_eq!(said(&log), ["touch down: force 0.100", "touch up: 2 moves, force 0.100 to 0.750, 1 events without force"]);
        log.clear();
        let press = egui::Event::PointerButton { pos: egui::Pos2::ZERO, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE };
        pen.watch(&log, std::slice::from_ref(&press), None);
        let sample = PenSample { pressure: 0.5, tilt_x: 10.0, tilt_y: -20.0, rotation: 90.0, eraser: true };
        pen.watch(&log, &[press], Some(sample));
        assert_eq!(said(&log), ["pointer down: a mouse (no pen is reported)", "pointer down: the eraser end at 50%, tilt 10 -20, rotation 90"]);
    }
}

/// How a playback went, said in the log when it stops: how many frames the window showed
/// and how fast, the longest wait for one, and how many frames of the film it went past
/// without showing, as when it falls behind the frame rate.
#[derive(Debug)]
pub struct Playback {
    /// The app's clock when it began, and when the last frame was shown.
    began: f64,
    last: f64,
    /// The playhead then.
    playhead: u32,
    shown: u32,
    skipped: u32,
    /// The longest wait between two frames shown, and the frame it ended on.
    slowest: (f64, u32),
    /// Whether the viewport was drawn on the graphics card.
    on_card: bool,
}

impl Playback {
    pub fn new(now: f64, playhead: u32, on_card: bool) -> Self {
        Playback { began: now, last: now, playhead, shown: 0, skipped: 0, slowest: (0.0, playhead), on_card }
    }

    /// A frame of the window drawn at `now`, with the playhead on `playhead`.
    pub fn frame(&mut self, now: f64, playhead: u32) {
        if playhead == self.playhead {
            return;
        }
        // Forward by more than one, frames were gone past; back, it went round the loop.
        if playhead > self.playhead {
            self.skipped += playhead - self.playhead - 1;
        }
        let waited = now - self.last;
        if waited > self.slowest.0 {
            self.slowest = (waited, playhead);
        }
        (self.last, self.playhead, self.shown) = (now, playhead, self.shown + 1);
    }

    /// What is said of it, of `what` played at `rate` frames a second. `None` for one that
    /// showed no frame.
    pub fn said(&self, what: &str, rate: f64) -> Option<String> {
        if self.shown == 0 {
            return None;
        }
        let seconds = (self.last - self.began).max(1e-6);
        let drawn = if self.on_card { "on the graphics card" } else { "on the processor" };
        Some(format!(
            "played {what}: {} frames in {seconds:.1} s, {:.1} fps of {rate}, {} frames gone past, the longest wait {:.0} ms (to frame {}), drawn {drawn}",
            self.shown,
            f64::from(self.shown) / seconds,
            self.skipped,
            self.slowest.0 * 1000.0,
            self.slowest.1 + 1,
        ))
    }
}
