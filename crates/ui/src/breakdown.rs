//! Breakdowns: the timing chart of the way between two keyframes, drawn on a timeline's row as
//! a line for each in-between frame, in a colour of its own (Settings ▸ Interface ▸ Colours),
//! the Animate switch's red until it is set. Where a line stands between the two keys is how
//! far along the way the value is on its frame, so a steady way puts each on its frame and an
//! ease bunches them towards a key. A line is dragged, between the frames too, and the lines
//! between it and the key it goes towards follow by halves or thirds, or stay, by hand
//! (`iskra_model::squash`). Both timelines draw them so, and share how they are set.

use egui::{CursorIcon, Rect, Sense, Stroke, pos2, vec2};
use iskra_model::Spacing;
use serde::{Deserialize, Serialize};

use crate::theme::{Role, Theme};

/// How the lines are shown and dragged, the same on both timelines, kept with the preferences.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Breakdowns {
    pub shown: bool,
    /// How the lines a dragged one pushes are spaced again, or by hand; Alt held while dragging
    /// swaps halves and thirds.
    pub rule: Spacing,
    /// The lines between the dragged one and the other key spaced again too.
    pub other_side: bool,
}

impl Default for Breakdowns {
    fn default() -> Self {
        Breakdowns { shown: true, rule: Spacing::Halves, other_side: true }
    }
}

impl Breakdowns {
    /// The rule a drag uses now: the one set, or the other of halves and thirds while Alt is
    /// held. Manual is manual either way.
    pub fn rule_now(self, ui: &egui::Ui) -> Spacing {
        match (self.rule, ui.input(|i| i.modifiers.alt)) {
            (rule, false) | (rule @ Spacing::Manual, true) => rule,
            (Spacing::Halves, true) => Spacing::Thirds,
            (Spacing::Thirds, true) => Spacing::Halves,
        }
    }
}

/// A line being dragged: of which way, by the row's id and the way's first frame; which line;
/// the spacing it started from and the one it makes now.
#[derive(Clone, Debug)]
pub struct ChartDrag {
    pub row: egui::Id,
    pub from: u32,
    pub line: usize,
    pub was: Vec<f64>,
    pub now: Vec<f64>,
}

/// What one way's lines came to on this frame.
#[derive(Default)]
pub struct Way {
    /// A line was let go of: the spacing to give the way.
    pub let_go: Option<Vec<f64>>,
    /// Where each line was drawn.
    pub lines: Vec<Rect>,
}

/// The lines must be at least this far apart, in points, to be drawn and dragged.
const ROOM: f32 = 3.0;

/// How wide a line is to grab.
const GRIP: f32 = 5.0;

/// Draw the lines of the way from the key whose middle is at `start` to the one at `end`, on a
/// row whose middle is at `y`, and let them be dragged. `spacings` is how each of the row's
/// values is spaced on the way: where they differ, the lines of the first are shown dimmed, as
/// mixed, and a line dragged gives them all one chart.
#[allow(clippy::too_many_arguments)]
pub fn way(
    ui: &egui::Ui,
    painter: &egui::Painter,
    settings: Breakdowns,
    drag: &mut Option<ChartDrag>,
    row: egui::Id,
    from: u32,
    (start, end): (f32, f32),
    y: f32,
    height: f32,
    spacings: &[Vec<f64>],
) -> Way {
    let mut made = Way::default();
    let Some(first) = spacings.first().filter(|first| !first.is_empty()) else { return made };
    if !settings.shown || end - start < ROOM * (first.len() + 1) as f32 {
        return made;
    }
    let mixed = spacings.iter().any(|other| other != first);
    let dragging = drag.as_ref().filter(|drag| drag.row == row && drag.from == from).cloned();
    let shown = dragging.as_ref().map_or(first.as_slice(), |drag| drag.now.as_slice());
    let red = Theme::of(ui.ctx()).colour(Role::Breakdowns);
    let x_of = |part: f64| start + part as f32 * (end - start);
    for (line, part) in shown.iter().enumerate() {
        let x = x_of(*part);
        // Kept off the keys' marks, which are grabbed to move the keys.
        let grip = Rect::from_center_size(pos2(x, y), vec2(GRIP, height)).intersect(Rect::from_x_y_ranges(start + 5.0..=end - 5.0, y - height..=y + height));
        made.lines.push(grip);
        let response =
            (grip.width() > 0.0).then(|| ui.interact(grip, row.with(("breakdown", from, line)), Sense::drag()).on_hover_cursor(CursorIcon::ResizeHorizontal));
        let held = dragging.as_ref().is_some_and(|drag| drag.line == line);
        let hovered = response.as_ref().is_some_and(egui::Response::hovered);
        if let Some(response) = &response {
            if response.drag_started() {
                *drag = Some(ChartDrag { row, from, line, was: first.clone(), now: first.clone() });
            }
            if let (Some(pos), Some(now)) =
                (response.interact_pointer_pos(), drag.as_mut().filter(|drag| drag.row == row && drag.from == from && drag.line == line))
            {
                let to = f64::from((pos.x - start) / (end - start).max(1.0));
                now.now = iskra_model::squash(&now.was, line, to, settings.rule_now(ui), settings.other_side);
            }
            if response.drag_stopped()
                && let Some(done) = drag.take_if(|drag| drag.row == row && drag.from == from)
            {
                made.let_go = Some(done.now);
            }
        }
        let colour = if mixed && dragging.is_none() { red.gamma_multiply(0.45) } else { red };
        let (half, width) = if held || hovered { (height * 0.4, 2.5) } else { (height * 0.3, 1.5) };
        painter.vline(x, y - half..=y + half, Stroke::new(width, colour));
    }
    made
}

/// The Breakdown menu in a timeline's bar: the lines shown or not, the rule, the other side,
/// and Even for the ways of the keyframes that are picked. Whether Even was chosen.
pub fn menu(ui: &mut egui::Ui, settings: &mut Breakdowns, can_even: bool) -> bool {
    let mut even = false;
    ui.menu_button("Breakdown", |ui| {
        ui.checkbox(&mut settings.shown, "Show Breakdowns")
            .on_hover_text("A red line for each in-between between two keyframes, at how far along the way it is: drag one to time the in-betweens");
        ui.separator();
        let hint = "How the lines a dragged one pushes are spaced again; Alt held while dragging uses the other";
        ui.radio_value(&mut settings.rule, Spacing::Halves, "Halves")
            .on_hover_text(format!("Each halfway between the key and the line after it, an ease. {hint}"));
        ui.radio_value(&mut settings.rule, Spacing::Thirds, "Thirds").on_hover_text(format!("Evenly in the room there is. {hint}"));
        ui.radio_value(&mut settings.rule, Spacing::Manual, "Manual")
            .on_hover_text("The dragged line moves alone, held between the ones beside it: every in-between spaced by hand");
        let other_side = ui.add_enabled(settings.rule != Spacing::Manual, egui::Checkbox::new(&mut settings.other_side, "Space the Other Side Too"));
        other_side.on_hover_text(
            "The lines between the dragged one and the other keyframe are spaced again too; off, they stay where they are, to be spaced by hand",
        );
        ui.separator();
        let hint = "The way from each picked keyframe to the next timed by its eases again, without a chart";
        if ui.add_enabled(can_even, egui::Button::new("Even")).on_hover_text(hint).on_disabled_hover_text("Pick a keyframe first").clicked() {
            even = true;
            ui.close();
        }
    });
    even
}
