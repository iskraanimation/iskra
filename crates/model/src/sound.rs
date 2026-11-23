//! Sound on a storyboard (plan §7.2, R21). Dialogue and music lie on a storyboard's sound
//! tracks, under its shots, as clips of sound media. A shot's sound is the stretch of those
//! tracks that lies under it.
//!
//! Places and lengths are in frames of the film, as the shots' are.

use serde::{Deserialize, Serialize};

use crate::{MediaId, SoundClipId, Storyboard};

/// A stretch of a sound item on a sound track.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundClip {
    pub id: SoundClipId,
    pub media: MediaId,
    /// The frame of the film the clip starts on.
    pub start: u32,
    /// How many frames into its sound the clip starts.
    pub offset: u32,
    pub length: u32,
    /// How loud: 1 is as recorded.
    pub gain: f32,
}

// A gain is always finite.
impl Eq for SoundClip {}

impl SoundClip {
    pub fn end(&self) -> u32 {
        self.start + self.length
    }
}

/// One of a storyboard's sound tracks. Its clips don't run into each other, and are kept in
/// the order they play.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundTrack {
    pub name: String,
    /// The track is not heard.
    #[serde(default)]
    pub mute: bool,
    pub clips: Vec<SoundClip>,
}

impl SoundTrack {
    /// Whether `clip` can lie on the track without running into another of its clips.
    pub fn has_room(&self, clip: &SoundClip) -> bool {
        self.clips.iter().filter(|other| other.id != clip.id).all(|other| other.end() <= clip.start || clip.end() <= other.start)
    }
}

impl Storyboard {
    /// A sound clip, with the track it is on.
    pub fn sound_clip(&self, id: SoundClipId) -> Option<(usize, &SoundClip)> {
        self.sound.iter().enumerate().find_map(|(track, of)| Some((track, of.clips.iter().find(|clip| clip.id == id)?)))
    }

    /// Put a sound clip on a track, or put it again after a change: on `track` if it is given
    /// and has room, else on the first track that has, else on a new one. A `track` past the
    /// last asks for a new one. Returns the track it is on.
    pub fn put_sound(&mut self, clip: SoundClip, track: Option<usize>) -> usize {
        for of in &mut self.sound {
            of.clips.retain(|other| other.id != clip.id);
        }
        let fits = |track: usize| self.sound.get(track).is_some_and(|of| of.has_room(&clip));
        let new = track.is_some_and(|track| track >= self.sound.len());
        let track = track.filter(|track| fits(*track)).or_else(|| (0..self.sound.len()).find(|track| fits(*track)).filter(|_| !new)).unwrap_or_else(|| {
            self.sound.push(SoundTrack { name: format!("Sound {}", self.sound.len() + 1), mute: false, clips: Vec::new() });
            self.sound.len() - 1
        });
        let clips = &mut self.sound[track].clips;
        clips.insert(clips.partition_point(|other| other.start < clip.start), clip);
        track
    }

    /// Take a sound clip off its track.
    pub fn take_sound(&mut self, id: SoundClipId) -> Option<SoundClip> {
        let (track, clip) = self.sound_clip(id).map(|(track, clip)| (track, *clip))?;
        self.sound[track].clips.retain(|other| other.id != id);
        Some(clip)
    }

    /// The frame after the last one that has sound.
    pub fn sound_end(&self) -> u32 {
        self.sound.iter().flat_map(|track| &track.clips).map(SoundClip::end).max().unwrap_or(0)
    }

    /// What of the sound lies between two frames of the film: each clip that is heard there,
    /// with its track, cut to the stretch. This is a shot's sound, for the shot whose clip
    /// lies there.
    pub fn sound_between(&self, from: u32, to: u32) -> Vec<(usize, SoundClip)> {
        let cut = |clip: &SoundClip| {
            let (start, end) = (clip.start.max(from), clip.end().min(to));
            (start < end).then(|| SoundClip { start, offset: clip.offset + (start - clip.start), length: end - start, ..*clip })
        };
        self.sound.iter().enumerate().flat_map(|(track, of)| of.clips.iter().filter_map(cut).map(move |clip| (track, clip))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoryboardId;

    fn clip(id: u64, start: u32, length: u32) -> SoundClip {
        SoundClip { id: SoundClipId(id), media: MediaId(100 + id), start, offset: 0, length, gain: 1.0 }
    }

    #[test]
    fn sound_clips_lie_on_tracks_without_running_into_each_other() {
        let mut board = Storyboard::new(StoryboardId(1), "Film");
        assert_eq!((board.put_sound(clip(1, 0, 48), None), board.put_sound(clip(2, 48, 24), None)), (0, 0), "end to end, on the first track");
        assert_eq!(board.put_sound(clip(3, 40, 24), None), 1, "what would run into another goes a track down");
        assert_eq!(board.put_sound(clip(4, 100, 10), Some(1)), 1, "on the track that is asked for, if it has room");
        assert_eq!(board.put_sound(clip(5, 60, 10), Some(1)), 2, "and where neither has, on a new one");
        let tracks = |board: &Storyboard| board.sound.iter().map(|track| track.clips.iter().map(|clip| clip.id.0).collect::<Vec<_>>()).collect::<Vec<_>>();
        assert_eq!((tracks(&board), board.sound_end(), board.sound[2].name.as_str()), (vec![vec![1, 2], vec![3, 4], vec![5]], 110, "Sound 3"));
        // Put again where it has room, a clip moves; its own old place is no obstacle.
        assert_eq!(board.put_sound(clip(3, 44, 24), Some(1)), 1);
        assert_eq!(board.put_sound(SoundClip { start: 80, ..clip(5, 0, 10) }, Some(0)), 0);
        assert_eq!(tracks(&board), [vec![1, 2, 5], vec![3, 4], vec![]]);
        assert_eq!(board.sound_clip(SoundClipId(5)).map(|(track, clip)| (track, clip.start)), Some((0, 80)));
        assert_eq!((board.take_sound(SoundClipId(4)).map(|clip| clip.start), board.take_sound(SoundClipId(4))), (Some(100), None));

        // A shot from frame 40 to 60 hears the end of the first clip, the start of the
        // second, and the part of the third that lies under it.
        let under: Vec<(usize, u64, u32, u32, u32)> =
            board.sound_between(40, 60).iter().map(|(track, clip)| (*track, clip.id.0, clip.start, clip.offset, clip.length)).collect();
        assert_eq!(under, [(0, 1, 40, 40, 8), (0, 2, 48, 0, 12), (1, 3, 44, 0, 16)]);
        assert!(board.sound_between(200, 300).is_empty());
        // A track past the last asks for a new one, though the others have room.
        assert_eq!(board.put_sound(clip(6, 200, 10), Some(3)), 3);
        assert_eq!(board.sound[3].name, "Sound 4");
    }
}
