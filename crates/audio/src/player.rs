//! What is heard: the clips of whatever is open, played from a time on, or a short stretch of
//! them when the playhead is moved by hand. The player knows no sound card. The app's sound
//! output asks it for samples, and the app asks it where it has got to: while sound plays,
//! the picture follows the sound, not the other way round.

use std::sync::{Arc, Mutex};

use crate::{Clip, mix};

/// How long a stretch is heard when the playhead is moved by hand, in seconds.
pub const SCRUB: f64 = 0.08;

/// The most a mix is turned down by when it is louder than a sound card takes.
const CEILING: f32 = 1.0;

#[derive(Default)]
struct State {
    clips: Arc<Vec<Clip>>,
    /// While playing: the time, in seconds, that is heard next.
    playing: Option<f64>,
    /// While a stretch is heard after the playhead was moved: the time heard next, and how
    /// much of the stretch is left.
    scrub: Option<(f64, f64)>,
}

/// See the module.
#[derive(Default)]
pub struct Player {
    state: Mutex<State>,
}

impl Player {
    pub fn new() -> Arc<Player> {
        Arc::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // A panic while the lock was held leaves the state as good as it was.
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// What there is to hear from now on. Playing goes on from where it is.
    pub fn set_clips(&self, clips: Vec<Clip>) {
        self.state().clips = Arc::new(clips);
    }

    pub fn clips(&self) -> Arc<Vec<Clip>> {
        self.state().clips.clone()
    }

    /// Play from a time, in seconds. A time before 0 is silent until 0 comes.
    pub fn play(&self, from: f64) {
        let mut state = self.state();
        (state.playing, state.scrub) = (Some(from), None);
    }

    pub fn stop(&self) {
        let mut state = self.state();
        (state.playing, state.scrub) = (None, None);
    }

    /// Let a short stretch be heard from a time: what is under the playhead as it is moved.
    /// While playing, nothing changes.
    pub fn scrub(&self, at: f64) {
        let mut state = self.state();
        if state.playing.is_none() {
            state.scrub = Some((at, SCRUB));
        }
    }

    /// Whether a stretch is being heard after the playhead was moved.
    pub fn scrubbing(&self) -> bool {
        self.state().scrub.is_some()
    }

    /// While playing: the time that is heard next, in seconds. This is the clock a playing
    /// picture follows.
    pub fn position(&self) -> Option<f64> {
        self.state().playing
    }

    /// Fill `out` with what is heard next: `channels` samples for each moment, `rate` moments
    /// a second. Silence when nothing plays. A sound card of one channel gets both sides
    /// together; of more than two, the first two.
    pub fn fill(&self, out: &mut [f32], channels: usize, rate: u32) {
        out.fill(0.0);
        let moments = out.len() / channels.max(1);
        // The sound card must not wait: if the app has the lock, this stretch is silent.
        let Ok(mut state) = self.state.try_lock() else { return };
        let seconds = moments as f64 / f64::from(rate.max(1));
        let mut heard = vec![0.0f32; moments * 2];
        if let Some(from) = state.playing {
            mix(&state.clips, from, rate, &mut heard);
            state.playing = Some(from + seconds);
        } else if let Some((from, left)) = state.scrub {
            mix(&state.clips, from, rate, &mut heard);
            // The stretch fades out over its length, so that it ends without a click.
            for (moment, pair) in heard.chunks_mut(2).enumerate() {
                let fade = (1.0 - (SCRUB - left + moment as f64 / f64::from(rate.max(1))) / SCRUB).clamp(0.0, 1.0) as f32;
                pair.iter_mut().for_each(|sample| *sample *= fade);
            }
            state.scrub = Some((from + seconds, left - seconds)).filter(|(_, left)| *left > 0.0);
        } else {
            return;
        }
        drop(state);
        for (moment, pair) in heard.chunks(2).enumerate() {
            let [left, right] = [pair[0].clamp(-CEILING, CEILING), pair[1].clamp(-CEILING, CEILING)];
            match &mut out[moment * channels..(moment + 1) * channels] {
                [] => {}
                [one] => *one = (left + right) / 2.0,
                [first, second, ..] => (*first, *second) = (left, right),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Sound, SoundInfo};

    /// Two seconds of 0.5, at 100 samples a second.
    fn clips() -> Vec<Clip> {
        let sound = Arc::new(Sound { info: SoundInfo { sample_rate: 100, channels: 1, frames: 200 }, samples: vec![0.5; 200] });
        vec![Clip { sound, start: 1.0, offset: 0.0, length: 2.0, gain: 1.0 }]
    }

    #[test]
    fn playing_is_heard_from_where_it_is_and_says_where_it_has_got_to() {
        let player = Player::new();
        player.set_clips(clips());
        let mut out = vec![1.0; 200];
        player.fill(&mut out, 2, 100);
        assert!(out.iter().all(|sample| *sample == 0.0) && player.position().is_none(), "stopped, it is silent");
        // From half a second in: half a second of silence, then the sound, on both sides.
        player.play(0.5);
        player.fill(&mut out, 2, 100);
        assert_eq!((out[98], out[99], out[100], out[101], out[199]), (0.0, 0.0, 0.5, 0.5, 0.5));
        assert!((player.position().unwrap() - 1.5).abs() < 1e-9, "a second of sound was asked for");
        // One channel gets both sides together; four get the first two.
        let (mut one, mut four) = (vec![0.0; 10], vec![0.0; 40]);
        player.fill(&mut one, 1, 100);
        player.fill(&mut four, 4, 100);
        assert_eq!((one[0], &four[0..4]), (0.5, &[0.5, 0.5, 0.0, 0.0][..]));
        player.stop();
        player.fill(&mut out, 2, 100);
        assert!(out.iter().all(|sample| *sample == 0.0) && player.position().is_none());
    }

    #[test]
    fn a_moved_playhead_lets_a_short_stretch_be_heard_that_fades_out() {
        let player = Player::new();
        player.set_clips(clips());
        player.scrub(1.5);
        let mut out = vec![0.0; 40];
        player.fill(&mut out, 2, 100);
        assert!(out[0] > 0.45 && out[14] < out[0] && out[14] > 0.0 && out[20] == 0.0, "it fades over {SCRUB} seconds: {out:?}");
        assert!(player.position().is_none(), "that is not playing");
        player.fill(&mut out, 2, 100);
        assert!(out.iter().all(|sample| *sample == 0.0), "and it is heard once");
        // While playing, a scrub changes nothing.
        player.play(1.0);
        player.scrub(0.0);
        player.fill(&mut out, 2, 100);
        assert_eq!(out[0], 0.5);
    }
}
