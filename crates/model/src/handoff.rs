//! Handing shots out and taking them back (plan §7.2a). A shot goes out as a project of its
//! own, which holds that one shot; whoever has it works on it as on any project. When it comes
//! back it takes the place of the shot it came from.
//!
//! The project wins: the returned shot is fitted to the project as it is now, and what doesn't
//! match is said, not silently changed.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Clip, Layer, Palette, Peg, Project, Shot, ShotId, Storyboard, StoryboardId};

/// Where a shot is in its life. Nothing follows from it but what is shown, except that a shot
/// that is out is expected back: what is done to it here meanwhile is lost then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    #[default]
    Board,
    /// With someone, as a file of its own.
    Out,
    Animation,
    /// Backgrounds and effects.
    Post,
    Final,
}

impl Status {
    pub const ALL: [Status; 5] = [Status::Board, Status::Out, Status::Animation, Status::Post, Status::Final];

    pub fn label(self) -> &'static str {
        match self {
            Status::Board => "Board",
            Status::Out => "Out",
            Status::Animation => "Animation",
            Status::Post => "Post",
            Status::Final => "Final",
        }
    }
}

/// What a shot file says about where it came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HandOff {
    /// The name of the project the shot is of.
    pub project: String,
    /// The shot's id in that project, and its name there when it was sent.
    pub shot: u64,
    pub name: String,
    /// When it was sent, in seconds since the start of 1970 (UTC).
    pub sent: u64,
    /// Who it was sent to.
    pub to: String,
}

