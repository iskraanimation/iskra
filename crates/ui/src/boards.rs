//! Panels in the window (plan §4.7): what is written of the panel under the playhead, in the
//! Shot Notes panel, and the storyboard read as cards, a card a panel, in the Panels view.
//!
//! Both are of the storyboard that is open, and ask for what they want done with the
//! storyboard timeline's actions: they hold nothing of the project themselves.

use std::collections::HashMap;

use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2};
use iskra_model::{Clip, Project, Shot, ShotId, Storyboard, Work};
use serde_json::{Value, json};

use crate::items::{Action, Playhead};
use crate::theme::{self, Theme};

/// The texts of a panel, as they are named in commands and on the panel.
const TEXTS: [(&str, &str, &str); 3] = [
    ("action", "Action", "What happens in this panel"),
    ("dialogue", "Dialogue", "What is spoken over it"),
    ("notes", "Notes", "Anything else: for the animator, the editor, the sound"),
];

/// How long `frames` frames last, as a card says it.
fn seconds(frames: u32, rate: f64) -> String {
    format!("{frames} fr · {:.1} s", f64::from(frames) / rate.max(1.0))
}

/// The commands that put `shot` at `place` among the storyboard's shots (0 is the first), for
/// `edit.batch` to run as one step, and where each shot they move starts then. The shots from
/// where it was to where it goes take each other's places in the same stretch of the film, and
/// the gaps between them stay where they were. None where it is there already.
pub fn reorder(board: &Storyboard, shot: ShotId, place: usize) -> Option<(Vec<Value>, Vec<(ShotId, u32)>)> {
    let clips = board.clips();
    let from = clips.iter().position(|clip| clip.shot == shot)?;
    let place = place.min(clips.len() - 1);
    if from == place {
        return None;
    }
    let (first, last) = (from.min(place), from.max(place));
    let gaps: Vec<u32> = clips[first..=last].windows(2).map(|pair| pair[1].start - pair[0].end()).collect();
    let mut order = clips[first..=last].to_vec();
    let moved = order.remove(from - first);
    order.insert(place - first, moved);
    let mut starts = Vec::new();
    let mut at = clips[first].start;
    for (slot, clip) in order.iter().enumerate() {
        if at != clip.start {
            starts.push((clip.shot, at));
        }
        at += clip.length() + gaps.get(slot).copied().unwrap_or(0);
    }
    // All of them out of the way first, past the end, so that none is put where another is.
    let to = |shot: ShotId, start: u32| json!({ "id": "storyboard.moveClip", "params": { "shot": shot.0, "start": start } });
    let mut aside = board.end() + 1;
    let mut commands = Vec::new();
    for (shot, _) in &starts {
        commands.push(to(*shot, aside));
        aside += board.clip(*shot).map_or(0, Clip::length);
    }
    commands.extend(starts.iter().map(|(shot, start)| to(*shot, *start)));
    Some((commands, starts))
}

/// The Shot Notes panel: what is written of the panel under the playhead.
#[derive(Default)]
pub struct NotesView {
    /// The panel whose texts are in the fields: its shot, and the frame it starts on.
    of: Option<(ShotId, u32)>,
    /// The texts as they are in the fields, and as the project had them when they were put there.
    texts: [String; 3],
    stored: [String; 3],
    /// One of the fields is being written in.
    writing: bool,
    /// Where the fields were drawn.
    #[cfg(test)]
    pub fields: Vec<Rect>,
}

