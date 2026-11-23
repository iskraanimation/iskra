//! A storyboard: a timeline of shots (plan §4.1, §7.2b). It refers to its shots, which are
//! items of the project in their own right.

use serde::{Deserialize, Serialize};

use crate::{BinId, ShotId, SoundTrack, StoryboardId};

/// A shot placed on a storyboard. The shot's frames from `in_point` up to `out_point` are in
/// the cut; the frames outside them are its handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clip {
    pub shot: ShotId,
    /// The storyboard frame the clip starts on.
    pub start: u32,
    /// The first frame of the shot that is in the cut.
    #[serde(rename = "in")]
    pub in_point: u32,
    /// The frame after the last one in the cut.
    #[serde(rename = "out")]
    pub out_point: u32,
}

impl Clip {
    /// Frames in the cut.
    pub fn length(&self) -> u32 {
        self.out_point - self.in_point
    }

    /// The storyboard frame after the clip's last.
    pub fn end(&self) -> u32 {
        self.start + self.length()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransitionKind {
    CrossDissolve,
    DipToBlack,
    DipToWhite,
}

/// Which end of a clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Start,
    End,
}

/// A transition on one edge of a shot's clip. Where another clip meets that edge it lies across
/// the cut, centred on it, and shows each shot's frames beyond its in or out point; on a free
/// edge it is a fade inside the clip. A cut's transition is kept on the outgoing clip's end; if
/// clips are moved so that both edges at a cut carry one, the outgoing clip's is the cut's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transition {
    pub shot: ShotId,
    pub edge: Edge,
    pub kind: TransitionKind,
    /// In frames.
    pub length: u32,
}

impl TransitionKind {
    /// How a picture is mixed `at` of the way through the transition, from 0 at its start to
    /// 1 at its end: how much of the outgoing picture, how much of the incoming, and how much
    /// of a flat colour, with that colour. The three add up to 1.
    pub fn mix(self, at: f64) -> (f64, f64, f64, [f64; 3]) {
        let at = at.clamp(0.0, 1.0);
        let dip = |colour: [f64; 3]| if at < 0.5 { (1.0 - 2.0 * at, 0.0, 2.0 * at, colour) } else { (0.0, 2.0 * at - 1.0, 2.0 - 2.0 * at, colour) };
        match self {
            TransitionKind::CrossDissolve => (1.0 - at, at, 0.0, [0.0; 3]),
            TransitionKind::DipToBlack => dip([0.0; 3]),
            TransitionKind::DipToWhite => dip([1.0; 3]),
        }
    }
}

/// A transition as it lies on the film.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lying {
    pub transition: Transition,
    /// The film's frames it is over: the first, and the one after the last.
    pub start: u32,
    pub end: u32,
    /// The clip it leads out of and the clip it leads into. On a free edge one of them is
    /// missing, and the film is black there.
    pub outgoing: Option<Clip>,
    pub incoming: Option<Clip>,
}

/// What the film shows on a frame inside a transition: a frame of the outgoing shot, one of
/// the incoming, and how far through the transition it is, from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mixed {
    pub kind: TransitionKind,
    pub outgoing: Option<(ShotId, u32)>,
    pub incoming: Option<(ShotId, u32)>,
    pub at: f64,
}

/// A clip can't go where it was put: it would run into this shot's clip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overlap(pub ShotId);

/// One shot track with free placement: clips can sit anywhere and leave gaps, but never
/// overlap, and a shot has at most one clip. The clips are kept in film order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Storyboard {
    pub id: StoryboardId,
    pub name: String,
    clips: Vec<Clip>,
    pub transitions: Vec<Transition>,
    /// The number its first frame has when times are told as timecode: 0 unless the film is
    /// to start at another time, as a reel that starts at one hour does.
    #[serde(default)]
    pub start: u32,
    /// The bin the storyboard is in. `None` is Master.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<BinId>,
    /// The sound tracks under the shots (plan §7.2): the film's dialogue and music.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sound: Vec<SoundTrack>,
    /// Marks on the film's frames, in their order, one a frame: a beat, a line, a note to self.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// The stretch of the film that is played, and gone round when it loops: its first frame
    /// and the one after its last. Without one, all of the film is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<(u32, u32)>,
}

