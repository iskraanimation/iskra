//! Panels (plan §4.7): the pictures a storyboard is read by. A panel is a frame marker inside
//! a shot, not a document of its own: the boards are drawn into the shot's layers, and a panel
//! says from which frame on the shot shows another picture, with what is said of it. It lasts
//! until the next panel, or the shot's end.
//!
//! A shot's first panel is on its first frame and is the shot's own: it is there whether
//! anything was written of it or not, and it is neither moved nor removed.

use serde::{Deserialize, Serialize};

use crate::{LayerId, Shot};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Panel {
    /// The frame of the shot the panel starts on.
    pub frame: u32,
    /// What happens in it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
    /// What is spoken over it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dialogue: String,
    /// Anything else: for the animator, the editor, the sound.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

impl Panel {
    /// Nothing is written of it.
    pub fn is_blank(&self) -> bool {
        self.action.is_empty() && self.dialogue.is_empty() && self.notes.is_empty()
    }
}

impl Shot {
    /// The shot's panels in the order of their frames, the one on the first frame among them
    /// whether it was written of or not.
    pub fn panels(&self) -> Vec<Panel> {
        let first = self.panels.first().is_none_or(|panel| panel.frame > 0).then(Panel::default);
        first.into_iter().chain(self.panels.iter().cloned()).collect()
    }

    /// The panel that starts on `frame`, if one does.
    pub fn panel_on(&self, frame: u32) -> Option<Panel> {
        self.panels().into_iter().find(|panel| panel.frame == frame)
    }

    /// The panel that `frame` is in: the last that starts on it or before it.
    pub fn panel_at(&self, frame: u32) -> Panel {
        self.panels().into_iter().rfind(|panel| panel.frame <= frame).unwrap_or_default()
    }

    /// The frame after the last of the panel that starts on `frame`: where the next one
    /// starts, or the shot ends.
    pub fn panel_end(&self, frame: u32) -> u32 {
        self.panels.iter().map(|panel| panel.frame).find(|start| *start > frame).unwrap_or(self.duration).max(frame + 1)
    }

    /// Put a panel into the shot, in place of the one that starts on its frame, if one does.
    /// The board layers a panel is tied to the drawings of, in their order: those the shot
    /// shows first, the top first, then any it does not show.
    fn board_layers(&self) -> Vec<LayerId> {
        let mut layers = self.stack();
        let rest: Vec<LayerId> = self.layers.iter().map(|layer| layer.id).filter(|id| !layers.contains(id)).collect();
        layers.extend(rest);
        layers.retain(|id| self.is_board(*id));
        layers
    }

    /// Move each panel with the board drawing it is tied to, as the shot was `before` an edit
    /// and is now (plan §7.2c): the drawing that started on its frame then, on the first board
    /// layer that had one starting there. Where that drawing now starts elsewhere, the nearest
    /// to where it was, the panel starts there too, unless another panel does. A panel with no
    /// drawing starting on its frame, the first panel, and one whose drawing is gone, stay.
    pub fn panels_follow(&mut self, before: &Shot) {
        let layers = before.board_layers();
        let mut moves: Vec<(u32, u32)> = Vec::new();
        for panel in before.panels.iter().filter(|panel| panel.frame > 0) {
            let tied = layers.iter().find_map(|id| {
                let run = before.layer(*id)?.exposure.runs().iter().find(|run| run.start == panel.frame)?;
                Some((*id, run.drawing))
            });
            let Some((layer, drawing)) = tied else { continue };
            let Some(now) = self.layer(layer) else { continue };
            let starts: Vec<u32> = now.exposure.runs().iter().filter(|run| run.drawing == drawing).map(|run| run.start).collect();
            if starts.contains(&panel.frame) {
                continue;
            }
            if let Some(to) = starts.into_iter().min_by_key(|start| start.abs_diff(panel.frame)).filter(|to| *to > 0 && *to < self.duration) {
                moves.push((panel.frame, to));
            }
        }
        for (from, to) in moves {
            if self.panels.iter().any(|panel| panel.frame == to) {
                continue;
            }
            if let Some(panel) = self.take_panel(from) {
                self.put_panel(Panel { frame: to, ..panel });
            }
        }
    }

    pub fn put_panel(&mut self, panel: Panel) {
        self.panels.retain(|other| other.frame != panel.frame);
        let at = self.panels.partition_point(|other| other.frame < panel.frame);
        self.panels.insert(at, panel);
    }

    /// Take the panel that starts on `frame` out of the shot. What it showed is then of the
    /// panel before it.
    pub fn take_panel(&mut self, frame: u32) -> Option<Panel> {
        let at = self.panels.iter().position(|panel| panel.frame == frame)?;
        Some(self.panels.remove(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    #[test]
    fn a_shot_is_read_by_its_panels_and_the_first_is_always_there() {
        let mut project = Project::default();
        let shot = std::sync::Arc::make_mut(&mut project.shots[0]);
        let starts = |shot: &Shot| shot.panels().iter().map(|panel| (panel.frame, shot.panel_end(panel.frame))).collect::<Vec<_>>();
        assert_eq!((starts(shot), shot.panel_at(30).is_blank()), (vec![(0, 48)], true), "one panel, the whole shot");
        // Panels are kept in the order of their frames, each lasting until the next.
        for (frame, action) in [(24, "She turns"), (12, "The door opens")] {
            shot.put_panel(Panel { frame, action: action.to_string(), ..Panel::default() });
        }
        assert_eq!(starts(shot), [(0, 12), (12, 24), (24, 48)]);
        assert_eq!((shot.panel_at(23).action.as_str(), shot.panel_at(24).action.as_str(), shot.panel_at(5).frame), ("The door opens", "She turns", 0));
        assert!(shot.panel_on(12).is_some() && shot.panel_on(13).is_none() && shot.panel_on(0).is_some());
        // Putting one where one is takes its place; what is written of the first is kept as any.
        shot.put_panel(Panel { frame: 0, dialogue: "Hello?".to_string(), ..Panel::default() });
        shot.put_panel(Panel { frame: 12, action: "The door creaks".to_string(), ..Panel::default() });
        assert_eq!((shot.panels.len(), shot.panel_at(0).dialogue.as_str(), shot.panel_at(12).action.as_str()), (3, "Hello?", "The door creaks"));
        // Taken out, its frames are the panel's before it.
        assert_eq!(shot.take_panel(12).map(|panel| panel.action), Some("The door creaks".to_string()));
        assert_eq!((starts(shot), shot.take_panel(12)), (vec![(0, 24), (24, 48)], None));
        // What is blank is not written to the file.
        shot.put_panel(Panel { frame: 30, ..Panel::default() });
        assert_eq!(serde_json::to_value(&shot.panels[2]).unwrap(), serde_json::json!({ "frame": 30 }));
    }
}
