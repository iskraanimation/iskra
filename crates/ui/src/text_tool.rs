//! The Text tool (plan §4.19, step 2): boxes of text put on a text layer's drawing and typed in.
//!
//! A click where there is no text puts a box there with the cursor in it, to type at once; a
//! drag makes a box of the width dragged, which its lines wrap in. A click on a box goes into it
//! with the cursor where it was clicked, a double-click picks the word there, and a drag in the
//! box being typed in picks letters. The keys type, delete and move the cursor as in any text
//! field: the letters go to a text field of egui's that is never seen, which has the keyboard
//! while a box is typed in, so that the app's shortcuts leave them alone. What the field holds
//! is the box's text; each change of it is a `text.type`. Up and Down go by the box's own lines.
//! Escape, or a click elsewhere in the view, lets the box go; a box let go with nothing in it is
//! deleted.
//!
//! The box typed in has handles at its corners and the middles of its sides, as the Transform
//! tool's box has: dragged, they set its width, which its lines wrap in, and its height; a drag
//! on its outline between them moves it. A new box is drawn as it is dragged out.

use egui::text::CCursor;
use egui::text_selection::CCursorRange;
use egui::{Color32, Key, Pos2, Response, Stroke as Outline};
use iskra_drawing::Laid;
use iskra_drawing::fonts::Fonts;
use iskra_model::kurbo::{self, Point};
use iskra_model::{DrawingId, Layer, LayerId, SwatchId, TextBox, TextFormat};
use serde_json::{Value, json};

use crate::points::Map;

/// How far, in the drawing's pixels, round a box a click still goes into it.
const MARGIN: f64 = 4.0;

/// The box being typed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Editing {
    pub layer: LayerId,
    pub drawing: DrawingId,
    pub index: usize,
}

/// A press of the tool, until it is let go: where, on screen and in the drawing, and the box and
/// place in it it was on, or the handle of the box typed in it took.
#[derive(Clone, Copy, Debug)]
struct Press {
    from: Pos2,
    at: Point,
    on: Option<(usize, usize)>,
    grab: Option<Grab>,
}

/// A handle of the box typed in, or its outline, taken by a press: which sides of the box it
/// moves, across and down, each -1 for the left or top, 1 for the right or bottom, 0 for
/// neither; both 0 is the outline, which moves the box. With the box as it was, its corners
/// in the drawing's pixels.
#[derive(Clone, Copy, Debug)]
struct Grab {
    sides: [i8; 2],
    was: [f64; 4],
}

impl Grab {
    /// The box as the drag has made it, by `by` in the drawing's pixels: no smaller than `least`.
    fn dragged(&self, by: kurbo::Vec2, least: f64) -> [f64; 4] {
        let [mut x0, mut y0, mut x1, mut y1] = self.was;
        match self.sides {
            [0, 0] => return [x0 + by.x, y0 + by.y, x1 + by.x, y1 + by.y],
            [across, down] => {
                match across {
                    -1 => x0 = (x0 + by.x).min(x1 - least),
                    1 => x1 = (x1 + by.x).max(x0 + least),
                    _ => {}
                }
                match down {
                    -1 => y0 = (y0 + by.y).min(y1 - least),
                    1 => y1 = (y1 + by.y).max(y0 + least),
                    _ => {}
                }
            }
        }
        [x0, y0, x1, y1]
    }
}

/// The handles of a box whose corners are `[x0, y0, x1, y1]`: each with the sides it moves.
fn handles([x0, y0, x1, y1]: [f64; 4]) -> [(Point, [i8; 2]); 8] {
    let at = |across: i8, down: i8| {
        let x = [x0, (x0 + x1) / 2.0, x1][(across + 1) as usize];
        let y = [y0, (y0 + y1) / 2.0, y1][(down + 1) as usize];
        (Point::new(x, y), [across, down])
    };
    [at(-1, -1), at(0, -1), at(1, -1), at(1, 0), at(1, 1), at(0, 1), at(-1, 1), at(-1, 0)]
}

/// How near on screen, in points, a press is to the outline of the box typed in to move it.
const OUTLINE_REACH: f32 = 4.0;

