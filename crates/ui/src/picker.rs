//! The swatch window's colour picker, laid out as the user's mock-up of 2026-11-02 has it: a
//! row with what the window puts first (its eyedropper) at its left and the colour in hex
//! after it, the colour, then the field of saturation and value, the hue and the alpha,
//! all as wide as the window.
//!
//! egui's own picker is one piece whose rows can't be put apart, so its parts are drawn here
//! after egui's `widgets/color_picker.rs` (egui 0.36, MIT OR Apache-2.0; see `ATTRIBUTION.md`):
//! the 2D slider and the 1D sliders, with egui's colour types.

use egui::ecolor::{Hsva, HsvaGamma};
use egui::emath::{lerp, remap_clamp};
use egui::{Color32, Mesh, Pos2, Response, Sense, Shape, Stroke, StrokeKind, TextEdit, Ui, Vec2};

/// Steps of a slider's colours, each way.
const STEPS: u32 = 36;

/// How wide the field of the colour in hex is, in points: eight digits.
const HEX_FIELD: f32 = 80.0;

/// How high the field of saturation and value is at most, in points: square in a narrow
/// window, wider than high in a wide one, so that the window fits on a small screen.
const SQUARE_HIGH: f32 = 250.0;

/// Black or white, whichever shows on `color`.
fn contrast(color: Color32) -> Color32 {
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    if u32::from(r) * 3 + u32::from(g) * 6 + u32::from(b) > 128 * 10 { Color32::BLACK } else { Color32::WHITE }
}

/// The colour `color` (straight sRGB) is, picked: `first` drawn at the left of the row of its
/// hex.
pub fn picker(ui: &mut Ui, color: &mut [u8; 4], first: impl FnOnce(&mut Ui)) {
    // The hue and saturation are kept while they can't be told from the colour (a grey, black).
    let kept = ui.id().with("swatch picker");
    let mut hsvag = match ui.data(|data| data.get_temp::<([u8; 4], HsvaGamma)>(kept)) {
        Some((was, hsvag)) if was == *color => hsvag,
        _ => HsvaGamma::from(Hsva::from_srgba_unmultiplied(*color)),
    };
    let before = hsvag;
    let wide = ui.available_width();
    ui.horizontal(|ui| {
        first(ui);
        hex(ui, &mut hsvag);
    });
    let shown = Hsva::from(hsvag).to_srgba_unmultiplied();
    let strip = ui.allocate_exact_size(Vec2::new(wide, ui.spacing().interact_size.y), Sense::hover()).0;
    crate::paint_checker(ui.painter(), strip);
    ui.painter().rect_filled(strip, 0.0, Color32::from_rgba_unmultiplied(shown[0], shown[1], shown[2], shown[3]));
    let opaque = HsvaGamma { a: 1.0, ..hsvag };
    let HsvaGamma { h, s, v, a } = &mut hsvag;
    square(ui, Vec2::new(wide, wide.min(SQUARE_HIGH)), s, v, |s, v| HsvaGamma { s, v, ..opaque }.into());
    bar(ui, wide, h, |h| HsvaGamma { h, s: 1.0, v: 1.0, a: 1.0 }.into()).on_hover_text("Hue");
    bar(ui, wide, a, |a| HsvaGamma { a, ..opaque }.into()).on_hover_text("Alpha");
    if hsvag != before {
        *color = Hsva::from(hsvag).to_srgba_unmultiplied();
    }
    ui.data_mut(|data| data.insert_temp(kept, (*color, hsvag)));
}