/// A mark on a frame of a storyboard's film.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub frame: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Its colour, as sRGB.
    pub colour: [u8; 3],
}

/// The colours a marker is given from, with their names; the first is a new marker's.
pub const MARKER_COLOURS: [(&str, [u8; 3]); 8] = [
    ("Blue", [66, 133, 244]),
    ("Red", [219, 68, 55]),
    ("Orange", [244, 160, 0]),
    ("Yellow", [240, 214, 60]),
    ("Green", [15, 157, 88]),
    ("Teal", [0, 172, 193]),
    ("Purple", [171, 71, 188]),
    ("Grey", [158, 158, 158]),
];

impl Storyboard {
    pub fn new(id: StoryboardId, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            clips: Vec::new(),
            transitions: Vec::new(),
            start: 0,
            bin: None,
            sound: Vec::new(),
            markers: Vec::new(),
            range: None,
        }
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    pub fn clip(&self, shot: ShotId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.shot == shot)
    }

    /// The frame after the last clip's last.
    pub fn end(&self) -> u32 {
        self.clips.last().map_or(0, Clip::end)
    }

    pub fn transition(&self, shot: ShotId, edge: Edge) -> Option<&Transition> {
        self.transitions.iter().find(|t| t.shot == shot && t.edge == edge)
    }

    /// The transitions as they lie on the film, in its order. One across a cut is centred on
    /// it and shows each shot beyond its out or in point; one on a free edge is a fade inside
    /// the clip. Where both edges at a cut carry one, the outgoing clip's is the cut's.
    pub fn lying(&self) -> Vec<Lying> {
        let mut lying = Vec::new();
        for (at, clip) in self.clips.iter().enumerate() {
            let before = at.checked_sub(1).map(|at| self.clips[at]).filter(|before| before.end() == clip.start);
            let after = self.clips.get(at + 1).copied().filter(|after| after.start == clip.end());
            // Leading in: only if the clip before, where one touches, has none leading out.
            if let Some(transition) =
                self.transition(clip.shot, Edge::Start).filter(|_| before.is_none_or(|before| self.transition(before.shot, Edge::End).is_none()))
            {
                lying.push(match before {
                    Some(before) => across(*transition, before, *clip),
                    None => Lying {
                        transition: *transition,
                        start: clip.start,
                        end: clip.start + transition.length.min(clip.length()),
                        outgoing: None,
                        incoming: Some(*clip),
                    },
                });
            }
            if let Some(transition) = self.transition(clip.shot, Edge::End) {
                lying.push(match after {
                    Some(after) => across(*transition, *clip, after),
                    None => Lying {
                        transition: *transition,
                        start: clip.end() - transition.length.min(clip.length()),
                        end: clip.end(),
                        outgoing: Some(*clip),
                        incoming: None,
                    },
                });
            }
        }
        lying
    }

    /// What the film shows on `frame`, if it is inside a transition.
    pub fn mixed_at(&self, frame: u32) -> Option<Mixed> {
        let lying = self.lying().into_iter().find(|lying| (lying.start..lying.end).contains(&frame))?;
        // A frame of a clip's shot at a frame of the film, also past the clip's ends.
        let of = |clip: Clip| (clip.shot, (i64::from(clip.in_point) + i64::from(frame) - i64::from(clip.start)).max(0) as u32);
        let at = (f64::from(frame - lying.start) + 0.5) / f64::from((lying.end - lying.start).max(1));
        Some(Mixed { kind: lying.transition.kind, outgoing: lying.outgoing.map(of), incoming: lying.incoming.map(of), at })
    }

    /// Place `clip`, replacing the clip its shot had here. A clip can't start inside another.
    /// If it would run into the next clip, `ripple` moves that clip and every later one along
    /// by what is needed; without it the clip is refused. On refusal nothing changes.
    pub fn put(&mut self, clip: Clip, ripple: bool) -> Result<(), Overlap> {
        let mut clips: Vec<Clip> = self.clips.iter().copied().filter(|c| c.shot != clip.shot).collect();
        let at = clips.partition_point(|c| c.start < clip.start);
        if let Some(before) = clips[..at].last().filter(|b| b.end() > clip.start) {
            return Err(Overlap(before.shot));
        }
        if let Some(next) = clips.get(at).filter(|n| n.start < clip.end()) {
            if !ripple {
                return Err(Overlap(next.shot));
            }
            let by = clip.end() - next.start;
            for later in &mut clips[at..] {
                later.start = later.start.saturating_add(by);
            }
        }
        clips.insert(at, clip);
        self.clips = clips;
        Ok(())
    }

    /// Put a marker on the film, in place of the one on its frame, if one is there.
    pub fn put_marker(&mut self, marker: Marker) {
        self.markers.retain(|other| other.frame != marker.frame);
        let at = self.markers.partition_point(|other| other.frame < marker.frame);
        self.markers.insert(at, marker);
    }

    /// The stretch that plays: the range that was set, as far as the film reaches, or all of
    /// the film. Its first frame and the one after its last.
    pub fn played(&self) -> (u32, u32) {
        let end = self.end().max(1);
        match self.range {
            Some((first, after)) if first < end => (first, after.min(end).max(first + 1)),
            _ => (0, end),
        }
    }

    /// Take a shot's clip off the storyboard, with the transitions on it.
    pub fn take(&mut self, shot: ShotId) -> Option<Clip> {
        let at = self.clips.iter().position(|c| c.shot == shot)?;
        self.transitions.retain(|t| t.shot != shot);
        Some(self.clips.remove(at))
    }
}