/// How far `at` is from the line from `a` to `b`, on screen.
fn off_line(at: Pos2, a: Pos2, b: Pos2) -> f32 {
    let way = b - a;
    let part = if way.length_sq() > 0.0 { ((at - a).dot(way) / way.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + way * part - at).length()
}

#[derive(Default)]
pub struct TextTool {
    pub editing: Option<Editing>,
    press: Option<Press>,
    /// What the unseen field holds: the edited box's text, as the project has it or as typed
    /// this frame.
    buffer: String,
    /// The next change typed is a step of the history of its own.
    fresh: bool,
    /// The place in the text to put the cursor at, or the letters to pick, once the field has
    /// the box's text.
    put_cursor: Option<(usize, usize)>,
    /// A box `text.add` was asked for, to type in once it is made.
    pub adding: bool,
    /// When and where the last click was, to tell a double-click by.
    last_click: Option<(f64, Pos2)>,
    /// The box being dragged, its handles or its outline, or dragged out new: its corners in the
    /// drawing's pixels, as the drag has them, to be drawn.
    live: Option<[f64; 4]>,
    /// The format a new box is made in, its size in the shot's pixels (plan §4.19): the box's
    /// is in its drawing's, as much larger as its layer's density. Its swatch is the one in hand.
    pub format: TextFormat,
}

/// How soon after a click, in seconds, a second at the same place is a double-click.
const DOUBLE_CLICK: f64 = 0.4;

/// The fonts texts are laid out in.
pub fn fonts() -> &'static Fonts {
    Fonts::system()
}

/// A box laid out.
fn laid(text: &TextBox) -> Laid {
    iskra_drawing::lay_out(text, fonts())
}

/// The corners of a box in the drawing's pixels, clockwise from its top left: as wide as its
/// width, or its text, and no narrower than a little of its size.
fn corners(text: &TextBox, laid: &Laid) -> [Point; 4] {
    let width = text.width.unwrap_or(laid.size[0]).max(text.format.size * 0.25);
    let height = laid.size[1].max(text.format.size).max(text.height.unwrap_or(0.0));
    let [x, y] = text.at;
    [Point::new(x, y), Point::new(x + width, y), Point::new(x + width, y + height), Point::new(x, y + height)]
}

/// The corners of a box, at its top left and bottom right: `[x0, y0, x1, y1]`.
fn rect_of(text: &TextBox, laid: &Laid) -> [f64; 4] {
    let [top_left, _, bottom_right, _] = corners(text, laid);
    [top_left.x, top_left.y, bottom_right.x, bottom_right.y]
}

/// How far outside its letters, on screen in points, a box's frame and handles are drawn and
/// taken: clear of the cursor at the start or the end of a line, at any zoom.
pub const FRAME_GAP: f32 = 6.0;

/// The frame of the box `[x0, y0, x1, y1]`, as it is drawn: [`FRAME_GAP`] outside it.
fn framed([x0, y0, x1, y1]: [f64; 4], map: &Map) -> [f64; 4] {
    let scale = ((map.to_screen)(Point::new(1.0, 0.0)) - (map.to_screen)(Point::ZERO)).length();
    let gap = if scale > 0.0 { f64::from(FRAME_GAP / scale) } else { 0.0 };
    [x0 - gap, y0 - gap, x1 + gap, y1 + gap]
}

/// The four corners of `[x0, y0, x1, y1]`, clockwise from the top left.
fn four([x0, y0, x1, y1]: [f64; 4]) -> [Point; 4] {
    [Point::new(x0, y0), Point::new(x1, y0), Point::new(x1, y1), Point::new(x0, y1)]
}

/// Whether the drawing's point `at` is on the box, or near it.
fn on_box(text: &TextBox, laid: &Laid, at: Point) -> bool {
    let [top_left, _, bottom_right, _] = corners(text, laid);
    (top_left.x - MARGIN..=bottom_right.x + MARGIN).contains(&at.x) && (top_left.y - MARGIN..=bottom_right.y + MARGIN).contains(&at.y)
}

/// The places in `text` where the word round the place `at` starts and ends.
fn word_at(text: &str, at: usize) -> (usize, usize) {
    let letters: Vec<char> = text.chars().collect();
    let in_word = |i: usize| letters.get(i).is_some_and(|c| c.is_alphanumeric() || *c == '\'');
    let (mut from, mut to) = (at.min(letters.len()), at.min(letters.len()));
    while from > 0 && in_word(from - 1) {
        from -= 1;
    }
    while in_word(to) {
        to += 1;
    }
    (from, to)
}