/// The colour in hex, after a button that copies it. What is typed stays as typed while the
/// field has the keyboard, and sets the colour once it says one.
fn hex(ui: &mut Ui, hsvag: &mut HsvaGamma) {
    let rgba = Hsva::from(*hsvag).to_srgba_unmultiplied();
    let typed = ui.id().with("swatch hex");
    let mut hex = ui.data(|data| data.get_temp::<String>(typed)).unwrap_or_else(|| crate::hex_of(rgba));
    if ui.button("📋").on_hover_text("Copy the colour in hex").clicked() {
        ui.copy_text(format!("#{}", crate::hex_of(rgba)));
    }
    ui.label("#");
    let field = ui.add(TextEdit::singleline(&mut hex).desired_width(HEX_FIELD)).on_hover_text("The colour in hex: af4dbd, or with its alpha, af4dbd80");
    if field.changed()
        && let Some(said) = crate::color_of_hex(&hex)
        && said != rgba
    {
        *hsvag = HsvaGamma::from(Hsva::from_srgba_unmultiplied(said));
    }
    if field.has_focus() {
        ui.data_mut(|data| data.insert_temp(typed, hex));
    } else {
        ui.data_mut(|data| data.remove::<String>(typed));
    }
}

/// A field of the colours `color_at` gives across it (`x` from left to right, `y` from the
/// bottom up), of `size`, the pair picked marked by a ring.
fn square(ui: &mut Ui, size: Vec2, x: &mut f32, y: &mut f32, color_at: impl Fn(f32, f32) -> Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if let Some(at) = response.interact_pointer_pos() {
        *x = remap_clamp(at.x, rect.left()..=rect.right(), 0.0..=1.0);
        *y = remap_clamp(at.y, rect.bottom()..=rect.top(), 0.0..=1.0);
    }
    let mut mesh = Mesh::default();
    for j in 0..=STEPS {
        for i in 0..=STEPS {
            let (u, w) = (i as f32 / STEPS as f32, j as f32 / STEPS as f32);
            mesh.colored_vertex(Pos2::new(lerp(rect.left()..=rect.right(), u), lerp(rect.bottom()..=rect.top(), w)), color_at(u, w));
            if i < STEPS && j < STEPS {
                let corner = j * (STEPS + 1) + i;
                mesh.add_triangle(corner, corner + 1, corner + STEPS + 1);
                mesh.add_triangle(corner + 1, corner + STEPS + 1, corner + STEPS + 2);
            }
        }
    }
    ui.painter().add(Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, 0.0, ui.style().interact(&response).bg_stroke, StrokeKind::Inside);
    let at = Pos2::new(lerp(rect.left()..=rect.right(), *x), lerp(rect.bottom()..=rect.top(), *y));
    let picked = color_at(*x, *y);
    ui.painter().circle(at, rect.height() / 12.0, picked, Stroke::new(1.5, contrast(picked)));
    response
}

/// A bar of the colours `color_at` gives along it, over a checker, `wide` across, the value
/// picked marked by a point under it.
fn bar(ui: &mut Ui, wide: f32, value: &mut f32, color_at: impl Fn(f32) -> Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(wide, ui.spacing().interact_size.y), Sense::click_and_drag());
    if let Some(at) = response.interact_pointer_pos() {
        *value = remap_clamp(at.x, rect.left()..=rect.right(), 0.0..=1.0);
    }
    crate::paint_checker(ui.painter(), rect);
    let mut mesh = Mesh::default();
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let x = lerp(rect.left()..=rect.right(), t);
        mesh.colored_vertex(Pos2::new(x, rect.top()), color_at(t));
        mesh.colored_vertex(Pos2::new(x, rect.bottom()), color_at(t));
        if i < STEPS {
            mesh.add_triangle(2 * i, 2 * i + 1, 2 * i + 2);
            mesh.add_triangle(2 * i + 1, 2 * i + 2, 2 * i + 3);
        }
    }
    ui.painter().add(Shape::mesh(mesh));
    ui.painter().rect_stroke(rect, 0.0, ui.style().interact(&response).bg_stroke, StrokeKind::Inside);
    let (x, r) = (lerp(rect.left()..=rect.right(), *value), rect.height() / 4.0);
    let picked = color_at(*value);
    let mark = vec![Pos2::new(x, rect.center().y), Pos2::new(x + r, rect.bottom()), Pos2::new(x - r, rect.bottom())];
    ui.painter().add(Shape::convex_polygon(mark, picked, Stroke::new(1.0, contrast(picked))));
    response
}
