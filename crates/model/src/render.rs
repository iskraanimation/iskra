//! What was rendered (plan §7.2a, §7.14): the project keeps a record of each render, with a
//! mark of how the shot was then, so that tomorrow it still says which shots are rendered and
//! which were changed since.
//!
//! A shot carries a *revision*, a number that is another after every change to what the shot
//! looks like, and the project one for its palette and settings. A record that holds the
//! numbers of the day it was made is stale as soon as one of them is another.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Project, Shot, ShotId, StoryboardId};

/// What a render is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Rendered {
    Shot(ShotId),
    /// A storyboard as one movie.
    Film(StoryboardId),
}

/// How what a render shows was when it was made: a render is stale when this is no longer what
/// the project gives for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderMark {
    /// The shot's revision; for a film, a number made of its shots' and of where they lie.
    pub picture: u64,
    /// The project's: its palette, its size, its frame rate, its working space.
    pub look: u64,
    /// The frames that were rendered: the first, and one past the last.
    pub frames: (u32, u32),
    /// A number made of the sound under those frames.
    pub sound: u64,
}

/// One render that was made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRecord {
    pub what: Rendered,
    /// Where it is: a movie's file, or a sequence's folder.
    pub at: PathBuf,
    /// The render settings it was made with, as the export wrote them. The model does not
    /// read them.
    pub settings: String,
    pub mark: RenderMark,
    /// When, in seconds since 1970.
    pub when: u64,
    /// The output it was, where a shot was rendered as passes (plan §7.14): a pass, by its
    /// name, or the whole picture beside them, *Final*. None for the shot rendered as itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<String>,
}

/// One number of several, the same for the same ones in the same order whatever the program
/// was built with: what is kept in a file must still compare tomorrow.
fn mixed(numbers: impl IntoIterator<Item = u64>) -> u64 {
    numbers.into_iter().fold(0xcbf2_9ce4_8422_2325u64, |sum, number| {
        number.to_le_bytes().iter().fold(sum, |sum, byte| (sum ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3))
    })
}

impl Shot {
    /// Whether the two would render alike: all of them the same but what is only said about a
    /// shot, its name, its status and who has it, its bin, its scene and sequence, where its nodes lie in the node view, and what the
    /// timeline keeps for the one who animates: folded and locked rows, markers and the stretch that plays;
    /// and the displays but the main one, which only the viewer looks through.
    pub fn renders_as(&self, other: &Shot) -> bool {
        let bare = |shot: &Shot| Shot {
            name: String::new(),
            status: Default::default(),
            out_to: None,
            bin: None,
            scene: String::new(),
            sequence: String::new(),
            panels: Vec::new(),
            folded: Default::default(),
            unlocked: Default::default(),
            locked: Default::default(),
            markers: Vec::new(),
            notes: Vec::new(),
            range: None,
            places: Default::default(),
            colours: Default::default(),
            display_name: String::new(),
            displays: Vec::new(),
            revision: 0,
            loaded: true,
            ..shot.clone()
        };
        bare(self) == bare(other)
    }
}

impl Project {
    /// Give a new revision to every shot of `self` that would no longer render as it does in
    /// `before`, and to the project if its palette or settings are others. To be called on a
    /// project that was just edited, with the project as it was.
    pub fn stamp(&mut self, before: &Project) {
        if self.settings != before.settings || self.palette != before.palette {
            self.look = self.new_id();
        }
        for at in 0..self.shots.len() {
            let now = &self.shots[at];
            let same = match before.shot(now.id) {
                Some(was) => std::ptr::eq(was, &**now) || was.renders_as(now),
                None => false,
            };
            if !same {
                let revision = self.new_id();
                Arc::make_mut(&mut self.shots[at]).revision = revision;
            }
        }
    }