/// What changed from `was` to `now`: the letters from one place up to another that were
/// replaced, and by what.
fn changed(was: &str, now: &str) -> (usize, usize, String) {
    let (was, now): (Vec<char>, Vec<char>) = (was.chars().collect(), now.chars().collect());
    let same_start = was.iter().zip(&now).take_while(|(a, b)| a == b).count();
    let same_end = was.iter().rev().zip(now.iter().rev()).take_while(|(a, b)| a == b).count().min(was.len().min(now.len()) - same_start);
    (same_start, was.len() - same_end, now[same_start..now.len() - same_end].iter().collect())
}

impl TextTool {
    fn field_id(_: &egui::Ui) -> egui::Id {
        Self::field_id_of()
    }

    fn field_id_of() -> egui::Id {
        egui::Id::new("text tool field")
    }

    /// The id of the unseen field the keys go to while a box is typed in.
    pub fn field(&self) -> egui::Id {
        Self::field_id_of()
    }

    /// The box being typed in, if it is still on the drawing exposed on the frame.
    fn edited<'a>(&self, layer: &'a Layer, frame: u32) -> Option<&'a TextBox> {
        let editing = self.editing?;
        let exposed = layer.drawing_at(frame).filter(|drawing| layer.id == editing.layer && drawing.id == editing.drawing)?;
        exposed.drawing.texts.get(editing.index)
    }

    /// Start typing in a box, with the cursor at `from` to `to`.
    pub fn edit(&mut self, editing: Editing, from: usize, to: usize) {
        (self.editing, self.fresh, self.put_cursor, self.adding) = (Some(editing), true, Some((from, to)), false);
        self.buffer.clear();
    }

    /// Stop typing: the command that deletes the box, if nothing is in it.
    pub fn let_go(&mut self, layer: &Layer) -> Option<(&'static str, Value)> {
        let editing = self.editing.take()?;
        self.press = None;
        let empty = layer.id == editing.layer
            && layer.drawing(editing.drawing).and_then(|d| d.drawing.texts.get(editing.index)).is_some_and(|text| text.text.is_empty());
        empty.then(|| ("text.delete", json!({ "layer": editing.layer.0, "drawing": editing.drawing.0, "box": editing.index })))
    }

    /// The tool's input on the text layer `layer` at `frame`, a new box made in the swatch
    /// `swatch`: the command it asks for.
    pub fn input(
        &mut self,
        ui: &mut egui::Ui,
        response: &Response,
        layer: &Layer,
        frame: u32,
        map: &Map,
        swatch: SwatchId,
        panning: bool,
    ) -> Option<(&'static str, Value)> {
        let format = TextFormat { swatch, size: self.format.size * layer.density, ..self.format.clone() };
        let exposed = layer.drawing_at(frame);
        // The box went from the frame, by a step of the playhead or an undo: it is let go.
        if self.editing.is_some() && self.edited(layer, frame).is_none() && !self.adding {
            self.editing = None;
        }
        let boxes: Vec<(TextBox, Laid)> = exposed.map(|d| d.drawing.texts.iter().map(|text| (text.clone(), laid(text))).collect()).unwrap_or_default();
        let place_in = |index: usize, at: Point| boxes.get(index).map_or(0, |(_, laid)| laid.place_at(at.x, at.y));
        let mut asked = None;
        let pointer = response.interact_pointer_pos();
        let moved = |press: &Press, now: Pos2| (now - press.from).length() >= crate::viewport::CLICK_SLOP;
        if response.drag_started_by(egui::PointerButton::Primary)
            && !panning
            && let Some(from) = pointer
        {
            let at = (map.to_drawing)(from);
            // A handle of the box typed in, or its outline, before what is inside.
            let edited = self.editing.and_then(|e| boxes.get(e.index)).map(|(text, laid)| rect_of(text, laid));
            let grab = edited.and_then(|was| {
                let frame = framed(was, map);
                let handle = handles(frame).into_iter().find(|(point, _)| ((map.to_screen)(*point) - from).length() <= crate::viewport::HANDLE_REACH);
                let screen = four(frame).map(|corner| (map.to_screen)(corner));
                let outline = (0..4).any(|i| off_line(from, screen[i], screen[(i + 1) % 4]) <= OUTLINE_REACH);
                handle.map(|(_, sides)| sides).or(outline.then_some([0, 0])).map(|sides| Grab { sides, was })
            });
            // The box being typed in first, then the top one there.
            let in_edited = self.editing.map(|e| e.index).filter(|i| boxes.get(*i).is_some_and(|(text, laid)| on_box(text, laid, at)));
            let on = in_edited.or_else(|| boxes.iter().rposition(|(text, laid)| on_box(text, laid, at)));
            self.press = Some(Press { from, at, on: on.map(|index| (index, place_in(index, at))), grab });
        }
        // The box, or the new one, as it is dragged.
        self.live = match (self.press, pointer.filter(|_| response.is_pointer_button_down_on())) {
            (Some(press), Some(now)) if moved(&press, now) => match press.grab {
                Some(grab) => Some(grab.dragged((map.to_drawing)(now) - press.at, MARGIN * 2.0)),
                None if press.on.is_none() => {
                    let to = (map.to_drawing)(now);
                    Some([press.at.x.min(to.x), press.at.y.min(to.y), press.at.x.max(to.x), press.at.y.max(to.y)])
                }
                None => None,
            },
            _ => None,
        };
        // A drag in the box being typed in picks letters as it goes.
        if let (Some(press), Some(now), Some(editing)) = (self.press, pointer, self.editing)
            && press.grab.is_none()
            && response.dragged()
            && moved(&press, now)
            && let Some((index, from)) = press.on.filter(|(index, _)| *index == editing.index)
        {
            self.put_cursor = Some((from, place_in(index, (map.to_drawing)(now))));
        }
        if !response.is_pointer_button_down_on()
            && let Some(press) = self.press.take()
        {
            let now = pointer.unwrap_or(press.from);
            let drawing = exposed.map(|d| d.id);
            let time = ui.input(|i| i.time);
            let double = !moved(&press, now) && self.last_click.is_some_and(|(at, place)| time - at < DOUBLE_CLICK && (now - place).length() < 6.0);
            self.last_click = (!moved(&press, now) && !double).then_some((time, now));
            match press.on {
                // A handle or the outline dragged: the box moved, or its sides.
                _ if press.grab.is_some() && moved(&press, now) => {
                    let (grab, editing) = (press.grab.expect("grabbed"), self.editing.expect("a handle is the box typed in's"));
                    let [x0, y0, x1, y1] = grab.dragged((map.to_drawing)(now) - press.at, MARGIN * 2.0);
                    let mut params = json!({ "layer": editing.layer.0, "drawing": editing.drawing.0, "box": editing.index, "at": [x0, y0] });
                    if grab.sides[0] != 0 {
                        params["width"] = json!(x1 - x0);
                    }
                    if grab.sides[1] != 0 {
                        params["height"] = json!(y1 - y0);
                    }
                    asked = Some(("text.setBox", params));
                }
                // Into a box, with the cursor where it was clicked, or the word there picked.
                Some((index, place)) if !moved(&press, now) => {
                    let (from, to) = if double { word_at(&boxes[index].0.text, place) } else { (place, place) };
                    let into = Editing { layer: layer.id, drawing: drawing.expect("a box is on a drawing"), index };
                    if self.editing == Some(into) {
                        self.put_cursor = Some((from, to));
                    } else {
                        asked = self.let_go(layer);
                        self.edit(into, from, to);
                    }
                }
                // Letters picked by the drag, done as it went.
                Some((index, _)) if self.editing.is_some_and(|e| e.index == index) => {}
                // A new box: where it was clicked, or as wide as it was dragged.
                _ => {
                    let to = (map.to_drawing)(now);
                    let dragged = moved(&press, now);
                    let width = dragged.then(|| (to.x - press.at.x).abs()).filter(|width| *width > 1.0);
                    let at = if dragged { [press.at.x.min(to.x), press.at.y.min(to.y)] } else { [press.at.x, press.at.y] };
                    match self.let_go(layer) {
                        // An empty box let go is deleted first; the click makes the next.
                        Some(delete) => asked = Some(delete),
                        None => {
                            self.adding = true;
                            let mut params = json!({ "layer": layer.id.0, "frame": frame, "at": at, "format": format });
                            if let Some(width) = width {
                                params["width"] = json!(width);
                            }
                            asked = Some(("text.add", params));
                        }
                    }
                }
            }
        }
        if asked.is_none() {
            asked = self.typing(ui, layer, frame, &boxes);
        }
        if response.hovered() && !panning {
            // Over a handle, the way it drags; over the outline, a move.
            let edited = self.editing.and_then(|e| boxes.get(e.index)).map(|(text, laid)| rect_of(text, laid));
            let hover = response.hover_pos();
            let over = edited.zip(hover).and_then(|(was, at)| {
                let handle = handles(was).into_iter().find(|(point, _)| ((map.to_screen)(*point) - at).length() <= crate::viewport::HANDLE_REACH);
                let screen = four(was).map(|corner| (map.to_screen)(corner));
                let outline = (0..4).any(|i| off_line(at, screen[i], screen[(i + 1) % 4]) <= OUTLINE_REACH);
                handle.map(|(_, sides)| sides).or(outline.then_some([0, 0]))
            });
            let icon = match over.or(self.press.and_then(|press| press.grab.map(|grab| grab.sides))) {
                Some([0, 0]) => egui::CursorIcon::Move,
                Some([_, 0]) => egui::CursorIcon::ResizeHorizontal,
                Some([0, _]) => egui::CursorIcon::ResizeVertical,
                Some([across, down]) if across == down => egui::CursorIcon::ResizeNwSe,
                Some(_) => egui::CursorIcon::ResizeNeSw,
                None => egui::CursorIcon::Text,
            };
            ui.ctx().set_cursor_icon(icon);
        }
        asked
    }

    /// The keys, while a box is typed in: the unseen field, and the change it makes.
    fn typing(&mut self, ui: &mut egui::Ui, layer: &Layer, frame: u32, boxes: &[(TextBox, Laid)]) -> Option<(&'static str, Value)> {
        let editing = self.editing?;
        let (text, laid) = boxes.get(editing.index).filter(|_| self.edited(layer, frame).is_some())?;
        let id = Self::field_id(ui);
        // The box as the project has it: after an undo, or what was typed last frame.
        if self.buffer != text.text {
            self.buffer = text.text.clone();
        }
        // While a field elsewhere has the keys, the Text panel's, the box waits for them: it is
        // still the one its format is set for.
        let elsewhere = ui.memory(|m| m.focused()).is_some_and(|focused| focused != id);
        // Escape lets the box go, as a click elsewhere in the view does.
        if ui.input(|i| i.key_pressed(Key::Escape)) && !elsewhere {
            return self.let_go(layer);
        }
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        if let Some((from, to)) = self.put_cursor.take() {
            state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(from), CCursor::new(to))));
        }
        // Up and Down go by the box's lines as they are laid out, which the field does not know.
        let picking = ui.input(|i| i.modifiers.shift);
        for (key, by) in [(Key::ArrowUp, -1), (Key::ArrowDown, 1)] {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key) || i.consume_key(egui::Modifiers::SHIFT, key)) {
                let range = state.cursor.char_range().unwrap_or_default();
                if let Some((line, x)) = laid.caret(range.primary.index.into()) {
                    let to = (line as i64 + by).clamp(0, laid.lines.len() as i64 - 1) as usize;
                    let row = &laid.lines[to];
                    let place = laid.place_at(x, row.top + row.height / 2.0);
                    let anchor = if picking { range.secondary } else { CCursor::new(place) };
                    state.cursor.set_char_range(Some(CCursorRange { primary: CCursor::new(place), secondary: anchor, h_pos: None }));
                }
            }
        }
        state.store(ui.ctx(), id);
        // Never seen: clipped away, it still has the keyboard.
        ui.scope(|ui| {
            ui.set_clip_rect(egui::Rect::NOTHING);
            let field = egui::TextEdit::multiline(&mut self.buffer).id(id).frame(egui::Frame::NONE).desired_rows(1).desired_width(1.0).lock_focus(true);
            let shown = ui.put(egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(1.0, 1.0)), field);
            if !elsewhere {
                shown.request_focus();
                // The arrows and Tab are the box's: egui would take the keys to another widget.
                let keys = egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: false };
                ui.memory_mut(|m| m.set_focus_lock_filter(id, keys));
            }
        });
        if self.buffer == text.text {
            return None;
        }
        let (from, to, insert) = changed(&text.text, &self.buffer);
        let new_step = std::mem::take(&mut self.fresh);
        Some((
            "text.type",
            json!({ "layer": editing.layer.0, "drawing": editing.drawing.0, "box": editing.index, "from": from, "to": to, "insert": insert, "newStep": new_step }),
        ))
    }

    /// The cursor's places in the box being typed in: where it is, and where what is picked
    /// starts, from 0.
    pub fn cursor(&self, ctx: &egui::Context, id: egui::Id) -> Option<(usize, usize)> {
        let range = egui::text_edit::TextEditState::load(ctx, id)?.cursor.char_range()?;
        Some((range.primary.index.into(), range.secondary.index.into()))
    }

    /// The boxes of the drawing on the frame, the one typed in with its cursor and what is
    /// picked in it.
    pub fn paint(&self, ui: &egui::Ui, painter: &egui::Painter, layer: &Layer, frame: u32, map: &Map, accent: Color32) {
        let Some(exposed) = layer.drawing_at(frame) else { return };
        let to_screen = |at: Point| (map.to_screen)(at);
        let editing = self.editing.filter(|e| e.layer == layer.id && e.drawing == exposed.id);
        let hover = ui.input(|i| i.pointer.hover_pos()).map(|at| (map.to_drawing)(at));
        // The handles, at the corners and the middles of the sides, as the Transform tool's.
        let handled = |rect: [f64; 4]| {
            let rect = framed(rect, map);
            painter.add(egui::Shape::closed_line(four(rect).map(to_screen).to_vec(), Outline::new(1.0, accent)));
            for (point, _) in handles(rect) {
                let square = egui::Rect::from_center_size(to_screen(point), egui::vec2(7.0, 7.0));
                painter.rect(square, 0.0, accent, Outline::new(1.0, Color32::from_gray(30)), egui::StrokeKind::Middle);
            }
        };
        // A new box being dragged out, before it is made.
        if let Some(rect) = self.live.filter(|_| self.press.is_some_and(|press| press.grab.is_none())) {
            painter.add(egui::Shape::closed_line(four(rect).map(to_screen).to_vec(), Outline::new(1.0, accent)));
        }
        for (index, text) in exposed.drawing.texts.iter().enumerate() {
            let laid = laid(text);
            let typed_in = editing.is_some_and(|e| e.index == index);
            let hovered = hover.is_some_and(|at| on_box(text, &laid, at));
            if !typed_in && !hovered {
                continue;
            }
            if !typed_in {
                let frame = four(framed(rect_of(text, &laid), map));
                painter.add(egui::Shape::closed_line(frame.map(to_screen).to_vec(), Outline::new(1.0, accent.gamma_multiply(0.5))));
                continue;
            }
            // Where the drag has it, while its handles or its outline are dragged.
            let grabbed = self.press.is_some_and(|press| press.grab.is_some());
            handled(self.live.filter(|_| grabbed).unwrap_or_else(|| rect_of(text, &laid)));
            let Some((at, anchor)) = self.cursor(ui.ctx(), Self::field_id(ui)) else { continue };
            let (from, to) = (at.min(anchor), at.max(anchor));
            for line in &laid.lines {
                let end = line.start + line.carets.len() - 1;
                if to > from && from <= end && to > line.start {
                    let (a, b) = (line.carets[from.saturating_sub(line.start)], line.carets[(to - line.start).min(line.carets.len() - 1)]);
                    let quad = [Point::new(a, line.top), Point::new(b, line.top), Point::new(b, line.top + line.height), Point::new(a, line.top + line.height)];
                    painter.add(egui::Shape::convex_polygon(quad.map(to_screen).to_vec(), accent.gamma_multiply(0.35), Outline::NONE));
                }
            }
            // The cursor blinks, half a second on and half off, as a text field's does.
            let time = ui.input(|i| i.time);
            if let Some((line, x)) = laid.caret(at)
                && time % 1.0 < 0.5
            {
                let row = &laid.lines[line];
                painter.line_segment([to_screen(Point::new(x, row.top)), to_screen(Point::new(x, row.top + row.height))], Outline::new(1.5, accent));
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_found_as_the_letters_it_replaced() {
        assert_eq!(changed("Hello", "Hello!"), (5, 5, "!".to_string()));
        assert_eq!(changed("Hello", "Jello"), (0, 1, "J".to_string()));
        assert_eq!(changed("Hello", "Helo"), (3, 4, String::new()), "one of two alike letters goes");
        assert_eq!(changed("aaa", "aaaa"), (3, 3, "a".to_string()));
        assert_eq!(changed("", "é"), (0, 0, "é".to_string()));
    }

    #[test]
    fn a_double_click_picks_the_word() {
        assert_eq!(word_at("one two three", 5), (4, 7));
        assert_eq!(word_at("one two", 3), (0, 3));
        assert_eq!(word_at("", 0), (0, 0));
    }
}
