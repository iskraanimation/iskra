//! The drawing substitution picker (plan §4.13): every drawing of the selected layer as a
//! thumbnail. Clicking one shows it on the current frame, which is how a mouth is given its
//! shapes along a line of dialogue, or a hand another pose. A thumbnail's menu renames the
//! drawing or deletes it.
//!
//! The thumbnails are framed alike, around everything painted on any of the layer's drawings,
//! so the shapes can be told apart by where they are as well as by what they are.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Color32, Rect, Sense, Stroke, TextureHandle, TextureOptions, vec2};
use iskra_drawing::{Drawing, Rect as PixelRect};
use iskra_model::{DrawingId, Layer, Palette};

use crate::theme::Theme;

/// The side of a thumbnail's picture, in points and in pixels.
const SIDE: usize = 64;

/// The picture of one drawing under one palette in one framing. It holds the drawing and the
/// palette so that their addresses, which are in the key, can't be reused while it lives.
struct Thumb {
    texture: TextureHandle,
    used: bool,
    _drawing: Arc<Drawing>,
    _palette: Arc<Palette>,
}

/// What was clicked in the picker.
#[derive(Clone, Debug, PartialEq)]
pub enum Picked {
    Drawing(DrawingId),
    /// The tile after the drawings: a new empty drawing on the frame.
    New,
    /// A drawing given a new name, typed under its thumbnail.
    Rename(DrawingId, String),
    /// A drawing deleted by its thumbnail's menu.
    Delete(DrawingId),
}

