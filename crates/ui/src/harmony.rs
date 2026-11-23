//! Colours that go with a colour (plan §4.8): five of them, after a rule of the colour wheel,
//! for the swatch window to offer as new swatches. The colour itself is the first one.
//!
//! The wheel is the painter's, red, yellow and blue, on which yellow is across from violet:
//! hues are turned on it and brought back to the screen's. The amounts are fitted to what the
//! user gave for yellow (2026-10-23).

/// A colour as hue (degrees), saturation and value, each of the sRGB numbers as they are, as
/// paint programs give them; not of light, as egui's `Hsva` is.
#[derive(Clone, Copy, Debug)]
struct Hsv {
    h: f32,
    s: f32,
    v: f32,
}

impl Hsv {
    fn of([r, g, b, _]: [u8; 4]) -> Hsv {
        let [r, g, b] = [r, g, b].map(|c| f32::from(c) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let range = max - min;
        let h = if range <= 0.0 {
            0.0
        } else if max == r {
            60.0 * ((g - b) / range).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / range + 2.0)
        } else {
            60.0 * ((r - g) / range + 4.0)
        };
        Hsv { h, s: if max > 0.0 { range / max } else { 0.0 }, v: max }
    }

    fn rgba(self, alpha: u8) -> [u8; 4] {
        let Hsv { h, s, v } = self;
        let (c, h) = (v * s, h.rem_euclid(360.0) / 60.0);
        let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
        let [r, g, b] = match h as u32 {
            0 => [c, x, 0.0],
            1 => [x, c, 0.0],
            2 => [0.0, c, x],
            3 => [0.0, x, c],
            4 => [x, 0.0, c],
            _ => [c, 0.0, x],
        };
        let byte = |channel: f32| ((channel + v - c) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        [byte(r), byte(g), byte(b), alpha]
    }
}

/// How the five colours are found from the one they go with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Harmony {
    /// Its neighbours on the wheel, two each side.
    #[default]
    Analogous,
    /// Across the wheel from it, with a softer and darker of each.
    Complementary,
    /// A third of the way round the wheel each way, with a softer and darker of it and of one.
    Triad,
    /// The same colour darker, step by step.
    Shades,
}

/// The painter's wheel against the screen's: a hue on the one, in degrees, and where it is on
/// the other. Between two of them it goes evenly.
const WHEEL: [(f32, f32); 6] = [(0.0, 0.0), (120.0, 60.0), (150.0, 110.0), (240.0, 205.0), (300.0, 269.0), (360.0, 360.0)];

/// A hue of the painter's wheel on the screen's.
fn to_screen(painter: f32) -> f32 {
    between(painter.rem_euclid(360.0), |(p, _)| p, |(_, s)| s)
}

/// A hue of the screen's wheel on the painter's.
fn to_painter(screen: f32) -> f32 {
    between(screen.rem_euclid(360.0), |(_, s)| s, |(p, _)| p)
}

/// `hue` read on the one side of [`WHEEL`] and given on the other.
fn between(hue: f32, from: impl Fn((f32, f32)) -> f32, to: impl Fn((f32, f32)) -> f32) -> f32 {
    let at = WHEEL.windows(2).find(|pair| hue <= from(pair[1])).unwrap_or(&WHEEL[4..]);
    let (a, b) = (at[0], at[1]);
    to(a) + (hue - from(a)) / (from(b) - from(a)) * (to(b) - to(a))
}

impl Harmony {
    pub const ALL: [Harmony; 4] = [Harmony::Analogous, Harmony::Complementary, Harmony::Triad, Harmony::Shades];

