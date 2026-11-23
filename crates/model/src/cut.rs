//! Cutting shots on a storyboard (plan §4.7): one shot split in two where the playhead is, and
//! two shots that touch merged into one.
//!
//! A split deletes nothing: both halves are the whole shot, each with the other's part of the
//! cut as its handles, as a clip cut in two in a film editor is. A merge makes one document of
//! two: the second shot's layers, pegs and deformers come into the first, later by as much as
//! the first is long in the cut.

use std::sync::Arc;

use crate::{Clip, Edge, Panel, PegId, Project, Shot, ShotId, StoryboardId, Transition};

/// Why two shots can't be merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoMerge {
    /// One of them is not on a storyboard, or they are on two.
    NotTogether,
    /// The second does not start on the frame the first ends on.
    Apart,
}

impl Project {
    /// Split `shot` in two at `frame` of it, which is in the cut and not its first frame
    /// there: the shot keeps the part before it, and a copy named `name`, with fresh ids
    /// throughout, has the part from it on, right after on the storyboard. A transition on the
    /// shot's end is the copy's. `None` if the shot is on no storyboard or the frame is not
    /// inside its cut. Returns the copy's id.
    pub fn split_shot(&mut self, shot: ShotId, frame: u32, name: &str) -> Option<ShotId> {
        let home = self.storyboard_of(shot)?;
        let (board, clip) = (home.id, *home.clip(shot)?);
        if frame <= clip.in_point || frame >= clip.out_point {
            return None;
        }
        let copy = self.duplicate_shot(shot, name)?;
        let made = copy.id;
        self.shots.push(Arc::new(copy));
        let board = self.storyboard_mut(board)?;
        for transition in board.transitions.iter_mut().filter(|transition| transition.shot == shot && transition.edge == Edge::End) {
            transition.shot = made;
        }
        // Shorter in its place, and the copy in the room that made: neither can be in the way.
        let _ = board.put(Clip { out_point: frame, ..clip }, false);
        let _ = board.put(Clip { shot: made, start: clip.start + (frame - clip.in_point), in_point: frame, out_point: clip.out_point }, false);
        Some(made)
    }

    /// Merge `second` into `first`: two shots that touch on a storyboard, the first ending
    /// where the second starts. The first shot is then as long in the cut as both were, with
    /// the second's nodes in it, what they show and how they move later by as much as the
    /// first was long; the second shot is gone. The first shot's frames past its out point
    /// show nothing any more, and neither do the second's before its in point. Where the
    /// second has more frames before its in point than the first has up to its out point,
    /// it is the first that goes later, by as many more, and its in point with it. The merged
    /// shot has the first's camera, which from the join on is where the second's was and
    /// moves as it did ([`Shot::camera_goes_on_as`]). Both shots have to be loaded.
    pub fn merge_shots(&mut self, first: ShotId, second: ShotId) -> Result<(), NoMerge> {
        let home = self.storyboard_of(first).filter(|home| home.clip(second).is_some()).ok_or(NoMerge::NotTogether)?;
        let (board, a, b) = (home.id, *home.clip(first).ok_or(NoMerge::NotTogether)?, *home.clip(second).ok_or(NoMerge::NotTogether)?);
        if a.end() != b.start {
            return Err(NoMerge::Apart);
        }
        // The second's in point goes right after the first's out point: the second goes later
        // by `by`, or where it has more frames before its in point, the first by `early`.
        let (early, by) = match a.out_point.checked_sub(b.in_point) {
            Some(by) => (0, by),
            None => (b.in_point - a.out_point, 0),
        };
        // The frame of the merged shot that the join is on.
        let join = a.out_point + early;
        let mut later = self.shot(second).ok_or(NoMerge::NotTogether)?.clone();
        later.shift_later(by);
        for layer in &mut later.layers {
            let layer = Arc::make_mut(layer);
            let shown = layer.shown_drawings();
            layer.exposure.set(0, join, None);
            layer.drop_unshown(&shown);
        }
        let camera_peg = PegId(self.new_id());
        let panels = later.panels();
        // The second's panels from its in point on: the one it is in there starts the join.
        let joined = Panel { frame: join, ..later.panel_at(b.in_point + by) };
        let after = panels.into_iter().filter(|panel| panel.frame > b.in_point + by);

        if let Some(into) = self.shot_mut(first) {
            into.shift_later(early);
            for layer in &mut into.layers {
                let layer = Arc::make_mut(layer);
                let (end, shown) = (layer.exposure.runs().last().map_or(0, |run| run.end()), layer.shown_drawings());
                layer.exposure.set(join, end.saturating_sub(join), None);
                layer.drop_unshown(&shown);
            }
            into.panels.retain(|panel| panel.frame < join);
            into.panels.push(joined);
            into.panels.extend(after);
            into.duration = into.duration.max(later.duration);
            into.camera_goes_on_as(&mut later, join, camera_peg);
        }
        self.place_nodes(first, later, None, None, true);
        self.shots.retain(|shot| shot.id != second);
        if let Some(board) = self.storyboard_mut(board) {
            // What was on the cut between them goes; what was on the second's end is the first's.
            let end = board.transition(second, Edge::End).copied();
            board.transitions.retain(|transition| (transition.shot, transition.edge) != (first, Edge::End));
            board.take(second);
            board.transitions.extend(end.map(|transition| Transition { shot: first, ..transition }));
            let _ = board.put(Clip { in_point: a.in_point + early, out_point: join + b.length(), ..a }, false);
        }
        Ok(())
    }
}