#[derive(Default)]
pub struct Substitution {
    thumbs: HashMap<(usize, usize, [i32; 4]), Thumb>,
    /// The drawing whose name is being typed under its thumbnail, and the name so far.
    renaming: Option<(DrawingId, String)>,
    /// Where each drawing's thumbnail was drawn, and the tile for a new one.
    #[cfg(test)]
    pub drawn: Vec<(DrawingId, Rect)>,
    #[cfg(test)]
    pub new_tile: Option<Rect>,
    /// The drawings marked as having no variant in the skin shown.
    #[cfg(test)]
    pub day_marked: Vec<DrawingId>,
    /// Where each button of a thumbnail's menu was drawn, while it is open.
    #[cfg(test)]
    pub menu: Vec<(&'static str, Rect)>,
}

impl Substitution {
    /// Show the drawings of `layer`, the one in `exposed` marked, and with a mark in a corner
    /// those of `day_only`: the drawings the skin shown has no variant of, which show as they
    /// are. `advance` is how many frames a picked drawing shows for before the playhead moves
    /// on, 0 for staying on the frame. Returns the drawing that was clicked, or the tile for a
    /// new one.
    /// The bar's *Then On*: how many frames a drawing clicked is shown for, the playhead moved
    /// on by as many.
    pub fn advance(ui: &mut egui::Ui, advance: &mut u32) {
        let hint = "After a click, show the drawing for this many frames and move the playhead on by as many: \
                    one click a mouth shape along a line of dialogue. At 0 the playhead stays, and the drawing takes the place of the one held here";
        ui.add(egui::DragValue::new(advance).range(0..=24).prefix("then on ")).on_hover_text(hint);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, layer: &Layer, palette: &Arc<Palette>, exposed: Option<DrawingId>, day_only: &[DrawingId]) -> Option<Picked> {
        let theme = Theme::of(ui.ctx());
        ui.strong(&layer.name);
        let frame = layer.drawings.iter().fold(PixelRect::EMPTY, |all, entry| all.union(&entry.drawing.bounds()));
        let mut clicked = None;
        #[cfg(test)]
        {
            self.drawn.clear();
            self.day_marked.clear();
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for entry in &layer.drawings {
                    let key = (Arc::as_ptr(&entry.drawing) as usize, Arc::as_ptr(palette) as usize, [frame.x0, frame.y0, frame.x1, frame.y1]);
                    let thumb = self.thumbs.entry(key).or_insert_with(|| Thumb {
                        texture: ui.ctx().load_texture("thumbnail", picture(&entry.drawing, palette, frame), TextureOptions::LINEAR),
                        used: false,
                        _drawing: entry.drawing.clone(),
                        _palette: palette.clone(),
                    });
                    thumb.used = true;
                    let (rect, response) = ui.allocate_exact_size(vec2(SIDE as f32 + 6.0, SIDE as f32 + 20.0), Sense::click());
                    let paper = Rect::from_min_size(rect.min + vec2(3.0, 3.0), vec2(SIDE as f32, SIDE as f32));
                    #[cfg(test)]
                    self.drawn.push((entry.id, rect));
                    let showing = exposed == Some(entry.id);
                    if showing || response.hovered() {
                        ui.painter().rect_filled(rect, 4.0, if showing { theme.accent.gamma_multiply(0.35) } else { theme.surface_raised });
                    }
                    // Paper, so that a drawing looks as it does in the viewport in either theme.
                    ui.painter().rect(paper, 0.0, Color32::WHITE, Stroke::new(1.0, theme.border), egui::StrokeKind::Outside);
                    ui.painter().image(thumb.texture.id(), paper, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    let day = day_only.contains(&entry.id);
                    #[cfg(test)]
                    if day {
                        self.day_marked.push(entry.id);
                    }
                    if day {
                        ui.painter().circle_filled(paper.right_top() + vec2(-6.0, 6.0), 4.0, theme.text_muted);
                    }
                    let strip = Rect::from_min_max(egui::pos2(rect.left(), paper.bottom() + 1.0), rect.max);
                    if let Some((_, text)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == entry.id) {
                        let field = egui::TextEdit::singleline(text).font(egui::FontId::proportional(11.0)).margin(vec2(2.0, 0.0));
                        let mut shown = ui.scope_builder(egui::UiBuilder::new().max_rect(strip), |ui| field.show(ui)).inner;
                        let edit = shown.response;
                        // Just opened, the whole name is selected, so that typing replaces it.
                        if !edit.has_focus() && !edit.lost_focus() {
                            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
                            shown.state.cursor.set_char_range(Some(all));
                            shown.state.store(ui.ctx(), edit.id);
                        }
                        if edit.lost_focus() {
                            // Enter keeps the new name; Escape, or the same name, changes nothing.
                            let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape)) && *text != entry.name;
                            if let Some((_, text)) = self.renaming.take().filter(|_| keep) {
                                clicked = Some(Picked::Rename(entry.id, text.trim().to_string()));
                            }
                        } else {
                            edit.request_focus();
                        }
                    } else {
                        let name = egui::Align2::CENTER_BOTTOM;
                        ui.painter().with_clip_rect(rect).text(
                            rect.center_bottom() - vec2(0.0, 2.0),
                            name,
                            &entry.name,
                            egui::FontId::proportional(11.0),
                            theme.text,
                        );
                    }
                    let response = response.on_hover_text(match day {
                        true => format!("Show {} on this frame. The skin has no variant of it: it shows as it is", entry.name),
                        false => format!("Show {} on this frame", entry.name),
                    });
                    if response.clicked() {
                        clicked = Some(Picked::Drawing(entry.id));
                    }
                    response.context_menu(|ui| {
                        let rename = ui.button("Rename");
                        let delete = ui.button("Delete").on_hover_text("The frames that show it are left empty");
                        #[cfg(test)]
                        {
                            self.menu = vec![("Rename", rename.rect), ("Delete", delete.rect)];
                        }
                        if rename.clicked() {
                            self.renaming = Some((entry.id, entry.name.clone()));
                            ui.close();
                        }
                        if delete.clicked() {
                            clicked = Some(Picked::Delete(entry.id));
                            ui.close();
                        }
                    });
                }
                // After the drawings, a tile for a new empty one.
                let (rect, response) = ui.allocate_exact_size(vec2(SIDE as f32 + 6.0, SIDE as f32 + 20.0), Sense::click());
                let paper = Rect::from_min_size(rect.min + vec2(3.0, 3.0), vec2(SIDE as f32, SIDE as f32));
                #[cfg(test)]
                {
                    self.new_tile = Some(rect);
                }
                if response.hovered() {
                    ui.painter().rect_filled(rect, 4.0, theme.surface_raised);
                }
                let edge = Stroke::new(1.0, theme.text_muted);
                ui.painter().rect_stroke(paper, 0.0, edge, egui::StrokeKind::Inside);
                ui.painter().text(paper.center(), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(28.0), theme.text_muted);
                let name = egui::Align2::CENTER_BOTTOM;
                ui.painter().text(rect.center_bottom() - vec2(0.0, 2.0), name, "New", egui::FontId::proportional(11.0), theme.text);
                if response.on_hover_text("A new empty drawing on this frame").clicked() {
                    clicked = Some(Picked::New);
                }
            });
        });
        self.thumbs.retain(|_, thumb| std::mem::take(&mut thumb.used));
        clicked
    }
}

