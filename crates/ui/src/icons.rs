//! Icons (plan §7.5): Lucide, with our own additions for what is particular to animation. They
//! are embedded SVGs, drawn at the size they are shown and tinted with the theme's text colour.
//! `ATTRIBUTION.md` lists them.

use egui::{Color32, Response, Vec2};

use crate::theme::Theme;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        /// Every icon by name, as SVG text.
        const ICONS: &[(&str, &str)] = &[$(($name, include_str!(concat!("../assets/icons/", $name, ".svg")))),*];
    };
}

icons!(
    "brush",
    "eraser",
    "paint-bucket",
    "eyedropper",
    "shapes",
    "mouse-pointer-2",
    "scissors",
    "move",
    "peg",
    "camera",
    "bones",
    "spline-pointer",
    "text",
    "image",
    "play",
    "square",
    "loop",
    "shot-previous",
    "shot-next",
    "sound",
    "sound-off",
    "onion-skin",
    "light-table",
    "folder-open",
    "storyboard",
    "sparkles",
    "export",
    "ellipsis",
    "lock",
    "lock-open",
    "eye",
    "eye-off",
    "magnet",
    "link",
    "film",
    "layer-vector",
    "layer-bitmap",
    "follow",
    "zoom",
    "harmony-analogous",
    "harmony-complementary",
    "harmony-triad",
    "harmony-shades",
    "art-all",
    "art-over",
    "art-line",
    "art-colour",
    "clip",
    "map",
    "keep-size",
    "ripple",
    "razor",
    "inspector",
    "pivot",
    "pivot-temporary",
    "bones-add",
    "draw-behind",
    "repaint",
    "flatten",
    "lasso",
    "box",
    "art-one",
    "all-drawings",
    "all-layers",
    "pieces",
    "select-shapes",
    "art-down",
    "flip-across",
    "flip-up",
    "turn-left",
    "turn-right",
    "merge-down",
    "strokes",
    "closed",
    "from-middle",
    "filled",
    "handles",
    "gradient",
    "cap-straight",
    "cap-round",
    "cap-miter",
    "shape-line",
    "shape-rectangle",
    "shape-ellipse",
    "shape-polyline",
    "zoom-out",
    "lays-curve",
    "lays-envelope",
    "pencil",
);

/// How thick the icons' lines are, in the units of their 24 by 24 drawing: 2 is as they were
/// drawn, less is thinner. Every icon is drawn with this one width.
pub const STROKE_WIDTH: &str = "1.5";

/// The side of an icon in a toolbar, in points.
pub const SIZE: f32 = 24.0;

/// The width the icons' files are drawn with, as they say it.
const DRAWN_WIDTH: &str = "stroke-width=\"2\"";

/// An icon as an image of `size` points, in `tint`. A name that is no icon draws nothing.
pub fn image(name: &str, size: f32, tint: Color32) -> egui::Image<'static> {
    let svg = ICONS.iter().find(|(icon, _)| *icon == name).map_or("<svg xmlns=\"http://www.w3.org/2000/svg\"/>", |(_, svg)| svg);
    // The icons are drawn in `currentColor`; white takes any tint. Their lines are as thick
    // as `STROKE_WIDTH` says.
    let white = svg.replace("currentColor", "#ffffff").replace(DRAWN_WIDTH, &format!("stroke-width=\"{STROKE_WIDTH}\"")).into_bytes();
    // The width is part of the name, so that pictures made at another are not shown.
    egui::Image::from_bytes(format!("bytes://icons/{name}-{STROKE_WIDTH}.svg"), white).fit_to_exact_size(Vec2::splat(size)).tint(tint)
}

/// A toolbar button that shows an icon. What it does is said in `hint`, its tooltip: every icon
/// has its words somewhere.
pub fn button(ui: &mut egui::Ui, name: &str, selected: bool, hint: &str) -> Response {
    let theme = Theme::of(ui.ctx());
    ui.add(egui::Button::image(image(name, SIZE, theme.text)).selected(selected)).on_hover_text(hint)
}

/// A switch drawn as an icon, lit while it is on: the bars' and the tools' (the user's of
/// 2026-10-26). What it switches is said in `hint`.
pub fn switch(ui: &mut egui::Ui, name: &str, on: bool, hint: &str) -> Response {
    let theme = Theme::of(ui.ctx());
    ui.add(egui::Button::image(image(name, 16.0, theme.text)).selected(on)).on_hover_text(hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_small_svg_drawn_in_the_current_colour() {
        for (name, svg) in ICONS {
            assert!(svg.contains("<svg") && svg.contains("viewBox=\"0 0 24 24\""), "{name}");
            assert!(svg.contains("currentColor"), "{name} can't be tinted");
            assert_eq!(svg.matches("stroke-width").count(), svg.matches(DRAWN_WIDTH).count(), "{name} has a line of a width of its own");
            assert!(svg.contains(DRAWN_WIDTH), "{name} is not drawn in lines: `STROKE_WIDTH` would not change it");
        }
    }
}
