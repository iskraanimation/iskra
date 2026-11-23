//! The Text panel (plan §4.19, step 3), made like the Brushes panel: the fonts at the top, as
//! their names or as a preview of each, with a search; under them the format of the box being
//! typed in, or of the next box made.
//!
//! A preview is the family's name set in it, laid out and turned into pixels as a text is, and
//! kept as a picture for as long as the panel is. The font's file is read for it and let go, so
//! a look down the list does not keep every font of the machine in memory; a few previews are
//! made a frame, the list filling in as it is scrolled.

use std::collections::HashMap;

use egui::{Color32, TextureHandle};
use iskra_drawing::fonts::Fonts;
use iskra_model::{ArtContent, Palette, ShapeId, ShapeKind, SwatchId, TextAlign, TextBox, TextFormat, VectorArt, VectorFill, VectorShape};
use serde_json::{Map, Value, json};

use crate::form;

/// How many previews are made in a frame, at most.
const PREVIEWS_A_FRAME: usize = 3;
/// How tall a preview's letters are, in points.
const PREVIEW_SIZE: f32 = 20.0;
/// How much of the panel's height the list of fonts leaves for the format under it.
const FORMAT_HEIGHT: f32 = 230.0;

#[derive(Default)]
pub struct TextPanel {
    search: String,
    /// Each family's preview, by name, once made, and whether it shows the name: none where its
    /// font could not be read.
    previews: HashMap<String, Option<(TextureHandle, bool)>>,
}

/// What the panel changed: the format's fields, as `text.setFormat` takes them; the kerning at
/// the cursor; and whether it is the start of a change, a step of the history of its own.
#[derive(Default)]
pub struct Changed {
    pub fields: Map<String, Value>,
    pub kerning: Option<f64>,
    pub new_step: bool,
    /// The box is to be put at this place of the camera's frame, across and down from 0 to 1.
    pub place: Option<[f64; 2]>,
    /// The box is to be turned into shapes.
    pub to_shapes: bool,
}

/// What the panel shows of the box: its format, the size in the shot's pixels.
pub struct Shown<'a> {
    pub format: &'a TextFormat,
    /// A box is being typed in; otherwise it is the format of the next one.
    pub editing: bool,
    /// The kerning before the letter after the cursor, where the cursor is between letters.
    pub kerning: Option<f64>,
    pub palette: &'a Palette,
    /// The swatch in hand, which a new box is made in.
    pub in_hand: SwatchId,
}

/// The family's name set in its regular face, as a picture: white, to be tinted. A font that
/// can't write its name, of another script or of symbols, shows letters it has, and says so.
fn preview(ctx: &egui::Context, family: &str, fonts: &Fonts) -> Option<(TextureHandle, bool)> {
    let face = fonts.face(family, iskra_drawing::fonts::REGULAR)?;
    let (data, index) = face.read()?;
    let (shown, named) = iskra_drawing::preview_text(&data, index, family)?;
    let scale = ctx.pixels_per_point();
    let format = TextFormat { size: f64::from(PREVIEW_SIZE * scale), leading: 130.0, ..TextFormat::default() };
    let text = TextBox { text: shown, ..TextBox::new([0.0, 0.0], None, format) };
    let laid = iskra_drawing::lay_out_with(&text, &data, index);
    let shapes = laid.glyphs.into_iter().enumerate().map(|(i, outline)| VectorShape {
        id: ShapeId(i as u32 + 1),
        swatch: SwatchId(1),
        kind: ShapeKind::Fill(VectorFill::plain(outline)),
    });
    let art = ArtContent::Vector(VectorArt::new(shapes.collect()));
    let bounds = art.bounds();
    if bounds.width() == 0 || bounds.height() == 0 {
        return None;
    }
    let ArtContent::Vector(art) = &art else { return None };
    let pixels = art.raster().to_rgba8(bounds, |_| [255, 255, 255, 255]);
    let image = egui::ColorImage::from_rgba_unmultiplied([bounds.width() as usize, bounds.height() as usize], pixels.as_flattened());
    Some((ctx.load_texture(format!("font preview {family}"), image, egui::TextureOptions::LINEAR), named))
}

impl TextPanel {
    /// The families whose previews are made.
    #[cfg(test)]
    pub fn previewed(&self) -> Vec<String> {
        self.previews.iter().filter(|(_, texture)| texture.is_some()).map(|(family, _)| family.clone()).collect()
    }

