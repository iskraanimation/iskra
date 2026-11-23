//! The editor of an effect node's curves of tone (plan §8.4), in the Properties panel: a
//! square with tones going in from left to right, dark to light, and coming out from bottom
//! to top, and the curve of the master or of one colour drawn on it. A drag on a point moves
//! it, a drag that starts off the points makes a new one there, and a right-click on a point
//! takes it away. The point last taken hold of is picked, and its two numbers are under the
//! square, to be typed.

use egui::{Color32, Pos2, Sense, Stroke, pos2, vec2};
use iskra_model::{Curves, STRAIGHT};

use crate::theme::{Theme, mix};

/// How near the pointer has to be to a point to take hold of it, in points of the screen.
const REACH: f32 = 9.0;
/// The least two points of a curve are apart, in tone.
const APART: f64 = 0.01;

/// What the editor remembers between frames.
#[derive(Default)]
pub(crate) struct ToneCurves {
    /// Which of the four is shown and changed: a place in [`Curves::NAMES`].
    pub channel: usize,
    /// The point that is being dragged, by its place among the curve's points.
    drag: Option<usize>,
    /// The point last taken hold of, whose numbers are typed, by its place.
    pub picked: Option<usize>,
    /// Where the square was drawn last, for the tests, which can't see it.
    #[cfg(test)]
    pub area: Option<egui::Rect>,
    /// What of the panel it is in could be seen, for the tests.
    #[cfg(test)]
    pub seen: Option<egui::Rect>,
    /// Where the picked point's two numbers were drawn, for the tests.
    #[cfg(test)]
    pub fields: Option<(egui::Rect, egui::Rect)>,
}

