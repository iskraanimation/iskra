//! Timings, for the Log panel's *Log timings*: how long the window's frames take and what in
//! them does, and each command, said in the log to be copied into a report of the app slowing
//! down. A stroke is said as it ends, from the pen coming down; the rest every few seconds,
//! when a frame was slow. Off, nothing is measured. They are kept for the thread the window
//! is drawn on.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::log::Log;

/// How long the frames are watched for, between strokes, before the log is told of them.
const EVERY: Duration = Duration::from_secs(5);
/// A frame that took this long is slow: the window is drawn fewer than 30 times a second.
const SLOW: Duration = Duration::from_millis(34);
/// A stroke longer than this is taken to have been let go of without being painted.
const LONGEST_STROKE: Duration = Duration::from_secs(60);
/// How many of the parts of a frame are said, those that took longest first.
const PARTS_SAID: usize = 12;

/// How often something took how long.
#[derive(Default, Clone, Copy)]
struct Tally {
    count: u32,
    total: Duration,
    most: Duration,
}

impl Tally {
    fn add(&mut self, took: Duration) {
        (self.count, self.total, self.most) = (self.count + 1, self.total + took, self.most.max(took));
    }

    fn average_ms(&self) -> f64 {
        ms(self.total) / f64::from(self.count.max(1))
    }
}

fn ms(took: Duration) -> f64 {
    took.as_secs_f64() * 1000.0
}

struct Timed {
    log: Log,
    /// When the frames now counted began, and when the last one did.
    since: Option<Instant>,
    last: Option<Instant>,
    /// The time the app took to draw each frame, and from the start of one to the next.
    frames: Tally,
    gaps: Tally,
    parts: BTreeMap<String, Tally>,
    counts: BTreeMap<&'static str, u64>,
    /// When the pen came down, while a stroke is drawn.
    stroke: Option<Instant>,
}

impl Timed {
    fn begin(&mut self, at: Instant, stroke: Option<Instant>) {
        (self.since, self.last, self.stroke) = (Some(at), None, stroke);
        (self.frames, self.gaps) = (Tally::default(), Tally::default());
        self.parts.clear();
        self.counts.clear();
    }

    /// How the frames went, for `what`: their number, how often they came and how long they
    /// took, then the parts that took longest and what was counted.
    fn said(&self, what: &str) -> String {
        let mut said = format!("timings, {what}: {} frames", self.frames.count);
        if self.gaps.count > 0 {
            said += &format!(", one every {:.1} ms (at most {:.1})", self.gaps.average_ms(), ms(self.gaps.most));
        }
        said += &format!("; the app drew one in {:.1} ms on average, {:.1} at most", self.frames.average_ms(), ms(self.frames.most));
        let mut parts: Vec<(&String, &Tally)> = self.parts.iter().collect();
        parts.sort_by_key(|(_, tally)| std::cmp::Reverse(tally.total));
        let parts: Vec<String> = parts
            .iter()
            .take(PARTS_SAID)
            .map(|(name, tally)| format!("{name} {}× {:.1} ms, at most {:.1}", tally.count, tally.average_ms(), ms(tally.most)))
            .collect();
        if !parts.is_empty() {
            said += &format!(". Of it: {}", parts.join("; "));
        }
        let counts: Vec<String> = self.counts.iter().map(|(name, count)| format!("{name} {count}")).collect();
        if !counts.is_empty() {
            said += &format!(". Counted: {}", counts.join(", "));
        }
        said
    }
}

thread_local! {
    static TIMED: RefCell<Option<Timed>> = const { RefCell::new(None) };
}

/// Measure or stop measuring, saying what is measured in `log`.
pub fn set(on: bool, log: &Log) {
    TIMED.with_borrow_mut(|timed| match (on, timed.is_some()) {
        (true, false) => {
            *timed = Some(Timed {
                log: log.clone(),
                since: None,
                last: None,
                frames: Tally::default(),
                gaps: Tally::default(),
                parts: BTreeMap::new(),
                counts: BTreeMap::new(),
                stroke: None,
            })
        }
        (false, true) => *timed = None,
        _ => {}
    });
}

