//! Putting sounds together in time, and what a waveform shows of one.

use std::sync::Arc;

use crate::Sound;

/// A stretch of a sound at a place in time: on a storyboard's sound track, say. Times are in
/// seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub sound: Arc<Sound>,
    /// When the clip starts.
    pub start: f64,
    /// How far into the sound the clip starts.
    pub offset: f64,
    /// How long the clip is. Past the end of its sound it is silent.
    pub length: f64,
    /// How loud: 1 is as recorded.
    pub gain: f32,
}

impl Sound {
    /// How long the sound is, in seconds.
    pub fn seconds(&self) -> f64 {
        self.info.frames as f64 / f64::from(self.info.sample_rate.max(1))
    }

    /// The sound at a time in seconds, left and right, read between the two samples around
    /// it. A sound of one channel is the same on both sides; of more than two, the first two
    /// are heard. Outside the sound it is silent.
    pub fn at(&self, seconds: f64) -> [f32; 2] {
        let (channels, frames) = (usize::from(self.info.channels), self.info.frames as usize);
        let place = seconds * f64::from(self.info.sample_rate);
        // Not a number is nowhere in the sound.
        if place.is_nan() || place < 0.0 || channels == 0 || place >= frames as f64 {
            return [0.0; 2];
        }
        let (first, part) = (place.floor() as usize, (place - place.floor()) as f32);
        let sample = |frame: usize, channel: usize| {
            if frame < frames { self.samples.get(frame * channels + channel.min(channels - 1)).copied().unwrap_or(0.0) } else { 0.0 }
        };
        let side = |channel: usize| sample(first, channel) * (1.0 - part) + sample(first + 1, channel) * part;
        [side(0), side(1)]
    }

    /// What a waveform draws of the sound: for each stretch of `seconds` from the start, the
    /// lowest and the highest value in it, over all channels.
    pub fn peaks(&self, seconds: f64) -> Vec<[f32; 2]> {
        let channels = usize::from(self.info.channels).max(1);
        let stretch = ((seconds * f64::from(self.info.sample_rate)).round() as usize).max(1) * channels;
        self.samples.chunks(stretch).map(|part| part.iter().fold([0.0f32; 2], |[low, high], sample| [low.min(*sample), high.max(*sample)])).collect()
    }
}

/// Add what `clips` sound like from the time `from`, in seconds, to `out`: left and right for
/// each moment, `rate` moments a second.
pub fn mix(clips: &[Clip], from: f64, rate: u32, out: &mut [f32]) {
    let step = 1.0 / f64::from(rate.max(1));
    let moments = out.len() / 2;
    for clip in clips {
        // The moments of `out` that the clip sounds in.
        let first = (((clip.start - from) / step).ceil().max(0.0)) as usize;
        let end = (((clip.start + clip.length - from) / step).ceil().max(0.0) as usize).min(moments);
        for moment in first..end {
            let [left, right] = clip.sound.at(from + moment as f64 * step - clip.start + clip.offset);
            out[moment * 2] += left * clip.gain;
            out[moment * 2 + 1] += right * clip.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SoundInfo;

    fn sound(rate: u32, channels: u16, samples: Vec<f32>) -> Arc<Sound> {
        let frames = (samples.len() / usize::from(channels)) as u64;
        Arc::new(Sound { info: SoundInfo { sample_rate: rate, channels, frames }, samples })
    }

    #[test]
    fn a_sound_is_read_between_its_samples_and_is_silent_outside() {
        let ramp = sound(10, 1, vec![0.0, 0.2, 0.4, 0.6]);
        assert_eq!((ramp.seconds(), ramp.at(0.1), ramp.at(-0.1), ramp.at(0.4)), (0.4, [0.2, 0.2], [0.0, 0.0], [0.0, 0.0]));
        assert!((ramp.at(0.15)[0] - 0.3).abs() < 1e-6, "half way between two samples");
        let stereo = sound(10, 2, vec![0.5, -0.5, 1.0, -1.0]);
        assert_eq!(stereo.at(0.1), [1.0, -1.0]);
        // A waveform's stretches each hold their lowest and highest.
        let wave = sound(10, 1, vec![0.1, -0.3, 0.7, 0.2, -0.9, 0.0]);
        assert_eq!(wave.peaks(0.2), [[-0.3, 0.1], [0.0, 0.7], [-0.9, 0.0]]);
        assert_eq!(stereo.peaks(1.0), [[-1.0, 1.0]]);
    }

    #[test]
    fn clips_are_heard_where_they_lie_and_add_up() {
        // A second of 0.5 from the time 1, and half a second of a sound's second half from 1.5.
        let (steady, late) = (sound(4, 1, vec![0.5; 4]), sound(8, 1, vec![0.0, 0.0, 0.0, 0.0, 0.25, 0.25, 0.25, 0.25]));
        let clips =
            [Clip { sound: steady, start: 1.0, offset: 0.0, length: 1.0, gain: 1.0 }, Clip { sound: late, start: 1.5, offset: 0.5, length: 0.5, gain: 2.0 }];
        // From the time 0.5, three seconds at 4 moments a second.
        let mut out = vec![0.0; 24];
        mix(&clips, 0.5, 4, &mut out);
        let left: Vec<f32> = out.chunks(2).map(|moment| moment[0]).collect();
        assert_eq!(left, [0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(out.chunks(2).all(|moment| moment[0] == moment[1]), "one channel is heard on both sides");
        // What is there already is added to.
        mix(&clips, 0.5, 4, &mut out);
        assert_eq!(out[8], 2.0);
    }
}