/// A drawing as a small square picture of what `frame` holds of it, on transparency. Each of
/// its pixels is the average of the drawing pixels it stands for.
fn picture(drawing: &Drawing, palette: &Palette, frame: PixelRect) -> egui::ColorImage {
    let mut image = egui::ColorImage::filled([SIDE, SIDE], Color32::TRANSPARENT);
    if frame.is_empty() {
        return image;
    }
    // A square around the frame, so the drawing keeps its proportions.
    let (width, height) = (frame.width() as usize, frame.height() as usize);
    let side = width.max(height);
    let (left, top) = ((side - width) / 2, (side - height) / 2);
    let pixels = palette.paint(drawing, frame, iskra_model::WorkingSpace::Srgb);
    if side <= SIDE {
        // A drawing smaller than the picture: each of the picture's pixels shows the one it is on.
        for (index, pixel) in image.pixels.iter_mut().enumerate() {
            let (x, y) = ((index % SIDE * side / SIDE).checked_sub(left), (index / SIDE * side / SIDE).checked_sub(top));
            if let (Some(x), Some(y)) = (x.filter(|x| *x < width), y.filter(|y| *y < height)) {
                let [r, g, b, a] = pixels[y * width + x];
                *pixel = Color32::from_rgba_unmultiplied(r, g, b, a);
            }
        }
        return image;
    }
    // For each pixel of the picture: the sums of colour weighted by alpha, of alpha, and the count.
    let mut sums = vec![[0u64; 5]; SIDE * SIDE];
    for (index, [r, g, b, a]) in pixels.into_iter().enumerate() {
        let (x, y) = ((index % width + left) * SIDE / side, (index / width + top) * SIDE / side);
        let sum = &mut sums[y.min(SIDE - 1) * SIDE + x.min(SIDE - 1)];
        let alpha = u64::from(a);
        for (channel, value) in [r, g, b].into_iter().enumerate() {
            sum[channel] += u64::from(value) * alpha;
        }
        (sum[3], sum[4]) = (sum[3] + alpha, sum[4] + 1);
    }
    for (pixel, [r, g, b, alpha, count]) in image.pixels.iter_mut().zip(sums) {
        // Nothing painted in the block leaves the pixel clear.
        let mean = |sum: u64, over: u64| sum.checked_div(over).map(|mean| mean as u8);
        if let (Some(r), Some(g), Some(b), Some(cover)) = (mean(r, alpha), mean(g, alpha), mean(b, alpha), mean(alpha, count)) {
            *pixel = Color32::from_rgba_unmultiplied(r, g, b, cover);
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use iskra_drawing::{ArtContent, ArtLayerKind};
    use iskra_model::SwatchId;

    use super::*;

    #[test]
    fn a_thumbnail_shows_the_drawing_small_in_its_place_in_the_frame() {
        let mut drawing = Drawing::default();
        let Some(ArtContent::Indexed(line)) = drawing.layer_mut(ArtLayerKind::LineArt).map(|l| &mut l.content) else { panic!("indexed") };
        // Red over the left half of a 200 × 100 frame.
        line.paint(PixelRect::new(0, 0, 100, 100), &vec![1.0; 100 * 100], SwatchId(3));
        let image = picture(&drawing, &Palette::standard(), PixelRect::new(0, 0, 200, 100));
        assert_eq!(image.size, [SIDE, SIDE]);
        let at = |x: usize, y: usize| image.pixels[y * SIDE + x];
        assert_eq!(at(10, 32).to_array(), Color32::from_rgb(220, 50, 47).to_array(), "the palette's red, in the left half");
        assert_eq!((at(50, 32).a(), at(10, 5).a(), at(10, 58).a()), (0, 0, 0), "nothing to the right, nor above and below the wide frame");
        assert_eq!(picture(&Drawing::default(), &Palette::standard(), PixelRect::EMPTY).pixels[0], Color32::TRANSPARENT);
    }
}
