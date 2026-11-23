//! The Retake Notes panel (plan §9.6): every shot's notes, by shot, to be gone through and
//! ticked done. A click on a note's frames goes there; its right-click menu changes its text or
//! takes it away. Its bar writes a new note at the playhead.

use egui::Color32;
use iskra_model::{Note, Project, ShotId};
use serde_json::{Value, json};

use crate::theme::Theme;

/// What was done in the panel, for the app to do.
pub enum Action {
    Run(&'static str, Value),
    /// Go to the shot, on the frame.
    Go(ShotId, u32),
}

#[derive(Default)]
pub struct NotesPanel {
    /// What is written of the next note, in the bar.
    pub writing: String,
    /// The bar's field is given the focus when it is next drawn.
    pub focus: bool,
    /// Notes ticked done are listed too.
    pub show_done: bool,
    /// The note whose text is being changed, with what it is changed to.
    editing: Option<(ShotId, u64, String)>,
    /// Each note's tick box as it was last drawn.
    #[cfg(test)]
    pub ticks: Vec<(ShotId, u64, egui::Rect)>,
    /// Where the bar's button was last drawn.
    #[cfg(test)]
    pub add_button: Option<egui::Rect>,
}

/// A day, from seconds since 1970, as `2026-11-03`.
pub fn day(seconds: u64) -> String {
    // Days to the civil date, as Howard Hinnant's `civil_from_days` works it out.
    let z = (seconds / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// The colour of an open note's mark; a done one is greyed.
const OPEN: Color32 = Color32::from_rgb(244, 160, 0);

/// A note's mark on a timeline's ruler: a band at its foot over the note's frames, `band`,
/// orange while it is open. Its tooltip is the note. Gives what was done on it.
pub fn mark(ui: &egui::Ui, painter: &egui::Painter, band: egui::Rect, id: egui::Id, note: &Note) -> egui::Response {
    let colour = if note.done { Theme::of(ui.ctx()).text_muted.gamma_multiply(0.6) } else { OPEN };
    painter.rect_filled(band, 1.5, colour);
    let by = if note.author.is_empty() { String::new() } else { format!("\nBy {}, {}", note.author, day(note.written)) };
    let state = if note.done { "Done" } else { "Open" };
    let said =
        format!("Retake note, frame {}: {}{by}\n{state}. Click to go to it; the Retake Notes panel lists them all", frames(note.frame, note.length), note.text);
    ui.interact(band.expand2(egui::vec2(0.0, 2.0)), id, egui::Sense::click()).on_hover_text(said)
}

/// The frames a note is on, as the window counts them, from 1.
pub fn frames(frame: u32, length: u32) -> String {
    match length {
        0 | 1 => format!("{}", frame + 1),
        _ => format!("{}–{}", frame + 1, frame + length),
    }
}

impl NotesPanel {
    /// The bar's field and button: a note written at `at`, the shot's name and frame there
    /// are, gives its text.
    pub fn write_bar(&mut self, ui: &mut egui::Ui, at: Option<(&str, u32)>) -> Option<String> {
        let field = egui::TextEdit::singleline(&mut self.writing).hint_text("A retake note").desired_width(130.0);
        let field = ui.add_enabled(at.is_some(), field);
        if std::mem::take(&mut self.focus) {
            field.request_focus();
        }
        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let label = at.map_or("Add".to_string(), |(_, frame)| format!("Add on {}", frame + 1));
        let hint = at.map_or("The playhead is on no shot".to_string(), |(shot, frame)| format!("Pin the note to {shot}, frame {}", frame + 1));
        let button = ui.add_enabled(at.is_some() && !self.writing.trim().is_empty(), egui::Button::new(label)).on_hover_text(hint);
        #[cfg(test)]
        {
            self.add_button = Some(button.rect);
        }
        let clicked = button.clicked();
        if (clicked || entered) && !self.writing.trim().is_empty() {
            return Some(std::mem::take(&mut self.writing));
        }
        None
    }

    /// The notes of the shots of `project`, those of `current` first.
    pub fn show(&mut self, ui: &mut egui::Ui, project: &Project, current: Option<ShotId>) -> Vec<Action> {
        let mut actions = Vec::new();
        #[cfg(test)]
        self.ticks.clear();
        let mut shots: Vec<_> = project.shots.iter().filter(|shot| !shot.notes.is_empty()).collect();
        shots.sort_by_key(|shot| Some(shot.id) != current);
        let shown = |done: bool| self.show_done || !done;
        if shots.iter().all(|shot| !shot.notes.iter().any(|note| shown(note.done))) {
            let done: usize = shots.iter().map(|shot| shot.notes.len()).sum();
            match done {
                0 => ui.weak("No retake notes. Write one at the playhead in the bar above."),
                _ => ui.weak(format!("All {done} notes are done: Show Done lists them.")),
            };
            return actions;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for shot in shots {
                let open = shot.notes.iter().filter(|note| !note.done).count();
                ui.horizontal(|ui| {
                    ui.strong(&shot.name);
                    ui.weak(if open == 0 { "all done".to_string() } else { format!("{open} open") });
                });
                for note in shot.notes.iter().filter(|note| shown(note.done)) {
                    ui.horizontal(|ui| {
                        let mut done = note.done;
                        let tick = ui.checkbox(&mut done, "").on_hover_text(if note.done { "Done: untick to open it again" } else { "Tick it done" });
                        #[cfg(test)]
                        self.ticks.push((shot.id, note.id, tick.rect));
                        if tick.changed() {
                            actions.push(Action::Run("note.set", json!({ "shot": shot.id.0, "note": note.id, "done": done })));
                        }
                        let at = ui.link(frames(note.frame, note.length)).on_hover_text("Go to it");
                        if at.clicked() {
                            actions.push(Action::Go(shot.id, note.frame));
                        }
                        let editing = self.editing.as_mut().filter(|(of, id, _)| *of == shot.id && *id == note.id);
                        let row = match editing {
                            Some((_, _, text)) => {
                                let field = ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width() - 8.0));
                                field.request_focus();
                                if field.lost_focus() {
                                    let typed = std::mem::take(text);
                                    self.editing = None;
                                    if ui.input(|i| !i.key_pressed(egui::Key::Escape)) && typed.trim() != note.text && !typed.trim().is_empty() {
                                        actions.push(Action::Run("note.set", json!({ "shot": shot.id.0, "note": note.id, "text": typed })));
                                    }
                                }
                                field
                            }
                            None => {
                                let text = egui::RichText::new(&note.text);
                                let text = if note.done { text.weak().strikethrough() } else { text };
                                let said = match note.author.as_str() {
                                    "" => format!("Written {}", day(note.written)),
                                    by => format!("By {by}, {}", day(note.written)),
                                };
                                ui.add(egui::Label::new(text).wrap().sense(egui::Sense::click()))
                                    .on_hover_text(format!("{said}\nRight-click to change or remove it"))
                            }
                        };
                        row.context_menu(|ui| {
                            if ui.button("Change Text").clicked() {
                                self.editing = Some((shot.id, note.id, note.text.clone()));
                                ui.close();
                            }
                            if ui.button("Remove Note").clicked() {
                                actions.push(Action::Run("note.remove", json!({ "shot": shot.id.0, "note": note.id })));
                                ui.close();
                            }
                        });
                    });
                }
                ui.add_space(6.0);
            }
        });
        actions
    }

    /// Where a note's tick box was last drawn.
    #[cfg(test)]
    pub fn tick(&self, shot: ShotId, note: u64) -> Option<egui::Rect> {
        self.ticks.iter().find(|(of, id, _)| *of == shot && *id == note).map(|(_, _, rect)| *rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_frames_are_said_as_the_window_counts_them() {
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(1_793_664_000), "2026-11-03");
        assert_eq!(day(951_782_400), "2000-02-29");
        assert_eq!((frames(41, 1), frames(2, 6)), ("42".to_string(), "3–8".to_string()));
    }
}