pub fn on() -> bool {
    TIMED.with_borrow(Option::is_some)
}

/// Measures from now until it is dropped, as a part of the frame named so.
#[must_use]
pub struct Timer(Option<(String, Instant)>);

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some((what, began)) = self.0.take() {
            let took = began.elapsed();
            TIMED.with_borrow_mut(|timed| {
                if let Some(timed) = timed {
                    timed.parts.entry(what).or_default().add(took);
                }
            });
        }
    }
}

/// A [`Timer`] of `what`, which measures nothing while timings are off.
pub fn timer(what: &str) -> Timer {
    timer_of(|| what.to_string())
}

/// [`timer`], its name made only while timings are on.
pub fn timer_of(what: impl FnOnce() -> String) -> Timer {
    Timer(on().then(|| (what(), Instant::now())))
}

/// Count `by` more of `what`: textures made, say.
pub fn count(what: &'static str, by: u64) {
    TIMED.with_borrow_mut(|timed| {
        if let Some(timed) = timed {
            *timed.counts.entry(what).or_default() += by;
        }
    });
}

/// A stroke begins: what came before it is said if a frame of it was slow, and the stroke is
/// counted from here.
pub fn stroke_began() {
    let now = Instant::now();
    let said = TIMED.with_borrow_mut(|timed| {
        let timed = timed.as_mut()?;
        let slow = (timed.frames.most >= SLOW).then(|| (timed.log.clone(), timed.said("before a stroke")));
        timed.begin(now, Some(now));
        slow
    });
    if let Some((log, said)) = said {
        log.say(said);
    }
}

/// The stroke that began is painted, or was refused: it is said, with `state`, what the
/// project and the window hold.
pub fn stroke_ended(state: impl FnOnce() -> String) {
    let now = Instant::now();
    let said = TIMED.with_borrow_mut(|timed| {
        let timed = timed.as_mut()?;
        let began = timed.stroke?;
        let said = timed.said(&format!("a stroke of {:.1} s", (now - began).as_secs_f64()));
        timed.begin(now, None);
        Some((timed.log.clone(), said))
    });
    if let Some((log, said)) = said {
        log.say(said);
        log.say(format!("timings, as it stands: {}", state()));
    }
}

/// A frame the app began to draw at `began` is drawn: every few seconds between strokes, how
/// the frames went is said if one was slow, with `state`.
pub fn frame(began: Instant, state: impl FnOnce() -> String) {
    let took = began.elapsed();
    let said = TIMED.with_borrow_mut(|timed| {
        let timed = timed.as_mut()?;
        if let Some(last) = timed.last.replace(began) {
            timed.gaps.add(began.saturating_duration_since(last));
        }
        timed.frames.add(took);
        let since = *timed.since.get_or_insert(began);
        let long = began.saturating_duration_since(since);
        match timed.stroke {
            Some(stroke) if began.saturating_duration_since(stroke) < LONGEST_STROKE => return None,
            _ if long < EVERY => return None,
            _ => {}
        }
        let said = (timed.frames.most >= SLOW).then(|| (timed.log.clone(), timed.said(&format!("the last {:.1} s", long.as_secs_f64()))));
        timed.begin(began, None);
        said
    });
    if let Some((log, said)) = said {
        log.say(said);
        log.say(format!("timings, as it stands: {}", state()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stroke_is_said_with_its_frames_and_their_parts_and_nothing_is_measured_while_off() {
        let log = Log::default();
        assert!(!on() && timer("nothing").0.is_none());
        set(true, &log);
        stroke_began();
        for _ in 0..3 {
            let began = Instant::now();
            drop(timer("viewport"));
            count("tile textures made", 2);
            frame(began, || unreachable!("said as the stroke ends"));
        }
        stroke_ended(|| "1 shot".to_string());
        let text = log.text();
        assert!(text.contains("timings, a stroke of") && text.contains(": 3 frames, one every"), "{text}");
        assert!(text.contains("Of it: viewport 3×") && text.contains("Counted: tile textures made 6"), "{text}");
        assert!(text.ends_with("timings, as it stands: 1 shot"), "{text}");
        set(false, &log);
        assert!(!on());
    }
}