/// A transition across the cut between two clips that touch: centred on the cut, and not
/// before the film's first frame.
fn across(transition: Transition, outgoing: Clip, incoming: Clip) -> Lying {
    let start = incoming.start.saturating_sub(transition.length / 2);
    Lying { transition, start, end: start + transition.length, outgoing: Some(outgoing), incoming: Some(incoming) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(shot: u64, start: u32, length: u32) -> Clip {
        Clip { shot: ShotId(shot), start, in_point: 0, out_point: length }
    }

    fn placed(s: &Storyboard) -> Vec<(u64, u32, u32)> {
        s.clips().iter().map(|c| (c.shot.0, c.start, c.end())).collect()
    }

    fn board() -> Storyboard {
        let mut s = Storyboard::new(StoryboardId(100), "Storyboard");
        s.put(clip(2, 24, 24), false).unwrap();
        s.put(clip(1, 0, 24), false).unwrap();
        s.put(clip(3, 60, 12), false).unwrap();
        s
    }

    #[test]
    fn clips_are_kept_in_film_order_and_may_leave_gaps() {
        let s = board();
        assert_eq!(placed(&s), [(1, 0, 24), (2, 24, 48), (3, 60, 72)]);
        assert_eq!(s.end(), 72);
    }

    #[test]
    fn a_clip_that_would_overlap_is_refused_and_nothing_changes() {
        let mut s = board();
        let before = s.clone();
        assert_eq!(s.put(clip(4, 50, 20), false), Err(Overlap(ShotId(3))), "it would run into the next clip");
        assert_eq!(s.put(clip(4, 30, 5), false), Err(Overlap(ShotId(2))), "it would start inside a clip");
        assert_eq!(s.put(clip(4, 30, 5), true), Err(Overlap(ShotId(2))), "also with ripple");
        assert_eq!(s, before);
        s.put(clip(4, 48, 12), false).unwrap();
        assert_eq!(placed(&s)[2], (4, 48, 60), "a gap of exactly its length takes it");
    }

    #[test]
    fn ripple_moves_the_later_clips_along_to_make_room() {
        let mut s = board();
        s.put(clip(4, 48, 20), true).unwrap();
        assert_eq!(placed(&s), [(1, 0, 24), (2, 24, 48), (4, 48, 68), (3, 68, 80)]);
        // Lengthening a clip in the cut is putting it again.
        s.put(clip(1, 0, 30), true).unwrap();
        assert_eq!(placed(&s), [(1, 0, 30), (2, 30, 54), (4, 54, 74), (3, 74, 86)], "every later clip moves by the same six frames");
    }

    #[test]
    fn moving_a_clip_is_putting_it_again() {
        let mut s = board();
        s.put(Clip { start: 100, ..*s.clip(ShotId(1)).unwrap() }, false).unwrap();
        assert_eq!(placed(&s), [(2, 24, 48), (3, 60, 72), (1, 100, 124)]);
        // A clip doesn't get in its own way.
        s.put(Clip { start: 30, ..*s.clip(ShotId(2)).unwrap() }, false).unwrap();
        assert_eq!(placed(&s)[0], (2, 30, 54));
    }

    #[test]
    fn taking_a_clip_takes_its_transitions() {
        let mut s = board();
        for shot in [1, 2] {
            s.transitions.push(Transition { shot: ShotId(shot), edge: Edge::End, kind: TransitionKind::CrossDissolve, length: 12 });
        }
        assert_eq!(s.take(ShotId(1)).map(|c| c.start), Some(0));
        assert_eq!(s.take(ShotId(1)), None);
        assert!(s.transition(ShotId(1), Edge::End).is_none() && s.transition(ShotId(2), Edge::End).is_some());
    }

    #[test]
    fn a_transition_lies_across_its_cut_or_inside_a_free_edge_and_mixes_the_two_shots() {
        // Two shots that touch at frame 24, each with handles, and a third after a gap.
        let mut s = Storyboard::new(StoryboardId(100), "Storyboard");
        s.put(Clip { shot: ShotId(1), start: 0, in_point: 0, out_point: 24 }, false).unwrap();
        s.put(Clip { shot: ShotId(2), start: 24, in_point: 10, out_point: 34 }, false).unwrap();
        s.put(Clip { shot: ShotId(3), start: 60, in_point: 0, out_point: 12 }, false).unwrap();
        let on = |shot: u64, edge, kind, length| Transition { shot: ShotId(shot), edge, kind, length };
        s.transitions = vec![
            on(1, Edge::End, TransitionKind::CrossDissolve, 8),
            on(3, Edge::Start, TransitionKind::DipToBlack, 4),
            on(3, Edge::End, TransitionKind::DipToWhite, 6),
        ];
        let spans: Vec<(u32, u32, bool, bool)> =
            s.lying().iter().map(|lying| (lying.start, lying.end, lying.outgoing.is_some(), lying.incoming.is_some())).collect();
        assert_eq!(spans, [(20, 28, true, true), (60, 64, false, true), (66, 72, true, false)]);
        // Across the cut each shot is seen beyond its out or in point, half way at the cut.
        let before = s.mixed_at(20).unwrap();
        assert_eq!((before.outgoing, before.incoming, before.at), (Some((ShotId(1), 20)), Some((ShotId(2), 6)), 0.0625));
        let after = s.mixed_at(27).unwrap();
        assert_eq!((after.outgoing, after.incoming, after.at), (Some((ShotId(1), 27)), Some((ShotId(2), 13)), 0.9375));
        assert!(s.mixed_at(19).is_none() && s.mixed_at(28).is_none() && s.mixed_at(64).is_none());
        // On a free edge there is one shot, and the other side is black.
        assert_eq!(s.mixed_at(60).map(|mixed| (mixed.outgoing, mixed.incoming)), Some((None, Some((ShotId(3), 0)))));
        assert_eq!(s.mixed_at(71).map(|mixed| (mixed.outgoing, mixed.incoming)), Some((Some((ShotId(3), 11)), None)));
        // A transition leading into a clip is the cut's only where the clip before has none.
        s.transitions.push(on(2, Edge::Start, TransitionKind::DipToBlack, 2));
        assert_eq!(s.lying().len(), 3);
        s.transitions.remove(0);
        assert_eq!(s.lying()[0].start..s.lying()[0].end, 23..25);
        // A dissolve goes straight from one to the other; a dip goes by way of its colour.
        assert_eq!(TransitionKind::CrossDissolve.mix(0.25), (0.75, 0.25, 0.0, [0.0; 3]));
        assert_eq!((TransitionKind::DipToWhite.mix(0.25), TransitionKind::DipToWhite.mix(0.75)), ((0.5, 0.0, 0.5, [1.0; 3]), (0.0, 0.5, 0.5, [1.0; 3])));
    }

    #[test]
    fn in_and_out_points_pick_the_part_in_the_cut() {
        let c = Clip { shot: ShotId(1), start: 10, in_point: 24, out_point: 96 };
        assert_eq!((c.length(), c.end()), (72, 82));
        assert_eq!(serde_json::to_value(c).unwrap(), serde_json::json!({ "shot": 1, "start": 10, "in": 24, "out": 96 }));
    }
}
