//! The Make Vector Layer window's preview (plan §4.16, step 9): the drawing on the frame in
//! hand as it is painted, and as the trace would make it with the window's settings, side by
//! side. A trace can take a while, so it is made on a thread of its own; while one is made the
//! settings may change again, and the next is begun once it is done, for the settings as they
//! are then.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, TextureHandle, TextureOptions};
use iskra_drawing::{ArtContent, Drawing, Trace};
use iskra_model::{DrawingId, Palette, Rect as PixelRect, StyleId, SwatchId};

/// The longest side of a preview, in pixels.
pub const SIDE: usize = 240;

/// What was traced: the drawing, by its id and its contents, and the settings.
#[derive(Clone, PartialEq)]
struct Of {
    drawing: DrawingId,
    pixels: Arc<Drawing>,
    trace: Trace,
}

/// A trace made: its picture, and how many shapes it has; `None` where it would be too large.
type Made = Option<(egui::ColorImage, usize)>;

#[derive(Default)]
pub struct TracePreview {
    /// The drawing as painted, for the drawing it is of.
    before: Option<(DrawingId, Arc<Drawing>, TextureHandle)>,
    /// The last trace shown, what it was of, and its number of shapes; `None` for one too large.
    after: Option<(Of, Option<(TextureHandle, usize)>)>,
    /// The trace being made, and what it is of.
    making: Option<(Of, Receiver<Made>)>,
}

impl TracePreview {
    /// Let go of everything, as the window closes.
    pub fn clear(&mut self) {
        *self = TracePreview::default();
    }

    /// Whether a trace is being made.
    pub fn busy(&self) -> bool {
        self.making.is_some()
    }

    /// How many shapes the trace shown has, if one is shown.
    #[cfg(test)]
    pub fn shapes(&self) -> Option<usize> {
        self.after.as_ref().and_then(|(_, made)| made.as_ref().map(|(_, shapes)| *shapes))
    }