impl crate::Layer {
    /// The drawings its cells show.
    fn shown_drawings(&self) -> std::collections::BTreeSet<crate::DrawingId> {
        self.exposure.runs().iter().map(|run| run.drawing).collect()
    }

    /// Take out of the pool the drawings of `shown`, which the cells showed before, that they
    /// show no more: those a merge left in no cell. A drawing that was in no cell before, kept
    /// to be put in one, stays.
    fn drop_unshown(&mut self, shown: &std::collections::BTreeSet<crate::DrawingId>) {
        let now = self.shown_drawings();
        self.drawings.retain(|entry| !shown.contains(&entry.id) || now.contains(&entry.id));
        // And their variants in skins with them.
        let kept: std::collections::BTreeSet<crate::DrawingId> = self.drawings.iter().map(|entry| entry.id).collect();
        self.variants.retain(|variant| kept.contains(&variant.of));
    }
}

impl Shot {
    /// Make everything in the shot happen `by` frames later, the shot as much longer: its
    /// exposures, with empty cells before them, every key, its panels and markers, the
    /// stretch that plays.
    pub fn shift_later(&mut self, by: u32) {
        if by == 0 {
            return;
        }
        for layer in &mut self.layers {
            let layer = Arc::make_mut(layer);
            layer.exposure.insert(0, by);
            layer.exposure.set(0, by, None);
        }
        for peg in &mut self.pegs {
            let peg = Arc::make_mut(peg);
            peg.animation = peg.animation.shifted(by);
        }
        for deformer in &mut self.deformers {
            *deformer = Arc::new(deformer.shifted(by));
        }
        for effect in &mut self.effects {
            *effect = Arc::new(effect.shifted(by));
        }
        self.camera.animation.zoom = self.camera.animation.zoom.shifted(by);
        self.camera.animation.distance = self.camera.animation.distance.shifted(by);
        // The first panel, implied on frame 0, is written out where it now starts.
        let panels = self.panels();
        self.panels = panels.into_iter().map(|panel| Panel { frame: panel.frame + by, ..panel }).collect();
        for marker in &mut self.markers {
            marker.frame += by;
        }
        self.range = self.range.map(|(from, to)| (from + by, to + by));
        self.duration += by;
    }

