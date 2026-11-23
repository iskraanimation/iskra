//! The Script Console panel (plan §9): a script typed in and run against the open project, and
//! what came of each run beneath it. A run is one undo step, as any script's.

use egui::RichText;

use crate::theme::Theme;

/// A line of what the console shows of its runs.
#[derive(Clone, Debug, PartialEq)]
enum Said {
    /// A run began: the script's name.
    Ran(String),
    /// What it printed.
    Printed(String),
    /// Its last value, where it had one.
    Result(String),
    Error(String),
}

#[derive(Default)]
pub struct ScriptConsole {
    /// The script typed in.
    pub code: String,
    said: Vec<Said>,
    /// Where the editor and the output were last drawn.
    #[cfg(test)]
    drawn: Option<(egui::Rect, egui::Rect)>,
}

/// How much of what was said is kept; the oldest goes first.
const KEPT: usize = 1000;

impl ScriptConsole {
    /// Write down what came of a run of the script `name`.
    pub fn ran(&mut self, name: &str, outcome: &iskra_script::Outcome) {
        // A run that stopped at its dialog says what it does once that is answered.
        if outcome.asks.is_some() {
            return;
        }
        self.said.push(Said::Ran(name.to_string()));
        self.said.extend(outcome.output.iter().cloned().map(Said::Printed));
        match &outcome.error {
            Some(error) => self.said.push(Said::Error(error.to_string())),
            None if !outcome.result.is_null() => self.said.push(Said::Result(outcome.result.to_string())),
            None => {}
        }
        let over = self.said.len().saturating_sub(KEPT);
        self.said.drain(..over);
    }

    pub fn clear(&mut self) {
        self.said.clear();
    }

    /// The editor over what was said, filling the room above it; the line between them is
    /// dragged to give one more and the other less, and kept, as the Panels panel's notes are. Gives whether the script is to be run: the
    /// editor was sent Ctrl+Enter (⌘+Enter on a Mac).
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        let theme = Theme::of(ui.ctx());
        let run = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter);
        let foot = egui::Panel::bottom(ui.id().with("script said")).resizable(true);
        let said = foot.default_size(ui.available_height() * 0.45).min_size(40.0).show(ui, |ui| {
            egui::ScrollArea::both().id_salt("said").auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
                for said in &self.said {
                    let line = match said {
                        Said::Ran(name) => RichText::new(format!("› {name}")).color(theme.text_muted),
                        Said::Printed(text) => RichText::new(text),
                        Said::Result(value) => RichText::new(format!("= {value}")).color(theme.accent),
                        Said::Error(why) => RichText::new(why).color(theme.danger),
                    };
                    ui.add(egui::Label::new(line.monospace()).extend());
                }
            });
        });
        #[cfg(test)]
        {
            self.drawn = Some((egui::Rect::NOTHING, said.response.rect));
        }
        #[cfg(not(test))]
        let _ = said;
        // The editor is as tall as all the room above the output, however that is dragged.
        let room = ui.available_height();
        let editor = egui::ScrollArea::vertical().id_salt("script").auto_shrink(false).show(ui, |ui| {
            let editor = egui::TextEdit::multiline(&mut self.code)
                .code_editor()
                .desired_width(f32::INFINITY)
                .min_size(egui::vec2(0.0, room))
                .hint_text("iskra.run(\"project.get\").shots.length");
            ui.add(editor)
        });
        #[cfg(test)]
        {
            if let Some(drawn) = self.drawn.as_mut() {
                drawn.0 = editor.inner.rect;
            }
        }
        let editor = editor.inner.on_hover_text("A script to run against the open project: Ctrl+Enter (⌘+Enter on a Mac) runs it");
        editor.has_focus() && ui.input_mut(|i| i.consume_shortcut(&run))
    }

    /// The lines it shows of its runs, as text, for tests.
    #[cfg(test)]
    pub fn said(&self) -> Vec<String> {
        self.said
            .iter()
            .map(|said| match said {
                Said::Ran(name) => format!("› {name}"),
                Said::Printed(text) => text.clone(),
                Said::Result(value) => format!("= {value}"),
                Said::Error(why) => why.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_fills_the_room_above_the_output_however_tall_that_is() {
        let ctx = egui::Context::default();
        let mut console = ScriptConsole::default();
        for _ in 0..2 {
            let shown = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    console.show(ui);
                });
            });
            let mut textures = shown.textures_delta;
            textures.clear();
        }
        let (editor, said) = console.drawn.expect("drawn");
        assert!(editor.height() > 8.0 * 14.0, "taller than its eight rows: {editor:?}");
        assert!((said.top() - editor.bottom()).abs() < 12.0, "no room left between them: {editor:?} {said:?}");
    }
}