    pub fn title(self) -> &'static str {
        match self {
            Harmony::Analogous => "Analogous",
            Harmony::Complementary => "Complementary",
            Harmony::Triad => "Triad",
            Harmony::Shades => "Shades",
        }
    }

    /// The name of its icon.
    pub fn icon(self) -> &'static str {
        match self {
            Harmony::Analogous => "harmony-analogous",
            Harmony::Complementary => "harmony-complementary",
            Harmony::Triad => "harmony-triad",
            Harmony::Shades => "harmony-shades",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Harmony::Analogous => "The colours beside this one on the painter's colour wheel, two each side",
            Harmony::Complementary => "The colour across the painter's wheel from this one, with a softer and darker of each",
            Harmony::Triad => "The two colours a third of the way round the painter's wheel from this one, with softer and darker ones",
            Harmony::Shades => "This colour darker, step by step",
        }
    }

    /// The five colours that go with `color` (straight-alpha sRGB), it first. They keep its
    /// alpha.
    pub fn of(self, color: [u8; 4]) -> [[u8; 4]; 5] {
        let base = Hsv::of(color);
        let Hsv { h, s, v } = base;
        let painter = to_painter(h);
        // So many degrees round the painter's wheel from this colour, as strong and as bright
        // as this one is times `soft` and `dark`.
        let at = |turn: f32, soft: f32, dark: f32| Hsv { h: to_screen(painter + turn), s: (s * soft).clamp(0.0, 1.0), v: (v * dark).clamp(0.0, 1.0) };
        let made = match self {
            Harmony::Analogous => [base, at(-15.0, 1.0, 1.0), at(15.0, 1.0, 1.0), at(-30.0, 1.0, 1.0), at(30.0, 1.0, 1.0)],
            Harmony::Complementary => [base, at(180.0, 1.0, 1.0), at(0.0, 0.75, 0.75), at(180.0, 0.665, 0.667), at(0.0, 0.5, 0.5)],
            Harmony::Triad => [base, at(-120.0, 1.0, 1.0), at(120.0, 1.0, 1.0), at(0.0, 0.665, 0.667), at(-120.0, 0.5, 0.5)],
            Harmony::Shades => [base, at(0.0, 1.0, 0.86), at(0.0, 1.0, 0.70), at(0.0, 1.0, 0.54), at(0.0, 1.0, 0.38)],
        };
        // The first is the colour as given, not as it came back through HSV.
        let mut out = made.map(|hsv| hsv.rgba(color[3]));
        out[0] = color;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> [u8; 4] {
        let n = u32::from_str_radix(text, 16).unwrap();
        [(n >> 16) as u8, (n >> 8) as u8, n as u8, 255]
    }

    /// Each channel within a few steps: the numbers are rounded, and the triad's red is 1° off on the
    /// wheel the user's came from (FF0500, against our FF0000).
    fn close(got: [[u8; 4]; 5], wanted: [&str; 5]) -> bool {
        got.iter().zip(wanted).all(|(got, wanted)| got.iter().zip(hex(wanted)).all(|(a, b)| a.abs_diff(b) <= 6))
    }

    #[test]
    fn yellow_gives_what_the_user_gave_for_it() {
        let yellow = hex("FFFF00");
        let rules = [
            (Harmony::Complementary, ["FFFF00", "7D00FF", "BFBF30", "7039AA", "808040"]),
            (Harmony::Analogous, ["FFFF00", "FFE000", "94FF00", "FFC100", "2AFF00"]),
            (Harmony::Triad, ["FFFF00", "FF0500", "0094FF", "AAAA39", "804140"]),
            (Harmony::Shades, ["FFFF00", "DBDB00", "B3B300", "8A8A00", "616100"]),
        ];
        for (harmony, wanted) in rules {
            let got = harmony.of(yellow);
            assert!(close(got, wanted), "{harmony:?}: {:?}", got.map(|c| format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])));
        }
    }

    #[test]
    fn the_two_wheels_go_round_alike_both_ways() {
        for screen in [0.0, 30.0, 60.0, 90.0, 120.0, 200.0, 240.0, 300.0, 359.0] {
            assert!((to_screen(to_painter(screen)) - screen).abs() < 1e-3, "{screen}");
        }
        assert!((to_painter(60.0) - 120.0).abs() < 1e-3, "yellow is a third of the way round the painter's wheel");
    }

    #[test]
    fn every_colour_keeps_its_alpha_and_comes_first() {
        for color in [[0, 0, 0, 255], [128, 128, 128, 90], [255, 255, 255, 255], [220, 40, 40, 200]] {
            for harmony in Harmony::ALL {
                let five = harmony.of(color);
                assert_eq!(five[0], color, "{harmony:?}");
                assert!(five.iter().all(|c| c[3] == color[3]), "{harmony:?}");
            }
        }
    }
}