    /// From `cut` on, have the camera see as the camera of `later` does, another shot whose
    /// nodes are about to come into this one: up to the frame before `cut` it is as it was,
    /// and on `cut` it jumps to where the other is. The lens goes on as the other's, and the
    /// peg that moves the camera as the other's peg, by keys: a held one on the frame before
    /// `cut`, so a move that crossed it ends there, a little changed if it eased. A camera
    /// that had no peg gets one, `fresh`. The other's peg, with nothing else to carry, is
    /// taken out of `later`: it would come over as a peg that moves nothing.
    ///
    /// Only where each camera is moved by one peg at most, with no peg above it, and both
    /// pegs turn about the same pivot. Otherwise the camera is left as it was, and `false`
    /// is returned.
    pub fn camera_goes_on_as(&mut self, later: &mut Shot, cut: u32, fresh: PegId) -> bool {
        let alone = |shot: &Shot| shot.camera.parent.is_none_or(|peg| shot.peg(peg).is_some_and(|peg| peg.parent.is_none()));
        let theirs = later.camera.parent.and_then(|peg| later.peg(peg)).cloned();
        let pivot = |shot: &Shot| shot.camera.parent.and_then(|peg| shot.peg(peg)).map(|peg| peg.transform.pivot);
        if !alone(self) || !alone(later) || pivot(self).zip(pivot(later)).is_some_and(|(mine, theirs)| mine != theirs) {
            return false;
        }
        let before = cut.saturating_sub(1);
        let (zoom, distance) = (self.camera.zoom_at(before), self.camera.distance_at(before));
        let lens = &mut self.camera.animation;
        lens.zoom.go_on_as(cut, zoom, &later.camera.animation.zoom, later.camera.zoom_at(cut));
        lens.distance.go_on_as(cut, distance, &later.camera.animation.distance, later.camera.distance_at(cut));
        // The other's peg, or one at home in its place; and a peg of this camera's to go on as it.
        let theirs = theirs.unwrap_or_else(|| crate::Peg {
            id: fresh,
            name: String::new(),
            parent: None,
            transform: crate::Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        });
        let moved = !theirs.animation.key_frames().is_empty() || theirs.transform != crate::Transform::default();
        if self.camera.parent.is_none() && moved {
            self.add_camera_peg(fresh);
            if let Some(peg) = self.pegs.iter_mut().find(|peg| peg.id == fresh) {
                Arc::make_mut(peg).transform.pivot = theirs.transform.pivot;
            }
        }
        let own = self.camera.parent;
        if let Some(peg) = self.pegs.iter_mut().find(|peg| Some(peg.id) == own).map(Arc::make_mut) {
            let (was, then) = (peg.transform_at(before), theirs.transform_at(cut));
            let (mine, other) = (&mut peg.animation, &theirs.animation);
            mine.position.path_goes_on_as(cut, was.position, &other.position, then.position);
            mine.rotation.go_on_as(cut, was.rotation, &other.rotation, then.rotation);
            mine.scale.go_on_as(cut, was.scale, &other.scale, then.scale);
            mine.skew.go_on_as(cut, was.skew, &other.skew, then.skew);
            mine.depth.go_on_as(cut, was.depth, &other.depth, then.depth);
            mine.tilt.go_on_as(cut, was.tilt, &other.tilt, then.tilt);
        }
        // Their peg has done its work, unless it carries something of theirs too.
        if let Some(peg) = later.camera.parent {
            let carries = later.layers.iter().any(|layer| layer.parent == Some(peg))
                || later.pegs.iter().any(|other| other.parent == Some(peg))
                || later.effects.iter().any(|effect| effect.parent == Some(peg));
            if !carries && !later.groups.iter().any(|group| group.members.contains(&peg.0)) {
                later.pegs.retain(|other| other.id != peg);
                later.places.remove(&peg.0);
                later.colours.remove(&peg.0);
                later.hidden.remove(&peg.0);
                later.camera.parent = None;
            }
        }
        true
    }
}