    /// The panel, over `shown`: what it changed.
    /// A part of the bar over the Text panel: the search of the fonts, or whether each is shown
    /// in itself.
    pub fn bar_part(&mut self, ui: &mut egui::Ui, part: &str, previews: &mut bool) {
        match part {
            "fontsearch" => drop(ui.add(egui::TextEdit::singleline(&mut self.search).hint_text("Search fonts").desired_width(140.0))),
            "previews" => drop(ui.toggle_value(previews, "Previews").on_hover_text("Show each font's name set in it, or its name alone")),
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, shown: Shown, previews: &mut bool, fonts: &Fonts) -> Changed {
        let mut changed = Changed::default();
        let format = shown.format;
        let query = self.search.to_lowercase();
        let families: Vec<&str> = fonts.families().into_iter().filter(|family| query.is_empty() || family.to_lowercase().contains(&query)).collect();
        let row_height = if *previews { PREVIEW_SIZE * 1.6 } else { ui.spacing().interact_size.y };
        let text_colour = ui.visuals().text_color();
        let mut made = 0;
        let list_height = (ui.available_height() - FORMAT_HEIGHT).max(row_height * 4.0);
        egui::ScrollArea::vertical().id_salt("fonts").max_height(list_height).auto_shrink([false, true]).show_rows(
            ui,
            row_height,
            families.len(),
            |ui, rows| {
                for family in &families[rows] {
                    let picked = *family == format.font;
                    let texture = if *previews {
                        if !self.previews.contains_key(*family) && made < PREVIEWS_A_FRAME {
                            made += 1;
                            let texture = preview(ui.ctx(), family, fonts);
                            self.previews.insert(family.to_string(), texture);
                        }
                        self.previews.get(*family).cloned().flatten()
                    } else {
                        None
                    };
                    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), egui::Sense::click());
                    if picked || response.hovered() {
                        let fill = if picked { ui.visuals().selection.bg_fill } else { ui.visuals().widgets.hovered.weak_bg_fill };
                        ui.painter().rect_filled(rect, 3.0, fill);
                    }
                    match texture {
                        Some((texture, named)) => {
                            // Not its name: the name first, in the interface's letters, then what
                            // the font writes.
                            let mut left = rect.left() + 6.0;
                            if !named {
                                let at = egui::pos2(left, rect.center().y);
                                let name = ui.painter().text(at, egui::Align2::LEFT_CENTER, *family, egui::FontId::proportional(13.0), text_colour);
                                left = name.right() + 10.0;
                            }
                            let size = texture.size_vec2() / ui.ctx().pixels_per_point();
                            let size = size * (row_height * 0.8 / size.y).min(1.0);
                            let at = egui::Rect::from_min_size(egui::pos2(left, rect.center().y - size.y / 2.0), size);
                            let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                            let tint = if named { text_colour } else { text_colour.gamma_multiply(0.7) };
                            ui.painter().with_clip_rect(rect).image(texture.id(), at, uv, tint);
                        }
                        None => {
                            let at = egui::pos2(rect.left() + 6.0, rect.center().y);
                            ui.painter().text(at, egui::Align2::LEFT_CENTER, *family, egui::FontId::proportional(13.0), text_colour);
                        }
                    }
                    if response.on_hover_text(*family).clicked() && !picked {
                        // The style it had, where the family has it too.
                        let styles = fonts.styles(family);
                        let style = if styles.contains(&format.style.as_str()) { format.style.as_str() } else { iskra_drawing::fonts::REGULAR };
                        changed.fields.insert("font".into(), json!(family));
                        changed.fields.insert("style".into(), json!(style));
                        changed.new_step = true;
                    }
                }
            },
        );
        if made > 0 {
            ui.ctx().request_repaint();
        }
        ui.separator();
        self.format(ui, &shown, fonts, &mut changed);
        changed
    }

    /// The format's fields, under the list.
    fn format(&mut self, ui: &mut egui::Ui, shown: &Shown, fonts: &Fonts, changed: &mut Changed) {
        let format = shown.format;
        ui.label(egui::RichText::new(if shown.editing { "The text being typed in" } else { "The next text made" }).weak());
        form::row(ui, "Font", |ui| {
            ui.label(egui::RichText::new(&format.font).strong());
        });
        if fonts.face(&format.font, &format.style).is_none() {
            form::row(ui, "", |ui| ui.label(egui::RichText::new("Not on this machine: laid out in Inter").color(ui.visuals().warn_fg_color)));
        }
        let set = |changed: &mut Changed, field: &str, value: Value, response: &egui::Response| {
            changed.fields.insert(field.into(), value);
            changed.new_step |= response.drag_started() || response.gained_focus() || response.clicked();
        };
        form::row(ui, "Style", |ui| {
            let mut style = format.style.clone();
            egui::ComboBox::from_id_salt("text style").selected_text(&style).width(form::FIELD_WIDTH).show_ui(ui, |ui| {
                for each in fonts.styles(&format.font) {
                    if ui.selectable_value(&mut style, each.to_string(), each).clicked() {
                        changed.fields.insert("style".into(), json!(each));
                        changed.new_step = true;
                    }
                }
            });
        });
        let number = |ui: &mut egui::Ui, changed: &mut Changed, field: &str, value: f64, range: std::ops::RangeInclusive<f64>, suffix: &str, hint: &str| {
            let mut value = value;
            let response = ui.add(egui::DragValue::new(&mut value).range(range).speed(0.5).suffix(suffix)).on_hover_text(hint);
            if response.changed() {
                set(changed, field, json!(value), &response);
            }
        };
        form::row(ui, "Size", |ui| number(ui, changed, "size", format.size, 1.0..=2000.0, " pt", "How tall a line is at leading 100 %, in the shot's pixels"));
        form::row(ui, "Align", |ui| {
            for (align, name) in [(TextAlign::Left, "Left"), (TextAlign::Centre, "Centre"), (TextAlign::Right, "Right"), (TextAlign::Justified, "Justified")] {
                if ui.selectable_label(format.align == align, name).clicked() && format.align != align {
                    changed.fields.insert("align".into(), json!(align));
                    changed.new_step = true;
                }
            }
        });
        let hint = "Space added after every letter of the text, in thousandths of its size";
        form::row(ui, "Tracking", |ui| number(ui, changed, "tracking", format.tracking, -500.0..=2000.0, "", hint));
        form::row(ui, "Leading", |ui| number(ui, changed, "leading", format.leading, 10.0..=1000.0, " %", "How tall a line is, in percent of the size"));
        form::row(ui, "Kerning", |ui| match shown.kerning {
            Some(kerning) => {
                let mut kerning = kerning;
                let hint = "Space before the letter after the cursor, in thousandths of the size";
                let response = ui.add(egui::DragValue::new(&mut kerning).range(-1000.0..=1000.0).speed(0.5)).on_hover_text(hint);
                if response.changed() {
                    changed.kerning = Some(kerning);
                    changed.new_step |= response.drag_started() || response.gained_focus();
                }
            }
            None => _ = ui.label(egui::RichText::new("Put the cursor between two letters").weak()),
        });
        form::row(ui, "Colour", |ui| {
            let swatch = shown.palette.swatch(format.swatch);
            let [r, g, b, a] = swatch.map_or([0, 0, 0, 0], |swatch| swatch.color);
            let (chip, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            ui.painter().rect_filled(chip, 2.0, Color32::from_rgba_unmultiplied(r, g, b, a));
            ui.label(swatch.map_or("No such swatch", |swatch| swatch.name.as_str()));
            let hint = "Give the text the swatch in hand: it changes with the swatch, everywhere it is used";
            if shown.editing && format.swatch != shown.in_hand && ui.button("Use Swatch in Hand").on_hover_text(hint).clicked() {
                changed.fields.insert("swatch".into(), json!(shown.in_hand));
                changed.new_step = true;
            }
        });
        if !shown.editing {
            form::row(ui, "", |ui| ui.label(egui::RichText::new("A new text is in the swatch in hand").weak()));
        }
        // A box filled behind the letters, as a subtitle's.
        form::row(ui, "Background", |ui| {
            let mut on = format.background.is_some();
            let hint = "Fill the box behind the letters with a swatch, the swatch in hand when it is switched on";
            if ui.checkbox(&mut on, "").on_hover_text(hint).changed() {
                changed.fields.insert("background".into(), if on { json!(shown.in_hand) } else { Value::Null });
                changed.new_step = true;
            }
            if let Some(background) = format.background {
                let swatch = shown.palette.swatch(background);
                let [r, g, b, a] = swatch.map_or([0, 0, 0, 0], |swatch| swatch.color);
                let (chip, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(chip, 2.0, Color32::from_rgba_unmultiplied(r, g, b, a));
                if background != shown.in_hand && ui.button("Use Swatch in Hand").on_hover_text("Fill the background with the swatch in hand").clicked() {
                    changed.fields.insert("background".into(), json!(shown.in_hand));
                    changed.new_step = true;
                }
            }
        });
        if format.background.is_some() {
            let hint = "How far the background reaches past the letters, in percent of the size";
            form::row(ui, "Padding", |ui| number(ui, changed, "padding", format.padding, 0.0..=500.0, " %", hint));
        }
        if shown.editing {
            // Nine places on the camera's frame, inside its title safe area.
            form::row(ui, "Place", |ui| {
                egui::Grid::new("text place").spacing([2.0, 2.0]).show(ui, |ui| {
                    for (down, row) in ["Top", "Middle", "Bottom"].into_iter().enumerate() {
                        for (across, side) in ["left", "", "right"].into_iter().enumerate() {
                            let name = match (row, side) {
                                ("Middle", "") => "the middle".to_string(),
                                (row, "") => format!("the {}", row.to_lowercase()),
                                (row, side) => format!("the {} {side}", row.to_lowercase()),
                            };
                            let hint = format!("Put the text at {name} of the camera's frame, inside its title safe area, and align it there");
                            if ui.add(egui::Button::new("").min_size(egui::vec2(16.0, 12.0))).on_hover_text(hint).clicked() {
                                changed.place = Some([across as f64 / 2.0, down as f64 / 2.0]);
                            }
                        }
                        ui.end_row();
                    }
                });
            });
            let hint = "Turn the text into shapes, which look the same on a machine without its font: it is no longer text (Cmd+Shift+T)";
            form::row(ui, "", |ui| {
                if ui.button("Convert to Shapes").on_hover_text(hint).clicked() {
                    changed.to_shapes = true;
                }
            });
        }
    }
}