impl ToneCurves {
    /// Draws the square for `curves`, and says what they are now if the pointer changed them
    /// on this frame, and whether a point is still being dragged.
    pub fn show(&mut self, ui: &mut egui::Ui, curves: &Curves) -> (Option<Curves>, bool) {
        let theme = Theme::of(ui.ctx());
        let side = ui.available_width().clamp(120.0, 220.0);
        let (area, response) = ui.allocate_exact_size(vec2(side, side), Sense::click_and_drag());
        #[cfg(test)]
        {
            (self.area, self.seen) = (Some(area), Some(ui.clip_rect()));
        }
        let on_screen = |[tone, out]: [f64; 2]| pos2(area.left() + tone as f32 * area.width(), area.bottom() - out as f32 * area.height());
        let in_tones = |at: Pos2| {
            let part = |value: f32, from: f32, size: f32| f64::from(((value - from) / size).clamp(0.0, 1.0));
            [part(at.x, area.left(), area.width()), 1.0 - part(at.y, area.top(), area.height())]
        };
        // A curve that was never changed is shown as the straight line's two points.
        let mut points: Vec<[f64; 2]> = if curves.points(self.channel).is_empty() { STRAIGHT.to_vec() } else { curves.points(self.channel).to_vec() };
        let near = |points: &[[f64; 2]], at: Pos2| {
            let reached = points.iter().enumerate().map(|(place, point)| (place, on_screen(*point).distance(at))).filter(|(_, away)| *away <= REACH);
            reached.min_by(|a, b| a.1.total_cmp(&b.1)).map(|(place, _)| place)
        };
        let mut changed = false;
        let pointer = response.interact_pointer_pos();
        if let (true, Some(at)) = (response.drag_started_by(egui::PointerButton::Primary), pointer) {
            // Hold of the point under the pointer, or of a new one made where it is.
            self.drag = near(&points, at).or_else(|| {
                let made = in_tones(at);
                let place = points.partition_point(|point| point[0] < made[0]);
                let room = points.len() < Curves::MOST && points.iter().all(|point| (point[0] - made[0]).abs() >= APART);
                room.then(|| {
                    points.insert(place, made);
                    changed = true;
                    place
                })
            });
            self.picked = self.drag.or(self.picked);
        }
        if let (Some(place), Some(at), true) = (self.drag, pointer, response.dragged_by(egui::PointerButton::Primary)) {
            // It goes with the pointer, and stays between the points on its two sides.
            let to = in_tones(at);
            let least = place.checked_sub(1).map_or(0.0, |before| points[before][0] + APART);
            let most = points.get(place + 1).map_or(1.0, |after| after[0] - APART);
            let moved = [to[0].clamp(least, most.max(least)), to[1]];
            if points.get(place).is_some_and(|point| *point != moved) {
                points[place] = moved;
                changed = true;
            }
        }
        if !response.dragged_by(egui::PointerButton::Primary) {
            self.drag = None;
        }
        // A point that is right-clicked is taken away, while two are left.
        if response.secondary_clicked()
            && let Some(place) = response.interact_pointer_pos().and_then(|at| near(&points, at))
            && points.len() > 2
        {
            points.remove(place);
            changed = true;
            self.picked = None;
        }
        let picked = self.picked.filter(|place| *place < points.len());

        let painter = ui.painter_at(area.expand(1.0));
        painter.rect_filled(area, 2.0, theme.surface_raised);
        let faint = Stroke::new(1.0, mix(theme.surface_raised, theme.border, 0.8));
        for quarter in 1..4 {
            let (x, y) = (area.left() + area.width() * quarter as f32 / 4.0, area.top() + area.height() * quarter as f32 / 4.0);
            painter.line_segment([pos2(x, area.top()), pos2(x, area.bottom())], faint);
            painter.line_segment([pos2(area.left(), y), pos2(area.right(), y)], faint);
        }
        painter.line_segment([area.left_bottom(), area.right_top()], faint);
        painter.rect_stroke(area, 2.0, Stroke::new(1.0, theme.border), egui::StrokeKind::Inside);
        // The curves that are not in hand, faintly, and the one that is over them.
        let mut shown = curves.clone();
        *shown.points_mut(self.channel) = points.clone();
        let colour = |channel: usize| {
            [theme.text, Color32::from_rgb(0xe5, 0x48, 0x4d), Color32::from_rgb(0x46, 0xa7, 0x58), Color32::from_rgb(0x3e, 0x8e, 0xf0)][channel]
        };
        let line =
            |channel: usize| -> Vec<Pos2> { (0..=64).map(|step| f64::from(step) / 64.0).map(|tone| on_screen([tone, shown.at(channel, tone)])).collect() };
        for channel in (0..4).filter(|channel| *channel != self.channel && !shown.points(*channel).is_empty()) {
            painter.add(egui::Shape::line(line(channel), Stroke::new(1.0, mix(theme.surface_raised, colour(channel), 0.45))));
        }
        painter.add(egui::Shape::line(line(self.channel), Stroke::new(1.5, colour(self.channel))));
        let hovered = response.hover_pos().and_then(|at| near(&points, at));
        for (place, point) in points.iter().enumerate() {
            let lit = self.drag == Some(place) || (self.drag.is_none() && hovered == Some(place));
            painter.circle_filled(on_screen(*point), 3.5, if lit { theme.accent } else { colour(self.channel) });
            painter.circle_stroke(on_screen(*point), 3.5, Stroke::new(1.0, theme.surface));
            if picked == Some(place) {
                painter.circle_stroke(on_screen(*point), 6.0, Stroke::new(1.0, theme.accent));
            }
        }
        response.on_hover_text("Drag a point, or drag from anywhere else to make one. A right-click on a point takes it away");
        // The picked point's numbers, from 0 to 255 as a paint program has them: the tone that
        // goes in, kept between the points on its two sides, and the tone that comes out.
        let mut typing = false;
        if let Some(place) = picked {
            let level = |tone: f64| (tone * 255.0).round();
            let (mut going, mut coming) = (level(points[place][0]), level(points[place][1]));
            let least = place.checked_sub(1).map_or(0.0, |before| points[before][0] + APART);
            let most = points.get(place + 1).map_or(1.0, |after| after[0] - APART);
            ui.horizontal(|ui| {
                let range = ((least * 255.0).ceil())..=((most * 255.0).floor().max((least * 255.0).ceil()));
                let one = ui.add(egui::DragValue::new(&mut going).range(range).speed(1.0).max_decimals(0).prefix("In "));
                let other = ui.add(egui::DragValue::new(&mut coming).range(0.0..=255.0).speed(1.0).max_decimals(0).prefix("Out "));
                typing = [&one, &other].iter().any(|field| field.dragged() || field.has_focus());
                #[cfg(test)]
                {
                    self.fields = Some((one.rect, other.rect));
                }
            });
            // Only a number that was changed is taken: the other stays as it is, between levels.
            let was = points[place];
            let moved = [
                if going == level(was[0]) { was[0] } else { (going / 255.0).clamp(least, most.max(least)) },
                if coming == level(was[1]) { was[1] } else { coming / 255.0 },
            ];
            if moved != was {
                points[place] = moved;
                *shown.points_mut(self.channel) = points.clone();
                changed = true;
            }
        }
        (changed.then_some(shown), self.drag.is_some() || typing)
    }
}
