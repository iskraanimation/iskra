//! The clips of the storyboard timeline coloured by their shots' scenes or sequences (the
//! user's, 2026-10-28). The colours are each user's own, in the preferences, not the project's:
//! a name with none of its own has one made from its letters, the same each time.

use std::collections::BTreeMap;

use egui::Color32;
use iskra_model::{Project, Shot, ShotId, Storyboard};
use serde::{Deserialize, Serialize};

/// What the clips are coloured by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColourBy {
    /// Every second shot a little lighter, as before.
    #[default]
    Nothing,
    Scene,
    Sequence,
}

/// How the clips are coloured, kept with the preferences.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ClipColours {
    pub by: ColourBy,
    /// The colours given to scenes and to sequences, by their names, as sRGB.
    pub scenes: BTreeMap<String, [u8; 3]>,
    pub sequences: BTreeMap<String, [u8; 3]>,
}

/// The colours a name is given from, by its letters, where it has none of its own.
const MADE: [[u8; 3]; 8] = [[196, 92, 82], [214, 150, 64], [186, 178, 70], [98, 172, 92], [70, 164, 166], [82, 128, 206], [148, 102, 198], [196, 98, 156]];

/// The colour of a name with none of its own: the same for the same name, on any machine.
pub fn made(name: &str) -> [u8; 3] {
    let sum = name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |sum, byte| (sum ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3));
    MADE[(sum % MADE.len() as u64) as usize]
}

impl ClipColours {
    /// The names given, of scenes or of sequences.
    fn of(&self, sequence: bool) -> &BTreeMap<String, [u8; 3]> {
        if sequence { &self.sequences } else { &self.scenes }
    }

    /// A scene's colour, or a sequence's.
    pub fn colour(&self, sequence: bool, name: &str) -> [u8; 3] {
        self.of(sequence).get(name).copied().unwrap_or_else(|| made(name))
    }

    /// The colour a shot's clip is drawn in, from what the clips are coloured by; none where
    /// they are coloured by nothing, or the shot is in no scene or sequence.
    pub fn of_shot(&self, shot: &Shot) -> Option<Color32> {
        let (sequence, name) = match self.by {
            ColourBy::Nothing => return None,
            ColourBy::Scene => (false, &shot.scene),
            ColourBy::Sequence => (true, &shot.sequence),
        };
        let [r, g, b] = self.colour(sequence, name);
        (!name.is_empty()).then_some(Color32::from_rgb(r, g, b))
    }
}

/// The shots of `board` in the scene called `name`, or the sequence with `sequence`, in the
/// order they lie in.
pub fn shots_of(project: &Project, board: &Storyboard, sequence: bool, name: &str) -> Vec<ShotId> {
    let mut clips: Vec<&iskra_model::Clip> = board.clips().iter().collect();
    clips.sort_by_key(|clip| clip.start);
    let named = |shot: &Shot| if sequence { shot.sequence == name } else { shot.scene == name };
    clips.into_iter().filter(|clip| project.shot(clip.shot).is_some_and(named)).map(|clip| clip.shot).collect()
}

/// What is asked for the shots of a scene or a sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Pick them on the storyboard.
    Pick(Vec<ShotId>),
    /// Send them out, as shots are sent out, once the user has said where to.
    SendOut(Vec<ShotId>),
}

/// The clips' colours, under a shot's Properties: what the storyboard timeline's clips are
/// coloured by, and each scene and sequence in use with its colour, to change, and buttons that
/// pick its shots on `board` and send them out.
pub fn colours(ui: &mut egui::Ui, colours: &mut ClipColours, project: &Project, board: &Storyboard) -> Option<Asked> {
    let mut asked = None;
    ui.strong("Clip Colours").on_hover_text("How the clips on the storyboard timeline are coloured");
    ui.radio_value(&mut colours.by, ColourBy::Nothing, "Every Second Shot Lighter");
    ui.radio_value(&mut colours.by, ColourBy::Scene, "By Scene");
    ui.radio_value(&mut colours.by, ColourBy::Sequence, "By Sequence");
    for (sequence, title, names) in [(false, "Scenes", project.scenes()), (true, "Sequences", project.sequences())] {
        ui.separator();
        ui.weak(title);
        if names.is_empty() {
            ui.weak("None given yet: in a shot's Properties, or its menu in the bin");
        }
        for name in names {
            ui.horizontal(|ui| {
                let mut colour = colours.colour(sequence, &name);
                let hint = "Its colour on the storyboard timeline: yours, kept in the preferences";
                if ui.color_edit_button_srgb(&mut colour).on_hover_text(hint).changed() {
                    if sequence { &mut colours.sequences } else { &mut colours.scenes }.insert(name.clone(), colour);
                }
                let shots = shots_of(project, board, sequence, &name);
                ui.label(format!("{name}  ({})", shots.len()));
                if ui.add_enabled(!shots.is_empty(), egui::Button::new("Pick")).on_hover_text("Pick its shots on this storyboard").clicked() {
                    asked = Some(Asked::Pick(shots.clone()));
                }
                let hint = "Send its shots on this storyboard out, each as a project file of its own, for someone to work on";
                if ui.add_enabled(!shots.is_empty(), egui::Button::new("Send Out…")).on_hover_text(hint).clicked() {
                    asked = Some(Asked::SendOut(shots));
                }
            });
        }
    }
    asked
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_shot_is_coloured_by_its_scene_or_sequence_and_a_name_has_a_colour_from_its_letters() {
        let mut project = Project::default();
        let board = project.storyboards[0].id;
        let first = project.shots[0].id;
        let mut second = project.new_shot("Shot_02", 24);
        (second.scene, second.sequence) = ("SC010".to_string(), "SQ01".to_string());
        let second_id = second.id;
        project.shots.push(Arc::new(second));
        project.storyboard_mut(board).unwrap().put(iskra_model::Clip { shot: second_id, start: 48, in_point: 0, out_point: 24 }, false).unwrap();
        Arc::make_mut(&mut project.shots[0]).scene = "SC010".to_string();

        let mut colours = ClipColours::default();
        let shot = |at: usize| project.shots[at].clone();
        assert_eq!(colours.of_shot(&shot(1)), None, "coloured by nothing, as before");
        colours.by = ColourBy::Scene;
        let [r, g, b] = made("SC010");
        assert_eq!(colours.of_shot(&shot(1)), Some(Color32::from_rgb(r, g, b)));
        assert_eq!(made("SC010"), made("SC010"));
        colours.scenes.insert("SC010".to_string(), [1, 2, 3]);
        assert_eq!((colours.of_shot(&shot(0)), colours.of_shot(&shot(1))), (Some(Color32::from_rgb(1, 2, 3)), Some(Color32::from_rgb(1, 2, 3))));
        colours.by = ColourBy::Sequence;
        assert_eq!(colours.of_shot(&shot(0)), None, "in no sequence");

        let board = project.storyboard(board).unwrap();
        assert_eq!(shots_of(&project, board, false, "SC010"), [first, second_id]);
        assert_eq!(shots_of(&project, board, true, "SQ01"), [second_id]);
        assert_eq!(shots_of(&project, board, true, "SQ02"), []);
    }
}