impl Project {
    /// What is spoken over each panel of a storyboard's shots (plan §4.7): for every panel's
    /// stretch in the cut that something is spoken over, its shot, the frame the panel starts
    /// on, and the words, in the order they are heard. A word is of the panel it starts in.
    /// The words are those of the transcripts of the sounds that lie under the shots on
    /// tracks that are not muted, as far as each clip plays them.
    pub fn spoken_over(&self, storyboard: StoryboardId) -> Vec<(ShotId, u32, String)> {
        let Some(board) = self.storyboard(storyboard) else { return Vec::new() };
        let rate = self.settings.frame_rate;
        // A thousandth of a second into a sound, as a frame into it.
        let frame_of = |ms: u32| (u64::from(ms) * rate.num.max(1) as u64 / (1000 * rate.den.max(1) as u64)) as u32;
        // Every word that is heard, with the frame of the film it starts on.
        let mut heard: Vec<(u32, &str)> = Vec::new();
        for track in board.sound.iter().filter(|track| !track.mute) {
            for clip in &track.clips {
                let Some(media) = self.media(clip.media) else { continue };
                let words = media.transcript.iter().map(|word| (frame_of(word.start), word.text.as_str()));
                let played = words.filter(|(at, _)| (clip.offset..clip.offset + clip.length).contains(at));
                heard.extend(played.map(|(at, text)| (clip.start + at - clip.offset, text)));
            }
        }
        heard.sort_by_key(|(at, _)| *at);
        let mut spoken = Vec::new();
        for clip in board.clips() {
            let Some(shot) = self.shot(clip.shot) else { continue };
            for panel in shot.panels() {
                // The panel's stretch in the cut, in frames of the film.
                let (start, end) = (panel.frame.max(clip.in_point), shot.panel_end(panel.frame).min(clip.out_point));
                if start >= end {
                    continue;
                }
                let film = clip.start + start - clip.in_point..clip.start + end - clip.in_point;
                let words: Vec<&str> = heard.iter().filter(|(at, _)| film.contains(at)).map(|(_, text)| *text).collect();
                if !words.is_empty() {
                    spoken.push((shot.id, panel.frame, words.join(" ")));
                }
            }
        }
        spoken
    }
}

#[cfg(test)]
mod camera_tests {
    use super::*;
    use crate::Interp;

    #[test]
    fn a_camera_goes_on_from_a_cut_as_another_shots_camera_does() {
        let (mut first, mut later) = (Project::starting().shots[0].as_ref().clone(), Project::starting().shots[0].as_ref().clone());
        fn peg_of(shot: &mut Shot) -> &mut crate::Peg {
            let peg = shot.camera.parent.unwrap();
            Arc::make_mut(shot.pegs.iter_mut().find(|other| other.id == peg).unwrap())
        }
        // The first pans from 0 to 100 over frames 0 to 20; the second, already 10 frames
        // later as a merge makes it, stands at 500 and pushes in from frame 14 to 24.
        let mine = peg_of(&mut first);
        mine.animation.position.set(0, [0.0, 0.0]);
        mine.animation.position.set(20, [100.0, 0.0]);
        let theirs = peg_of(&mut later);
        theirs.transform.position = [500.0, 40.0];
        theirs.animation.depth.set(14, 0.0);
        theirs.animation.depth.set(24, 300.0);
        later.camera.zoom = 2.0;
        let (was, then) = (first.clone(), later.clone());
        assert!(first.camera_goes_on_as(&mut later, 10, PegId(9000)));
        // Before the cut the camera sees as it did, and from the cut on as the other's does.
        let same = |a: Option<crate::Affine>, b: Option<crate::Affine>| a.zip(b).is_some_and(|(a, b)| a.m.iter().zip(b.m).all(|(a, b)| (a - b).abs() < 0.01));
        for frame in 0..30 {
            let as_it_was = if frame < 10 { was.seen(0.0, frame) } else { then.seen(0.0, frame) };
            assert!(same(first.seen(0.0, frame), as_it_was), "frame {frame}: {:?} {as_it_was:?}", first.seen(0.0, frame));
        }
        let peg = peg_of(&mut first).clone();
        assert_eq!(peg.animation.position.keys().iter().map(|key| (key.frame, key.out)).collect::<Vec<_>>()[1..], [(9, Interp::Hold), (10, Interp::Linear)]);
        assert!(peg.animation.rotation.is_empty() && peg.animation.scale.is_empty(), "what is the same in both gets no key");
        // The other's peg carried nothing else, and does not come over.
        assert!(later.pegs.is_empty() && later.camera.parent.is_none());

        // Two cameras at home change nothing; one under a peg that is under another is left alone.
        let (mut plain, mut other) = (was.clone(), Project::starting().shots[0].as_ref().clone());
        peg_of(&mut plain).animation = Default::default();
        let unmoved = plain.clone();
        assert!(plain.camera_goes_on_as(&mut other, 10, PegId(9001)) && plain == unmoved);
        let mut deep = was.clone();
        deep.add_camera_peg(PegId(9002));
        let untouched = deep.clone();
        assert!(!deep.camera_goes_on_as(&mut then.clone(), 10, PegId(9003)) && deep == untouched);
    }
}