/// "1 drawing", "3 drawings".
fn count(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// What differs between two versions of a layer, in words; nothing if they are alike.
fn layer_differences(was: &Layer, is: &Layer) -> Option<String> {
    let name = |layer: &Layer, id| layer.drawing(id).map(|entry| entry.name.clone());
    let names = |layer: &Layer| layer.drawings.iter().map(|entry| entry.name.clone()).collect::<BTreeSet<_>>();
    let (before, after) = (names(was), names(is));
    let changed = is.drawings.iter().filter(|entry| was.drawings.iter().any(|old| old.name == entry.name && old.drawing != entry.drawing)).count();
    let mut said = Vec::new();
    for (n, what) in [(after.difference(&before).count(), "new"), (changed, "changed"), (before.difference(&after).count(), "gone")] {
        if n > 0 {
            said.push(format!("{} {what}", count(n, "drawing")));
        }
    }
    // The timing, by the drawings' names: their ids are not the same in the two.
    let timing = |layer: &Layer| layer.exposure.runs().iter().map(|run| (run.start, run.len, name(layer, run.drawing))).collect::<Vec<_>>();
    if timing(was) != timing(is) {
        said.push("the timing changed".to_string());
    }
    if was.density != is.density {
        said.push(format!("its resolution is {}× where it was {}×", is.density, was.density));
    }
    (!said.is_empty()).then(|| format!("The layer “{}”: {}.", is.name, said.join(", ")))
}

impl Project {
    /// A shot as a project of its own, to hand to someone: the shot as it is, its drawings
    /// shared with this project's, with the project's settings and its whole palette. Its
    /// storyboard has the shot's clip, so the part that is in the cut shows there, and under
    /// it the shot's sound: the stretch of this project's sound tracks that lies under the
    /// shot, cut to it, with the sound items it is of (plan §7.2, R21). Ids go on from this
    /// project's.
    pub fn shot_file(&self, shot: ShotId) -> Option<Project> {
        let from = self.shots.iter().find(|s| s.id == shot)?;
        let mut file = Project {
            settings: self.settings.clone(),
            palette: self.palette.clone(),
            shots: Vec::new(),
            storyboards: Vec::new(),
            bins: Vec::new(),
            media: Vec::new(),
            renders: Vec::new(),
            look: 0,
            next_id: self.next_id,
        };
        let mut storyboard = Storyboard::new(StoryboardId(file.new_id()), "Storyboard");
        let cut = self.storyboard_of(shot).and_then(|home| home.clip(shot)).map_or((0, from.duration), |clip| (clip.in_point, clip.out_point));
        let _ = storyboard.put(Clip { shot, start: 0, in_point: cut.0, out_point: cut.1 }, false);
        // The shot hears what lies under it: there, from the file's first frame on.
        if let Some((home, clip)) = self.storyboard_of(shot).and_then(|home| Some((home, *home.clip(shot)?))) {
            for (track, sound) in home.sound_between(clip.start, clip.end()) {
                // Each track of the project that is heard is a track there, under its name.
                let name = &home.sound[track].name;
                let on = storyboard.sound.iter().position(|of| &of.name == name).unwrap_or_else(|| {
                    storyboard.sound.push(crate::SoundTrack { name: name.clone(), mute: home.sound[track].mute, clips: Vec::new() });
                    storyboard.sound.len() - 1
                });
                storyboard.sound[on].clips.push(crate::SoundClip { start: sound.start - clip.start, ..sound });
                if let Some(media) = self.media.iter().find(|media| media.id == sound.media).filter(|media| file.media(media.id).is_none()) {
                    file.media.push(Arc::new(crate::Media { bin: None, ..crate::Media::clone(media) }));
                }
            }
        }
        // There it is a shot being animated, with no one else.
        let mut out = from.clone();
        if (out.status, &out.out_to) != (Status::Animation, &None) {
            let shot = Arc::make_mut(&mut out);
            (shot.status, shot.out_to) = (Status::Animation, None);
        }
        // The file has no bins: there the shot is in Master.
        if out.bin.is_some() {
            Arc::make_mut(&mut out).bin = None;
        }
        file.shots.push(out);
        file.storyboards.push(Arc::new(storyboard));
        Some(file)
    }

    /// What bringing `returned` back as the shot `shot` would change, and what of it doesn't
    /// match the project, one line each. `returned` is a project whose first shot is the one
    /// that comes back. Both versions of the shot have to be loaded: drawings are compared by
    /// what is painted. `None` if either has no such shot.
    pub fn differences(&self, shot: ShotId, returned: &Project) -> Option<Vec<String>> {
        let (was, is) = (self.shot(shot)?, &**returned.shots.first()?);
        let mut said = Vec::new();
        if was.duration != is.duration {
            said.push(format!("It is {} long, where it was {}.", count(is.duration as usize, "frame"), count(was.duration as usize, "frame")));
            if let Some(clip) = self.storyboard_of(shot).and_then(|home| home.clip(shot)).filter(|clip| clip.out_point > is.duration) {
                said.push(format!("It no longer reaches its out point, frame {}: the cut is left as it is, and needs looking at.", clip.out_point));
            }
        }
        let named = |shot: &Shot, name: &str| shot.layers.iter().any(|layer| layer.name == name);
        for layer in &is.layers {
            match was.layers.iter().find(|old| old.name == layer.name) {
                Some(old) => said.extend(layer_differences(old, layer)),
                None => said.push(format!("A new layer, “{}”, with {}.", layer.name, count(layer.drawings.len(), "drawing"))),
            }
        }
        said.extend(
            was.layers
                .iter()
                .filter(|old| !named(is, &old.name))
                .map(|old| format!("The layer “{}” is gone, with {}.", old.name, count(old.drawings.len(), "drawing"))),
        );
        let peg = |shot: &'_ Shot, name: &str| shot.pegs.iter().find(|peg| peg.name == name).cloned();
        // A peg as it is, whatever its ids: with its parent's name.
        let pose = |shot: &Shot, peg: &Peg| (peg.transform, peg.animation.clone(), shot.ancestors(peg.parent).next().map(|above| above.name.clone()));
        for now in &is.pegs {
            match peg(was, &now.name) {
                Some(old) if pose(was, &old) != pose(is, now) => said.push(format!("The peg “{}” is placed, animated or parented differently.", now.name)),
                Some(_) => {}
                None => said.push(format!("A new peg, “{}”.", now.name)),
            }
        }
        said.extend(was.pegs.iter().filter(|old| peg(is, &old.name).is_none()).map(|old| format!("The peg “{}” is gone.", old.name)));
        let lens = |shot: &Shot| (shot.camera.distance, shot.camera.zoom, shot.ancestors(shot.camera.parent).next().map(|above| above.name.clone()));
        if lens(was) != lens(is) {
            said.push("The camera has another lens, or is moved by another peg.".to_string());
        }
        let seen = |shot: &Shot| shot.stack().into_iter().filter_map(|id| shot.layer(id).map(|layer| layer.name.clone())).collect::<Vec<_>>();
        if seen(was) != seen(is) {
            said.push("What is seen, or which layer is over which, has changed.".to_string());
        }
        // The retake notes ticked done, and those written, out there.
        let done = is.notes.iter().filter(|note| note.done && was.notes.iter().any(|old| old.same_as(note) && !old.done)).count();
        if done > 0 {
            said.push(format!("{} ticked done.", count(done, "retake note")));
        }
        let written = is.notes.iter().filter(|note| !was.notes.iter().any(|old| old.same_as(note))).count();
        if written > 0 {
            said.push(format!("{} written.", count(written, "new retake note")));
        }

        // What of the file doesn't match the project, which stays as it is.
        let (ours, theirs) = (&self.settings, &returned.settings);
        if (ours.width, ours.height) != (theirs.width, theirs.height) {
            said.push(format!("It was made at {} × {}. The project's {} × {} is kept.", theirs.width, theirs.height, ours.width, ours.height));
        }
        if ours.frame_rate != theirs.frame_rate {
            said.push(format!("It was made at {} frames a second. The project's {} is kept.", theirs.frame_rate.label(), ours.frame_rate.label()));
        }
        for swatch in &returned.palette.swatches {
            match self.palette.swatch(swatch.id) {
                Some(ours) if ours.color != swatch.color => {
                    said.push(format!("The swatch “{}” has another colour in the file. The project's is kept.", ours.name))
                }
                Some(_) => {}
                None => said.push(format!("A new swatch, “{}”, is added to the palette.", swatch.name)),
            }
        }
        Some(said)
    }

    /// Put the first shot of `returned` in the place of the shot `shot`: its layers, pegs,
    /// graph and length, under ids of this project. The shot keeps its id and its name, and
    /// so its clip, which is left as it is; it is now in animation. The project's settings
    /// stay, and its palette gets only the swatches it lacks. `None` if either has no such
    /// shot.
    pub fn bring_back(&mut self, shot: ShotId, returned: &Project) -> Option<()> {
        let at = self.shots.iter().position(|s| s.id == shot)?;
        let mut copy = Shot::clone(returned.shots.first()?);
        // New ids throughout: what was made in the file since may have ids that this project
        // has given to something else meanwhile.
        copy.reidentify(shot, || self.new_id());
        // Its variants in this project's skins of the same names.
        let like = self.skins_like(&returned.palette);
        copy.reskin(&like);
        copy.name = self.shots[at].name.clone();
        copy.bin = self.shots[at].bin;
        // Its notes as they come back, and those written here meanwhile.
        copy.keep_notes(&self.shots[at].notes);
        (copy.status, copy.out_to, copy.loaded) = (Status::Animation, None, true);
        if returned.palette.swatches.iter().any(|swatch| self.palette.swatch(swatch.id).is_none()) {
            let mut palette = Palette::clone(&self.palette);
            palette.adopt(&returned.palette);
            self.palette = Arc::new(palette);
        }
        self.shots[at] = Arc::new(copy);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Animation, DrawingId, PegId, PoolDrawing, Role, Swatch, SwatchId, Transform};

    /// A project whose shot is on its storyboard from frame 4 to 40, with a drawing on its
    /// layer `Drawing` from frame 0 and a peg `Bounce`.
    fn film() -> Project {
        let mut project = Project::default();
        let (drawing, peg, shot) = (DrawingId(project.new_id()), PegId(project.new_id()), project.shots[0].id);
        let layer = project.layer_mut(project.shots[0].layers[0].id).unwrap();
        layer.drawings.push(PoolDrawing { id: drawing, name: "1".to_string(), drawing: Arc::default() });
        layer.exposure.set(0, 12, Some(drawing));
        let bounce = Peg {
            id: peg,
            name: "Bounce".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Animation::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        };
        Arc::make_mut(&mut project.shots[0]).pegs.push(Arc::new(bounce));
        let board = project.storyboards[0].id;
        project.storyboard_mut(board).unwrap().put(Clip { shot, start: 0, in_point: 4, out_point: 40 }, false).unwrap();
        project
    }

    fn paint(drawing: &mut Arc<crate::Drawing>, swatch: u16) {
        if let crate::ArtContent::Indexed(surface) = &mut Arc::make_mut(drawing).art_layers[1].content {
            surface.paint(iskra_drawing::Rect::new(0, 0, 1, 1), &[1.0], SwatchId(swatch));
        }
    }

    #[test]
    fn a_shot_goes_out_as_a_project_of_its_own_with_its_cut() {
        let mut project = film();
        let shot = project.shots[0].id;
        (project.shot_mut(shot).unwrap().status, project.shot_mut(shot).unwrap().out_to) = (Status::Out, Some("Ana".to_string()));
        let file = project.shot_file(shot).unwrap();
        let [out] = &file.shots[..] else { panic!("one shot") };
        assert_eq!((out.id, out.name.as_str(), out.status, &out.out_to), (shot, "Shot_01", Status::Animation, &None));
        assert!(Arc::ptr_eq(&out.layers[0], &project.shots[0].layers[0]) && Arc::ptr_eq(&file.palette, &project.palette), "shared, not copied");
        assert_eq!(file.storyboards[0].clips(), [Clip { shot, start: 0, in_point: 4, out_point: 40 }], "the part that is in the cut");
        let mut next = file.clone();
        assert!(next.new_id() > project.clone().new_id(), "its ids go on from the project's");
        assert!(project.shot_file(ShotId(999)).is_none());
        assert_eq!(project.differences(shot, &file), Some(vec![]), "as it went out, nothing differs");
    }

    #[test]
    fn what_a_returned_shot_changes_is_said_and_the_project_wins() {
        let mut project = film();
        let shot = project.shots[0].id;
        let mut file = project.shot_file(shot).unwrap();
        // Meanwhile the project gives out ids that the file gives out too. (The file's
        // storyboard took one.)
        project.new_id();
        let meanwhile = project.new_layer("Notes", Role::Board);
        Arc::make_mut(&mut project.shots[0]).layers.push(Arc::new(meanwhile.clone()));

        // In the file: a new layer with a drawing, the old drawing painted on and timed
        // otherwise, a second drawing, the peg moved, the shot shortened, and other settings.
        let extra = file.new_layer("Shadow", Role::Animation);
        assert_eq!(extra.id, meanwhile.id, "the same id for two things");
        let second = DrawingId(file.new_id());
        let returned = Arc::make_mut(&mut file.shots[0]);
        returned.duration = 30;
        returned.connect_on_top(extra.id);
        returned.layers.insert(0, Arc::new(extra));
        let layer = Arc::make_mut(&mut returned.layers[1]);
        paint(&mut layer.drawings[0].drawing, 1);
        layer.drawings.push(PoolDrawing { id: second, name: "2".to_string(), drawing: Arc::default() });
        layer.exposure.set(6, 6, Some(second));
        Arc::make_mut(&mut returned.pegs[0]).transform.position = [5.0, 0.0];
        (file.settings.width, file.settings.height) = (1280, 720);
        let mut palette = Palette::clone(&file.palette);
        palette.swatches[0].color = [10, 10, 10, 255];
        palette.swatches.push(Swatch {
            id: SwatchId(40),
            name: "Skin".to_string(),
            color: [240, 200, 170, 255],
            set: Default::default(),
            kind: Default::default(),
            drift: None,
        });
        file.palette = Arc::new(palette);

        let said = project.differences(shot, &file).unwrap();
        assert_eq!(
            said,
            [
                "It is 30 frames long, where it was 48 frames.",
                "It no longer reaches its out point, frame 40: the cut is left as it is, and needs looking at.",
                "A new layer, “Shadow”, with 0 drawings.",
                "The layer “Drawing”: 1 drawing new, 1 drawing changed, the timing changed.",
                "The layer “Notes” is gone, with 0 drawings.",
                "The peg “Bounce” is placed, animated or parented differently.",
                "What is seen, or which layer is over which, has changed.",
                "It was made at 1280 × 720. The project's 1920 × 1080 is kept.",
                "The swatch “Black” has another colour in the file. The project's is kept.",
                "A new swatch, “Skin”, is added to the palette.",
            ]
        );

        let before = project.clone();
        project.bring_back(shot, &file).unwrap();
        let back = &project.shots[0];
        assert_eq!((back.id, back.name.as_str(), back.status, back.duration, back.loaded), (shot, "Shot_01", Status::Animation, 30, true));
        assert_eq!(back.layers.iter().map(|layer| layer.name.as_str()).collect::<Vec<_>>(), ["Shadow", "Drawing"]);
        let ids: Vec<u64> = back.node_ids().chain(back.drawings().map(|entry| entry.id.0)).collect();
        assert!(before.shots[0].node_ids().all(|id| !ids.contains(&id)) && ids.iter().all(|id| *id >= before.clone().new_id()), "new ids throughout");
        assert_eq!(back.layers[1].drawing_at(7).map(|entry| entry.name.as_str()), Some("2"), "the timing shows the drawings under their new ids");
        assert!(Arc::ptr_eq(&back.layers[1].drawings[0].drawing, &file.shots[0].layers[1].drawings[0].drawing), "the pixels are the file's");
        assert_eq!(back.stack(), [back.layers[0].id, back.layers[1].id]);
        assert_eq!(project.storyboards[0].clips(), before.storyboards[0].clips(), "the cut is left as it is");
        assert_eq!(
            (project.settings.width, project.palette.color(SwatchId(1)), project.palette.color(SwatchId(40))),
            (1920, [0, 0, 0, 255], [240, 200, 170, 255])
        );
        assert_eq!(project.differences(shot, &file).unwrap().len(), 2, "what is left to say is what of the file the project didn't take");
        assert!(project.bring_back(ShotId(999), &file).is_none() && project.differences(ShotId(999), &file).is_none());
    }
}