    /// How a shot's frames `frames`, or a storyboard as a film, is now: what a record of a
    /// render made now would hold. With `sound`, the sound under it counts.
    pub fn render_mark(&self, what: Rendered, frames: (u32, u32), sound: bool) -> Option<RenderMark> {
        // The master volume counts where it is not 1, so that a record made before there was
        // one still says the same.
        let master = Some(self.settings.volume).filter(|volume| *volume != 1.0).map(|volume| u64::from(volume.to_bits()));
        let heard = |clips: Vec<(usize, crate::SoundClip)>| {
            let clips = clips.into_iter().flat_map(|(track, clip)| {
                [track as u64, clip.media.0, u64::from(clip.start), u64::from(clip.offset), u64::from(clip.length), u64::from(clip.gain.to_bits())]
            });
            mixed(clips.chain(master))
        };
        match what {
            Rendered::Shot(id) => {
                let shot = self.shot(id)?;
                let under = self.storyboard_of(id).and_then(|board| Some((board, *board.clip(id)?))).filter(|_| sound);
                let sound = under.map_or(0, |(board, clip)| {
                    // The frames of the film that the rendered frames of the shot are.
                    let film = |frame: u32| (i64::from(clip.start) + i64::from(frame) - i64::from(clip.in_point)).max(0) as u32;
                    let unmuted: Vec<_> =
                        board.sound_between(film(frames.0), film(frames.1)).into_iter().filter(|(track, _)| !board.sound[*track].mute).collect();
                    heard(unmuted)
                });
                Some(RenderMark { picture: shot.revision, look: self.look, frames, sound })
            }
            Rendered::Film(id) => {
                let board = self.storyboard(id)?;
                let shots = board.clips().iter().flat_map(|clip| {
                    let revision = self.shot(clip.shot).map_or(0, |shot| shot.revision);
                    [revision, u64::from(clip.start), u64::from(clip.in_point), u64::from(clip.out_point)]
                });
                let all = board.sound_between(0, board.end()).into_iter().filter(|(track, _)| !board.sound[*track].mute).collect();
                Some(RenderMark { picture: mixed(shots), look: self.look, frames: (0, board.end()), sound: if sound { heard(all) } else { 0 } })
            }
        }
    }

    /// Keep a record of a render. One of the same thing at the same place is replaced.
    pub fn record_render(&mut self, record: RenderRecord) {
        self.renders.retain(|other| !(other.what == record.what && other.at == record.at));
        self.renders.push(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MediaId, SoundClip, SoundClipId, Status};

    #[test]
    fn a_shot_has_a_new_revision_when_it_would_render_otherwise_and_only_then() {
        let before = Project::default();
        let shot = before.shots[0].id;
        // What is only said about a shot changes no picture.
        let mut said = before.clone();
        Arc::make_mut(&mut said.shots[0]).status = Status::Final;
        said.stamp(&before);
        assert_eq!(said.shots[0].revision, before.shots[0].revision);
        // Its length does.
        let mut longer = before.clone();
        Arc::make_mut(&mut longer.shots[0]).duration += 1;
        longer.stamp(&before);
        assert_ne!(longer.shots[0].revision, before.shots[0].revision);
        let mark = |project: &Project| project.render_mark(Rendered::Shot(shot), (0, 10), true).unwrap();
        assert_ne!(mark(&longer), mark(&before));
        // A new shot has a revision of its own; the palette and the settings are the project's.
        let mut more = longer.clone();
        let second = more.new_shot("Shot_02", 12);
        more.shots.push(Arc::new(second));
        more.settings.width = 1280;
        more.stamp(&longer);
        assert!(more.shots[1].revision != 0 && more.shots[0].revision == longer.shots[0].revision && more.look != longer.look);
        assert_ne!(mark(&more), mark(&longer), "another size is another render of every shot");

        // The sound under the frames counts where it is asked to, and the same sound is the same number.
        let mut loud = before.clone();
        let clip = SoundClip { id: SoundClipId(900), media: MediaId(901), start: 4, offset: 0, length: 20, gain: 1.0 };
        Arc::make_mut(&mut loud.storyboards[0]).put_sound(clip, None);
        assert_ne!(mark(&loud).sound, mark(&before).sound);
        assert_eq!(loud.render_mark(Rendered::Shot(shot), (0, 10), false), before.render_mark(Rendered::Shot(shot), (0, 10), false));
        assert_eq!(mark(&loud), mark(&loud.clone()));
        Arc::make_mut(&mut loud.storyboards[0]).sound[0].mute = true;
        assert_eq!(mark(&loud).sound, mark(&before).sound, "a muted track is not in the render");
        // A film is as its shots are and as they lie.
        let board = before.storyboards[0].id;
        let film = |project: &Project| project.render_mark(Rendered::Film(board), (0, 0), true).unwrap();
        assert!(film(&longer) != film(&before) && film(&before).frames == (0, before.storyboards[0].end()));
        assert!(before.render_mark(Rendered::Shot(ShotId(999)), (0, 1), true).is_none());

        // A record of the same thing at the same place takes the earlier one's place.
        let mut project = before.clone();
        let record = |when| RenderRecord {
            what: Rendered::Shot(shot),
            at: PathBuf::from("Renders/Shot_01"),
            settings: "{}".to_string(),
            mark: mark(&before),
            when,
            pass: None,
        };
        project.record_render(record(1));
        project.record_render(record(2));
        project.record_render(RenderRecord { at: PathBuf::from("Renders/Shot_01.mov"), ..record(3) });
        assert_eq!(project.renders.iter().map(|record| record.when).collect::<Vec<_>>(), [2, 3]);
    }
}
