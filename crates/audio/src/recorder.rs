//! Recording: what a sound input hears, kept until it is asked for. Like the player, it knows
//! nothing of sound devices: whoever opens an input feeds it the samples that come, and it
//! keeps them while it is recording.
//!
//! The recorder says that it wants an input, and the app's shell opens one for as long as it
//! does, so that the system asks for the microphone only when something is recorded.

use std::sync::{Arc, Mutex};

/// What was recorded: the samples of all channels in turn, as they came.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Take {
    pub samples: Vec<f32>,
    pub channels: u16,
    pub rate: u32,
}

impl Take {
    /// How long it is, in seconds.
    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / (f64::from(self.channels.max(1)) * f64::from(self.rate.max(1)))
    }
}

#[derive(Default)]
struct State {
    recording: bool,
    take: Take,
    /// The loudest sample of what came last, from 0 to 1: for a meter.
    level: f32,
    /// Why nothing can be recorded, if an input could not be opened.
    problem: Option<String>,
}

/// See the module.
#[derive(Default)]
pub struct Recorder {
    state: Mutex<State>,
}

impl Recorder {
    pub fn new() -> Arc<Recorder> {
        Arc::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic while the lock was held leaves the state as good as it was.
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Start a new take: from now on, what is fed is kept.
    pub fn start(&self) {
        *self.state() = State { recording: true, ..State::default() };
    }

    /// End the take and hand it over. `None` if nothing was being recorded.
    pub fn stop(&self) -> Option<Take> {
        let mut state = self.state();
        let was = std::mem::take(&mut state.recording);
        was.then(|| std::mem::take(&mut state.take))
    }

    /// Whether a take is being recorded: an input is wanted for as long as one is.
    pub fn is_recording(&self) -> bool {
        self.state().recording
    }

    /// How long the take is so far, in seconds, and how loud what came last was, from 0 to 1.
    pub fn progress(&self) -> (f64, f32) {
        let state = self.state();
        (state.take.seconds(), state.level)
    }

    /// What an input heard: `samples` of `channels` channels in turn, at `rate` a second.
    pub fn feed(&self, samples: &[f32], channels: u16, rate: u32) {
        let mut state = self.state();
        if !state.recording {
            return;
        }
        // The first samples say what the take is made of; an input does not change after.
        if state.take.samples.is_empty() {
            (state.take.channels, state.take.rate) = (channels.max(1), rate.max(1));
        }
        state.level = samples.iter().fold(0.0f32, |loudest, sample| loudest.max(sample.abs())).min(1.0);
        state.take.samples.extend_from_slice(samples);
    }

    /// No input could be opened: the take is over before it began, and `why` is to be said.
    pub fn fail(&self, why: &str) {
        let mut state = self.state();
        (state.recording, state.problem) = (false, Some(why.to_string()));
    }

    /// Why the last take could not be recorded, said once.
    pub fn problem(&self) -> Option<String> {
        self.state().problem.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_fed_while_recording_is_the_take() {
        let recorder = Recorder::new();
        recorder.feed(&[0.5; 8], 2, 48_000);
        assert_eq!((recorder.is_recording(), recorder.stop()), (false, None), "nothing is kept before a take is started");
        recorder.start();
        recorder.feed(&[0.25, -0.75, 0.0, 0.5], 2, 48_000);
        recorder.feed(&[0.1; 4], 2, 48_000);
        let (seconds, level) = recorder.progress();
        assert!((seconds - 4.0 / 48_000.0).abs() < 1e-9 && (level - 0.1).abs() < 1e-6, "{seconds} {level}");
        let take = recorder.stop().unwrap();
        assert_eq!((take.samples.len(), take.channels, take.rate), (8, 2, 48_000));
        assert!(!recorder.is_recording() && recorder.stop().is_none());
        // A take that could not be heard is over, and why is said once.
        recorder.start();
        recorder.fail("no microphone");
        assert_eq!((recorder.is_recording(), recorder.problem(), recorder.problem()), (false, Some("no microphone".to_string()), None));
    }
}