    /// Draw the two pictures of `drawing` for `trace`, beginning a trace where the one shown is
    /// not of them, with `styles` the shot's palette styles.
    pub fn show(&mut self, ui: &mut egui::Ui, drawing: Option<(DrawingId, Arc<Drawing>)>, trace: &Trace, palette: &Arc<Palette>, styles: &[StyleId]) {
        let Some((id, pixels)) = drawing else {
            ui.weak("No drawing on this frame to preview: the layer's drawings are traced all the same.");
            return;
        };
        let ctx = ui.ctx().clone();
        let colour = |swatch: SwatchId| palette.color_in(styles, swatch);
        // As painted: made again only for another drawing.
        if self.before.as_ref().is_none_or(|(was, was_pixels, _)| *was != id || !Arc::ptr_eq(was_pixels, &pixels)) {
            let image = picture(&pixels, &colour);
            self.before = Some((id, pixels.clone(), ctx.load_texture("trace before", image, TextureOptions::LINEAR)));
        }
        // A trace done is shown; the next is begun where the one shown is not of what is asked.
        if let Some((of, done)) = self.making.take() {
            match done.try_recv() {
                Ok(made) => {
                    let made = made.map(|(image, shapes)| (ctx.load_texture("trace after", image, TextureOptions::LINEAR), shapes));
                    self.after = Some((of, made));
                }
                Err(TryRecvError::Empty) => self.making = Some((of, done)),
                Err(TryRecvError::Disconnected) => {}
            }
        }
        let wanted = Of { drawing: id, pixels: pixels.clone(), trace: trace.clone() };
        if self.making.is_none() && self.after.as_ref().is_none_or(|(of, _)| *of != wanted) {
            let (send, done) = std::sync::mpsc::channel();
            let (of, palette, styles) = (wanted.clone(), palette.clone(), styles.to_vec());
            std::thread::spawn(move || {
                let colours: Vec<([u8; 3], SwatchId)> = palette
                    .swatches
                    .iter()
                    .map(|swatch| {
                        let [r, g, b, _] = palette.color_in(&styles, swatch.id);
                        ([r, g, b], swatch.id)
                    })
                    .collect();
                let _ = send.send(
                    traced(&of.pixels, &of.trace, &colours).map(|(drawing, shapes)| (picture(&drawing, &|swatch| palette.color_in(&styles, swatch)), shapes)),
                );
            });
            self.making = Some((wanted, done));
        }
        if self.making.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        let side = egui::vec2(SIDE as f32 / 2.0, SIDE as f32 / 2.0);
        ui.horizontal(|ui| {
            let frame = |ui: &mut egui::Ui, texture: Option<&TextureHandle>, label: &str| {
                ui.vertical(|ui| {
                    ui.weak(label);
                    let (area, _) = ui.allocate_exact_size(side, egui::Sense::hover());
                    ui.painter().rect_filled(area, 2.0, Color32::WHITE);
                    if let Some(texture) = texture {
                        let size = texture.size_vec2();
                        let fit = (side.x / size.x).min(side.y / size.y);
                        let shown = egui::Rect::from_center_size(area.center(), size * fit);
                        ui.painter().image(texture.id(), shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                    }
                });
            };
            frame(ui, self.before.as_ref().map(|(.., texture)| texture), "Painted");
            let after = self.after.as_ref().and_then(|(_, made)| made.as_ref().map(|(texture, _)| texture));
            frame(ui, after, "Traced");
        });
        match (&self.after, self.busy()) {
            (_, true) => ui.weak("Tracing…"),
            (Some((_, Some((_, shapes)))), false) => ui.weak(format!("{shapes} shapes")),
            (Some((_, None)), false) => ui.weak("Too much detail to trace: lower the detail"),
            (None, false) => ui.weak(""),
        };
    }
}

/// `drawing` with every art layer of pixels traced as `trace` says, and how many shapes that
/// makes; `None` where a trace would be too large.
fn traced(drawing: &Drawing, trace: &Trace, colours: &[([u8; 3], SwatchId)]) -> Option<(Drawing, usize)> {
    let mut traced = drawing.clone();
    let mut shapes = 0;
    for layer in &mut traced.art_layers {
        let art = iskra_drawing::vectorize(&layer.content, trace, colours)?;
        shapes += art.shapes().len();
        layer.content = ArtContent::Vector(art);
    }
    Some((traced, shapes))
}

/// A drawing as a picture no larger than [`SIDE`] each way, keeping its proportions, on
/// transparency. Each of its pixels is the average of the drawing pixels it stands for.
fn picture(drawing: &Drawing, colour: &dyn Fn(SwatchId) -> [u8; 4]) -> egui::ColorImage {
    let bounds = drawing.bounds();
    if bounds.is_empty() {
        return egui::ColorImage::filled([1, 1], Color32::TRANSPARENT);
    }
    // At most one pixel of the picture to one of the drawing.
    let (width, height) = (bounds.width() as usize, bounds.height() as usize);
    let scale = (SIDE as f64 / width.max(height) as f64).min(1.0);
    let (wide, tall) = (((width as f64 * scale).ceil() as usize).max(1), ((height as f64 * scale).ceil() as usize).max(1));
    let pixels = drawing.to_rgba8(PixelRect::new(bounds.x0, bounds.y0, bounds.x1, bounds.y1), colour);
    let mut sums = vec![[0u64; 5]; wide * tall];
    for (index, [r, g, b, a]) in pixels.into_iter().enumerate() {
        let (x, y) = (((index % width) as f64 * scale) as usize, ((index / width) as f64 * scale) as usize);
        let sum = &mut sums[y.min(tall - 1) * wide + x.min(wide - 1)];
        let alpha = u64::from(a);
        for (channel, value) in [r, g, b].into_iter().enumerate() {
            sum[channel] += u64::from(value) * alpha;
        }
        (sum[3], sum[4]) = (sum[3] + alpha, sum[4] + 1);
    }
    let mut image = egui::ColorImage::filled([wide, tall], Color32::TRANSPARENT);
    for (pixel, [r, g, b, alpha, count]) in image.pixels.iter_mut().zip(sums) {
        let mean = |sum: u64, over: u64| sum.checked_div(over).map(|mean| mean as u8);
        if let (Some(r), Some(g), Some(b), Some(cover)) = (mean(r, alpha), mean(g, alpha), mean(b, alpha), mean(alpha, count)) {
            *pixel = Color32::from_rgba_unmultiplied(r, g, b, cover);
        }
    }
    image
}
