//! Media items (plan §4.1, §7.11): pictures and sound that were brought into the project. A
//! media item refers to a file on disk and keeps what was read of it when it was brought in;
//! the file's pixels and samples stay where they are and are read when they are needed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use effectcraft_time::FrameRate;
use serde::{Deserialize, Serialize};

use crate::{BinId, MediaId, Project};

/// How a picture's file keeps its colours with their alpha.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Alpha {
    /// The colours as they are, apart from the alpha.
    #[default]
    Straight,
    /// The colours times the alpha: what it would be over black.
    PremultipliedBlack,
    /// What it would be over white.
    PremultipliedWhite,
}

impl Alpha {
    pub fn is_straight(&self) -> bool {
        *self == Alpha::Straight
    }

    /// Make `rgba`, straight pixels, as a file that keeps its colours so would have them: what
    /// [`Alpha::read`] undoes, but where an alpha of nothing lost its colour.
    pub fn write(self, rgba: &mut [u8]) {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                let times = (u32::from(*channel) * alpha + 127) / 255;
                *channel = match self {
                    Alpha::Straight => *channel,
                    Alpha::PremultipliedBlack => times as u8,
                    Alpha::PremultipliedWhite => (times + 255 - alpha).min(255) as u8,
                };
            }
        }
    }

    /// Make `rgba`, a picture's pixels as its file has them, straight, as a drawing keeps
    /// them; or, `opaque`, every pixel whole, its colours as the file has them.
    pub fn read(self, opaque: bool, rgba: &mut [u8]) {
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            if opaque {
                pixel[3] = 255;
                continue;
            }
            if self == Alpha::Straight {
                continue;
            }
            if alpha == 0 {
                pixel.copy_from_slice(&[0; 4]);
                continue;
            }
            for channel in &mut pixel[..3] {
                // Over white, what is not covered is white: take it off first.
                let over = match self {
                    Alpha::PremultipliedWhite => u32::from(*channel).saturating_sub(255 - alpha),
                    _ => u32::from(*channel),
                };
                *channel = ((over * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

/// What a media item is, with what was read of its file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MediaKind {
    /// One picture.
    Image { width: u32, height: u32 },
    /// Numbered pictures that are the frames of a clip, one file a frame. The item's file is
    /// the first of them; `frames` is how many numbers the sequence runs over, and a number
    /// with no file is a frame that shows nothing.
    ImageSequence { width: u32, height: u32, frames: u32 },
    /// Sound: `frames` samples for each of its channels.
    Sound { sample_rate: u32, channels: u16, frames: u64 },
}

impl MediaKind {
    /// What it is called in a list.
    pub fn name(&self) -> &'static str {
        match self {
            MediaKind::Image { .. } => "Image",
            MediaKind::ImageSequence { .. } => "Image Sequence",
            MediaKind::Sound { .. } => "Sound",
        }
    }

    /// How large its pictures are, in pixels.
    pub fn size(&self) -> Option<[u32; 2]> {
        match *self {
            MediaKind::Image { width, height } | MediaKind::ImageSequence { width, height, .. } => Some([width, height]),
            MediaKind::Sound { .. } => None,
        }
    }

    /// How long it is in frames of a film at `rate`: a picture is one frame, a sequence one
    /// frame a file, and a sound as many as it lasts, the last one begun counted.
    pub fn length(&self, rate: FrameRate) -> u32 {
        match *self {
            MediaKind::Image { .. } => 1,
            MediaKind::ImageSequence { frames, .. } => frames,
            MediaKind::Sound { sample_rate, frames, .. } => {
                let per_second = u128::from(sample_rate.max(1)) * rate.den.max(1) as u128;
                (u128::from(frames) * rate.num.max(1) as u128).div_ceil(per_second).min(u128::from(u32::MAX)) as u32
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Media {
    pub id: MediaId,
    /// Unique among the project's media.
    pub name: String,
    /// The bin it is in. `None` is Master.
    #[serde(default)]
    pub bin: Option<BinId>,
    /// The file it refers to. A file in the project's folder is named from that folder, so the
    /// folder can be moved, or go to another machine, with its media.
    pub path: PathBuf,
    pub kind: MediaKind,
    /// What is spoken in a sound, with when (plan §4.7): typed, read from a subtitle file, or
    /// heard by a speech recogniser. In the order it is spoken.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transcript: Vec<Spoken>,
}

/// A word of a sound's transcript, or a few words that are spoken together, as a subtitle
/// file has them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spoken {
    pub text: String,
    /// When it starts and ends, in thousandths of a second from the start of the sound.
    pub start: u32,
    pub end: u32,
}

impl Media {
    /// Where the file is, for a project whose file is in `folder`.
    pub fn file(&self, folder: Option<&Path>) -> PathBuf {
        match folder {
            Some(folder) if self.path.is_relative() => folder.join(&self.path),
            _ => self.path.clone(),
        }
    }

    /// `file` as a media item names it, for a project whose file is in `folder`: from the
    /// folder if it is inside it, else whole.
    pub fn path_from(folder: Option<&Path>, file: &Path) -> PathBuf {
        folder.and_then(|folder| file.strip_prefix(folder).ok()).map_or_else(|| file.to_path_buf(), Path::to_path_buf)
    }
}

impl Project {
    pub fn media(&self, id: MediaId) -> Option<&Media> {
        self.media.iter().find(|media| media.id == id).map(|media| &**media)
    }

    pub fn media_mut(&mut self, id: MediaId) -> Option<&mut Media> {
        self.media.iter_mut().find(|media| media.id == id).map(Arc::make_mut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_media_item_knows_its_length_and_names_its_file_from_the_projects_folder() {
        let film = FrameRate { num: 24, den: 1 };
        assert_eq!(MediaKind::Image { width: 8, height: 8 }.length(film), 1);
        assert_eq!(MediaKind::ImageSequence { width: 8, height: 8, frames: 30 }.length(film), 30);
        let sound = |frames| MediaKind::Sound { sample_rate: 48_000, channels: 2, frames };
        assert_eq!((sound(48_000).length(film), sound(48_001).length(film), sound(0).length(film)), (24, 25, 0));
        assert_eq!(sound(48_048).length(FrameRate { num: 24_000, den: 1001 }), 24);

        let folder = Path::new("/films/Ball");
        let inside = Media::path_from(Some(folder), Path::new("/films/Ball/Audio/line.wav"));
        let outside = Media::path_from(Some(folder), Path::new("/sounds/line.wav"));
        assert_eq!((inside.as_path(), outside.as_path()), (Path::new("Audio/line.wav"), Path::new("/sounds/line.wav")));
        let media = |path: PathBuf| Media { id: MediaId(1), name: "line".to_string(), bin: None, path, kind: sound(1), transcript: Vec::new() };
        assert_eq!(media(inside.clone()).file(Some(Path::new("/elsewhere/Ball"))), Path::new("/elsewhere/Ball/Audio/line.wav"), "it moves with the folder");
        assert_eq!((media(outside.clone()).file(Some(folder)), media(inside.clone()).file(None)), (outside, inside));
    }
}
