//! Sound (plan §3, R7): reading sound files into samples, putting sounds together in time, and
//! the player that says what is heard next. It is headless and knows no sound card: the app's
//! sound output asks the player for samples.
//!
//! For now it reads WAV, the form dialogue and music are handed over in. Compressed sound
//! (MP3, AAC, FLAC, Ogg) comes with FilmCraft's decoders.

mod mix;
mod player;
mod recorder;
mod wav;

pub use mix::{Clip, mix};
pub use player::{Player, SCRUB};
pub use recorder::{Recorder, Take};
pub use wav::{is_wav, pcm_bytes, read_wav, read_wav_info, write_wav, write_wav16};

/// What a sound file says of itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SoundInfo {
    pub sample_rate: u32,
    pub channels: u16,
    /// How many samples each channel has.
    pub frames: u64,
}

/// A sound, read whole: its samples from -1 to 1, the channels of each moment one after
/// another.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    pub info: SoundInfo,
    pub samples: Vec<f32>,
}

/// Why a sound file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundError {
    /// Not a file of a kind that is read.
    NotSound,
    /// A sound file of a kind this version doesn't read, or a damaged one, with what is wrong.
    Unreadable(String),
}

impl std::fmt::Display for SoundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoundError::NotSound => write!(f, "not a sound file that Iskra reads (WAV for now)"),
            SoundError::Unreadable(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for SoundError {}