impl NotesView {
    /// `frame` is the frame of `shot` under the playhead; `clip` its clip on the storyboard.
    pub fn show(&mut self, ui: &mut egui::Ui, shot: &Shot, clip: Option<Clip>, frame: u32, rate: f64) -> Vec<Action> {
        let mut actions = Vec::new();
        let panel = shot.panel_at(frame);
        let now = [panel.action.clone(), panel.dialogue.clone(), panel.notes.clone()];
        let key = (shot.id, panel.frame);
        if self.of != Some(key) {
            // The playhead left a panel that was being written of: what was written is kept.
            if let Some((shot, frame)) = self.of.filter(|_| self.writing) {
                for (at, (name, ..)) in TEXTS.iter().enumerate().filter(|(at, _)| self.texts[*at] != self.stored[*at]) {
                    actions.push(Action::Run("panel.set", json!({ "shot": shot.0, "frame": frame, *name: self.texts[at] })));
                }
            }
            (self.of, self.writing) = (Some(key), false);
        }
        // What is not being written in follows the project: an undo is seen at once.
        if !self.writing {
            (self.texts, self.stored) = (now.clone(), now);
        }

        let panels = shot.panels();
        let place = panels.iter().position(|other| other.frame == panel.frame).unwrap_or(0);
        let end = shot.panel_end(panel.frame);
        ui.horizontal_wrapped(|ui| {
            ui.strong(&shot.name);
            ui.weak(format!("panel {} of {} · {}", place + 1, panels.len(), seconds(end - panel.frame, rate)));
        });
        ui.horizontal(|ui| {
            let starts_here = shot.panel_on(frame).is_some();
            let hint = "A new panel that starts on the playhead's frame: from there on the shot shows another picture of the storyboard";
            if ui
                .add_enabled(!starts_here, egui::Button::new("+ Panel"))
                .on_hover_text(hint)
                .on_disabled_hover_text("A panel starts on this frame already")
                .clicked()
            {
                actions.push(Action::Run("panel.add", json!({ "shot": shot.id.0, "frame": frame })));
            }
            let hint = "Remove this panel with what is written of it. Nothing drawn changes: its frames are the panel's before it";
            let removed = ui.add_enabled(panel.frame > 0, egui::Button::new("Remove Panel")).on_hover_text(hint);
            if removed.on_disabled_hover_text("A shot's first panel is the shot's own").clicked() {
                actions.push(Action::Run("panel.remove", json!({ "shot": shot.id.0, "frame": panel.frame })));
            }
            // Where the panel is in the cut, for a shot that is trimmed.
            if let Some(clip) = clip.filter(|clip| panel.frame < clip.in_point || panel.frame >= clip.out_point) {
                ui.weak(if panel.frame < clip.in_point { "starts before the cut" } else { "after the cut" });
            }
        });
        ui.separator();
        #[cfg(test)]
        self.fields.clear();
        let mut writing = false;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (at, (name, title, hint)) in TEXTS.iter().enumerate() {
                ui.label(egui::RichText::new(*title).strong());
                let field = ui.add(egui::TextEdit::multiline(&mut self.texts[at]).desired_rows(3).desired_width(f32::INFINITY).hint_text(*hint));
                #[cfg(test)]
                self.fields.push(field.rect);
                writing |= field.has_focus();
                // Written when the field is left, as one step.
                if field.lost_focus() && self.texts[at] != self.stored[at] {
                    actions.push(Action::Run("panel.set", json!({ "shot": shot.id.0, "frame": panel.frame, *name: self.texts[at] })));
                    self.stored[at].clone_from(&self.texts[at]);
                }
                ui.add_space(4.0);
            }
        });
        self.writing = writing;
        actions
    }
}

/// How wide a card's picture is to begin with, in points, and how wide it can be made.
pub const CARD_WIDTH: f32 = 176.0;
pub const CARD_SIZES: std::ops::RangeInclusive<f32> = 80.0..=400.0;

/// The room under a card's picture for what is written of it: the shot's line, and two lines
/// each of what happens and what is said.
const CARD_TEXT: f32 = 92.0;

/// How many lines of a card one text has, before what is left of it is cut off with an
/// ellipsis.
const CARD_LINES: usize = 2;

/// How many pictures are made on one frame of the screen: the rest wait for the next.
const MADE_A_FRAME: usize = 2;

/// A card's picture, and what it is of: the shot's revision and the project's look; and how
/// wide it was made, in pixels.
struct Thumb {
    of: (u64, u64),
    texture: TextureHandle,
    width: f32,
}

/// The Panels view: the storyboard as cards in the order of the film, a card a panel, with its
/// picture, what is written of it and how long it lasts. A click goes to the panel; a card
/// dragged takes its shot, all of its cards, to where it is let go among the others.
#[derive(Default)]
pub struct CardsView {
    /// The shot whose card is being dragged, and the place among the shots it would go to.
    dragging: Option<(ShotId, Option<usize>)>,
    /// How wide a card's picture is, in points. The app says before the view is shown.
    pub width: f32,
    /// The pictures, by shot and by the frame of it they show. One is kept after its shot is
    /// let go from memory, and made anew when the shot is loaded and is another.
    thumbs: HashMap<(ShotId, u32), Thumb>,
    /// A shot whose cards are seen with no picture, and that is not in memory: the app reads
    /// it from the file, without loading it, for [`CardsView::make_of`].
    pub wanted: Option<ShotId>,
    /// How many pictures were made, as against found: for tests.
    #[cfg(test)]
    pub made: usize,
    /// Where the cards were drawn: the shot, the frame of it the card starts on, its place.
    #[cfg(test)]
    pub cards: Vec<(ShotId, u32, Rect)>,
    /// What each card's texts came to: the shot, the frame, how many lines, and whether the
    /// rest was cut off.
    #[cfg(test)]
    pub card_texts: Vec<(ShotId, u32, usize, bool)>,
}

/// `words` laid out on at most [`CARD_LINES`] lines `width` wide, in `font`: its own lines run
/// together, and what does not fit ends with an ellipsis.
fn card_lines(ui: &egui::Ui, words: &str, width: f32, font: FontId, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(words.split_whitespace().collect::<Vec<_>>().join(" "), font, color, width);
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: CARD_LINES, break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

/// The stretches of a shot's panels that are in the cut: for each, the frame it starts on in
/// the cut, the frame after its last there, and its place among the shot's panels.
fn in_cut(shot: &Shot, clip: &Clip) -> Vec<(usize, u32, u32)> {
    let stretches = shot.panels().into_iter().enumerate().filter_map(|(place, panel)| {
        let (start, end) = (panel.frame.max(clip.in_point), shot.panel_end(panel.frame).min(clip.out_point));
        (start < end).then_some((place, start, end))
    });
    stretches.collect()
}

impl CardsView {
    /// Make the picture of the card of `shot` that starts on `start`, of its boards.
    fn make(&mut self, ctx: &egui::Context, project: &Project, shot: &Shot, start: u32) {
        let width = self.width();
        let view = iskra_render::View::frame(&project.settings, f64::from(width) / f64::from(project.settings.width.max(1)));
        let made = iskra_render::render(&shot.only(Work::Boards), &project.palette, project.settings.working_space, start, &view);
        let pixels: Vec<u8> = made.pixels.iter().flatten().copied().collect();
        let image = egui::ColorImage::from_rgba_unmultiplied([made.width as usize, made.height as usize], &pixels);
        let texture = ctx.load_texture("panel", image, TextureOptions::LINEAR);
        self.thumbs.insert((shot.id, start), Thumb { of: (shot.revision, project.look), texture, width });
        #[cfg(test)]
        {
            self.made += 1;
        }
    }

    /// How wide a card's picture is: as the app says, within the sizes it can be.
    fn width(&self) -> f32 {
        if self.width > 0.0 { self.width.clamp(*CARD_SIZES.start(), *CARD_SIZES.end()) } else { CARD_WIDTH }
    }

    /// Where among the shots the one being dragged would go, by the card under the pointer: the
    /// pointer in the first half of that shot's cards, before it, in the second, after it. The
    /// cards side by side, the halves are across them, in a column, down them. A line in
    /// `color` is drawn there. Near the top or the foot of the view it scrolls.
    fn drop_place(&self, ui: &mut egui::Ui, board: &Storyboard, dragged: ShotId, laid: &[(usize, usize, usize, Rect)], color: Color32) -> Option<usize> {
        ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        let pointer = ui.ctx().pointer_latest_pos()?;
        let seen = ui.clip_rect();
        let edge = 24.0;
        let by = if pointer.y < seen.top() + edge {
            8.0
        } else if pointer.y > seen.bottom() - edge {
            -8.0
        } else {
            0.0
        };
        if by != 0.0 {
            ui.scroll_with_delta(vec2(0.0, by));
            ui.ctx().request_repaint();
        }
        let from = board.clips().iter().position(|clip| clip.shot == dragged)?;
        // The cards are 8 points apart: half of that on each side is the card's.
        let &(order, card, count, rect) = laid.iter().find(|(.., rect)| rect.expand(4.0).contains(pointer))?;
        let across = laid.windows(2).any(|pair| pair[0].3.top() == pair[1].3.top());
        let into = if across { (pointer.x - rect.left()) / rect.width() } else { (pointer.y - rect.top()) / rect.height() };
        let before = (card as f32 + into) < count as f32 / 2.0;
        let slot = if before { order } else { order + 1 };
        let to = if slot > from { slot - 1 } else { slot };
        if to == from {
            return None;
        }
        // The line is before the shot's first card, or after its last.
        let cards = || laid.iter().filter(|(of, ..)| *of == order).map(|(.., rect)| *rect);
        let rect = if before { cards().next()? } else { cards().next_back()? };
        let line = match (across, before) {
            (true, true) => [rect.left_top() - vec2(4.0, 0.0), rect.left_bottom() - vec2(4.0, 0.0)],
            (true, false) => [rect.right_top() + vec2(4.0, 0.0), rect.right_bottom() + vec2(4.0, 0.0)],
            (false, true) => [rect.left_top() - vec2(0.0, 4.0), rect.right_top() - vec2(0.0, 4.0)],
            (false, false) => [rect.left_bottom() + vec2(0.0, 4.0), rect.right_bottom() + vec2(0.0, 4.0)],
        };
        ui.painter().line_segment(line, Stroke::new(3.0, color));
        Some(to)
    }

    /// Make the pictures of all the cards of a shot that is not in memory, from `read`, the
    /// shot with its drawings read from the file.
    pub fn make_of(&mut self, ctx: &egui::Context, project: &Project, board: &Storyboard, read: &Shot) {
        let Some(clip) = board.clip(read.id) else { return };
        for (_, start, _) in in_cut(read, clip) {
            self.make(ctx, project, read, start);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, project: &Project, board: &Storyboard, at: Playhead) -> Vec<Action> {
        let theme = Theme::of(ui.ctx());
        let mut actions = Vec::new();
        // The cards as they were laid: the shot's place on the storyboard, the card's among the
        // shot's and how many it has, and the card's place on the screen.
        let mut laid: Vec<(usize, usize, usize, Rect)> = Vec::new();
        let mut dropped = false;
        let rate = project.settings.frame_rate.as_f64();
        let aspect = project.settings.width.max(1) as f32 / project.settings.height.max(1) as f32;
        let width = self.width();
        let picture = vec2(width, (width / aspect).round());
        #[cfg(test)]
        {
            self.cards.clear();
            self.card_texts.clear();
        }
        let mut making = MADE_A_FRAME;
        let mut waiting = false;
        if board.clips().is_empty() {
            ui.weak("The storyboard has no shots yet.");
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for (order, clip) in board.clips().iter().enumerate() {
                    let Some(shot) = project.shot(clip.shot) else { continue };
                    let panels = shot.panels();
                    let stretches = in_cut(shot, clip);
                    let count = stretches.len();
                    for (card, (place, start, end)) in stretches.into_iter().enumerate() {
                        let (rect, response) = ui.allocate_exact_size(picture + vec2(8.0, 8.0 + CARD_TEXT), Sense::click_and_drag());
                        laid.push((order, card, count, rect));
                        #[cfg(test)]
                        self.cards.push((shot.id, start, rect));
                        if response.drag_started() {
                            self.dragging = Some((shot.id, None));
                        }
                        dropped |= response.drag_stopped();
                        let film = clip.start + start - clip.in_point;
                        let under = at.shot == Some(shot.id) && (film..film + end - start).contains(&at.frame);
                        if under || response.hovered() {
                            ui.painter().rect_filled(rect, 4.0, if under { theme.accent.gamma_multiply(0.35) } else { theme.surface_raised });
                        }
                        let paper = Rect::from_min_size(rect.min + vec2(4.0, 4.0), picture);
                        ui.painter().rect(paper, 0.0, Color32::WHITE, Stroke::new(1.0, theme.border), egui::StrokeKind::Outside);
                        // The picture is of the boards, as the page shows them. It is made
                        // when the shot's drawings are in memory, and kept when they are let go.
                        let of = (shot.revision, project.look);
                        // One made smaller than the card is now is made again; a larger one does.
                        let stale = self.thumbs.get(&(shot.id, start)).is_none_or(|thumb| thumb.of != of || thumb.width < width);
                        if stale && ui.is_rect_visible(rect) {
                            // A shot not in memory is read from the file by the app, a shot at a time.
                            if !shot.loaded {
                                self.wanted = self.wanted.or(Some(shot.id));
                                waiting = true;
                            } else if making > 0 {
                                making -= 1;
                                self.make(ui.ctx(), project, shot, start);
                            } else {
                                waiting = true;
                            }
                        }
                        if let Some(thumb) = self.thumbs.get(&(shot.id, start)) {
                            ui.painter().image(thumb.texture.id(), paper, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        // Where the panel starts in the film, over the picture's corner.
                        let at = iskra_model::timecode(i64::from(film), project.settings.frame_rate);
                        let corner = paper.left_bottom() + vec2(3.0, -3.0);
                        let said = ui.painter().layout_no_wrap(at, theme::mono(9.0), Color32::WHITE);
                        let chip = Rect::from_min_size(corner - vec2(0.0, said.size().y + 2.0), said.size() + vec2(4.0, 2.0));
                        ui.painter().rect_filled(chip, 2.0, Color32::from_black_alpha(150));
                        ui.painter().galley(chip.min + vec2(2.0, 1.0), said, Color32::WHITE);
                        // Under the picture: which panel of which shot and how long, then what
                        // happens and what is said.
                        let text = ui.painter().with_clip_rect(rect.shrink(3.0));
                        let mut line = paper.left_bottom() + vec2(0.0, 4.0);
                        let head = if panels.len() > 1 { format!("{} · {}/{}", shot.name, place + 1, panels.len()) } else { shot.name.clone() };
                        text.text(line, Align2::LEFT_TOP, head, FontId::proportional(11.0), theme.text);
                        text.text(pos2(paper.right(), line.y), Align2::RIGHT_TOP, seconds(end - start, rate), theme::mono(10.0), theme.text_muted);
                        let panel = &panels[place];
                        line.y += 15.0;
                        for (words, color, quoted) in [(&panel.action, theme.text, false), (&panel.dialogue, theme.text_muted, true)] {
                            let words = words.trim();
                            let shown = if quoted && !words.is_empty() { format!("“{words}”") } else { words.to_string() };
                            let laid = card_lines(ui, &shown, paper.width(), FontId::proportional(11.0), color);
                            #[cfg(test)]
                            self.card_texts.push((shot.id, start, laid.rows.len(), laid.elided));
                            // Two lines' room whatever is written, so the cards line up.
                            let room = laid.rows.first().map_or(13.0, |row| row.height()) * CARD_LINES as f32 + 2.0;
                            text.galley(line, laid, color);
                            line.y += room;
                        }
                        let said = [panel.action.as_str(), panel.dialogue.as_str(), panel.notes.as_str()];
                        let hint = said.iter().filter(|text| !text.is_empty()).copied().collect::<Vec<_>>().join("\n\n");
                        let response = if hint.is_empty() { response } else { response.on_hover_text(hint) };
                        if response.clicked() {
                            actions.push(Action::Seek(film));
                        }
                        if response.double_clicked() {
                            actions.push(Action::OpenShot(shot.id));
                        }
                        // The cards of the shot being dragged are dimmed while it is.
                        if self.dragging.is_some_and(|(dragged, _)| dragged == shot.id) {
                            ui.painter().rect_filled(rect, 4.0, theme.surface.gamma_multiply(0.6));
                        }
                    }
                }
            });
            if let Some((dragged, _)) = self.dragging {
                let to = self.drop_place(ui, board, dragged, &laid, theme.accent);
                self.dragging = Some((dragged, to));
            }
        });
        if let Some((shot, to)) = self.dragging.filter(|_| dropped) {
            if let Some(to) = to {
                actions.push(Action::Reorder(shot, to));
            }
            self.dragging = None;
        } else if self.dragging.is_some() && !ui.input(|i| i.pointer.any_down()) {
            self.dragging = None;
        }
        // Pictures of shots that are gone are let go.
        self.thumbs.retain(|(shot, frame), _| project.shot(*shot).is_some_and(|shot| *frame < shot.duration));
        if waiting {
            ui.ctx().request_repaint();
        }
        actions
    }
}
