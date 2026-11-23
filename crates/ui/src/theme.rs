//! Appearance (plan §7.5): a dark and a light theme. Widgets ask the theme for a colour by its
//! role instead of naming a colour, so a theme is data and applies everywhere.
//!
//! The area around the camera frame is not themed: it is a neutral grey of the user's choice,
//! because colour can't be judged against a black or white surround.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals};
use serde::{Deserialize, Serialize};

/// Which theme the user wants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Appearance {
    #[default]
    Dark,
    Light,
    /// Dark or light as the system is.
    System,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::Dark, Appearance::Light, Appearance::System];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
            Appearance::System => "Follow the System",
        }
    }
}

/// A colour of the interface that the user can set (Settings ▸ Interface ▸ Colours): one set
/// for both appearances. One that is not set is its own default, which may be the theme's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Accent,
    Animate,
    Breakdowns,
    Cached,
    ShotOdd,
    ShotEven,
    PutAway,
    OnionBefore,
    OnionAfter,
    Coarse,
    Guides,
    Mask,
    Pass,
    Camera,
    Rigged,
    ChainStart,
    /// The other warnings: a display that is not the main one, a render out of date, what a
    /// template would change.
    Warning,
    NodeLayer,
    NodePeg,
    NodeComposite,
    NodeDeformer,
    NodeGroup,
    NodeEffect,
    NodeDisplay,
    NodeCamera,
    StatusBoard,
    StatusOut,
    StatusAnimation,
    StatusPost,
    StatusFinal,
}

impl Role {
    pub const ALL: [Role; 30] = [
        Role::Accent,
        Role::Animate,
        Role::Breakdowns,
        Role::Cached,
        Role::ShotOdd,
        Role::ShotEven,
        Role::PutAway,
        Role::OnionBefore,
        Role::OnionAfter,
        Role::Coarse,
        Role::Guides,
        Role::Mask,
        Role::Pass,
        Role::Camera,
        Role::Rigged,
        Role::ChainStart,
        Role::Warning,
        Role::NodeLayer,
        Role::NodePeg,
        Role::NodeComposite,
        Role::NodeDeformer,
        Role::NodeGroup,
        Role::NodeEffect,
        Role::NodeDisplay,
        Role::NodeCamera,
        Role::StatusBoard,
        Role::StatusOut,
        Role::StatusAnimation,
        Role::StatusPost,
        Role::StatusFinal,
    ];

    /// The heading it is listed under.
    pub fn group(self) -> &'static str {
        match self {
            Role::NodeLayer
            | Role::NodePeg
            | Role::NodeComposite
            | Role::NodeDeformer
            | Role::NodeGroup
            | Role::NodeEffect
            | Role::NodeDisplay
            | Role::NodeCamera => "Node kinds, in the node view",
            Role::StatusBoard | Role::StatusOut | Role::StatusAnimation | Role::StatusPost | Role::StatusFinal => "Shot status",
            _ => "Interface",
        }
    }

    /// What it is used for, as the list says.
    pub fn label(self) -> &'static str {
        match self {
            Role::Accent => "What is picked and selected",
            Role::Animate => "The Animate switch, and the viewer's outline while it is on",
            Role::Breakdowns => "The in-betweens' lines between keyframes, and their points on the curves",
            Role::Cached => "The ruler's line over the frames ready to play",
            Role::ShotOdd => "Shots on the storyboard timeline",
            Role::ShotEven => "Every other shot on the storyboard timeline",
            Role::PutAway => "The marks of what a folded row puts away",
            Role::OnionBefore => "The onion skin, before",
            Role::OnionAfter => "The onion skin, after",
            Role::Coarse => "A layer shown too large for its pixels",
            Role::Guides => "The guides over the camera's frame, and burned in",
            Role::Mask => "The aspect mask: its colour and how dark",
            Role::Pass => "A pass's tag in the node view",
            Role::Camera => "The camera in the Top and Side views",
            Role::Rigged => "Bones: what they rig",
            Role::ChainStart => "Bones: the start of a chain",
            Role::Warning => "Warnings: a display not the main one, a render out of date, what a template changes",
            Role::NodeLayer => "Layers",
            Role::NodePeg => "Pegs",
            Role::NodeComposite => "Composites",
            Role::NodeDeformer => "Bones and curves",
            Role::NodeGroup => "Groups",
            Role::NodeEffect => "Effects",
            Role::NodeDisplay => "Displays",
            Role::NodeCamera => "The camera",
            Role::StatusBoard => "Board",
            Role::StatusOut => "Out, and whom a shot is out to",
            Role::StatusAnimation => "Animation",
            Role::StatusPost => "Post",
            Role::StatusFinal => "Final",
        }
    }

    /// Its colour where the user has set none, in `theme`.
    pub fn default_in(self, theme: &Theme) -> Color32 {
        match self {
            Role::Accent => theme.accent,
            Role::Animate | Role::PutAway => KEY_RED,
            Role::Breakdowns => Color32::from_rgb(90, 90, 90),
            Role::Cached => Color32::from_rgb(14, 66, 34),
            Role::ShotOdd => theme.surface_raised,
            Role::ShotEven => mix(theme.surface_raised, theme.text, 0.07),
            Role::OnionBefore => Color32::from_rgb(230, 60, 60),
            Role::OnionAfter => Color32::from_rgb(60, 170, 80),
            Role::Coarse => AMBER,
            Role::Guides => {
                let [r, g, b] = iskra_export::burn::GUIDE_COLOUR;
                Color32::from_rgb(r, g, b)
            }
            Role::Mask => Color32::from_black_alpha(iskra_export::burn::MASK_ALPHA),
            Role::Pass => PASS,
            Role::Camera => Color32::from_rgb(255, 140, 40),
            Role::Rigged => Color32::from_rgb(230, 120, 30),
            Role::ChainStart => Color32::from_rgb(220, 40, 40),
            Role::Warning | Role::StatusOut => AMBER,
            Role::NodeLayer | Role::StatusAnimation => Color32::from_rgb(86, 140, 220),
            Role::NodePeg | Role::StatusFinal => Color32::from_rgb(84, 170, 110),
            Role::NodeComposite | Role::StatusPost => Color32::from_rgb(164, 122, 214),
            Role::NodeDeformer => Color32::from_rgb(222, 140, 60),
            Role::NodeGroup => Color32::from_rgb(80, 168, 178),
            Role::NodeEffect => Color32::from_rgb(214, 108, 140),
            Role::NodeDisplay | Role::StatusBoard => Color32::from_gray(150),
            Role::NodeCamera => Color32::from_rgb(212, 172, 64),
        }
    }

    /// Whether how strong it is can be set too: the mask's darkness is its alpha.
    pub fn has_alpha(self) -> bool {
        self == Role::Mask
    }
}

/// The colours the user set, by role, as the preferences keep them.
pub type Colours = std::collections::BTreeMap<Role, [u8; 4]>;

/// A theme: one colour for each role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub dark: bool,
    /// Panels and bars.
    pub surface: Color32,
    /// What lies on a surface: buttons, fields, menus.
    pub surface_raised: Color32,
    pub border: Color32,
    pub text: Color32,
    /// Labels that matter less: ruler numbers, hints.
    pub text_muted: Color32,
    /// The one accent: selection, the playhead, the active tool. A spark orange.
    pub accent: Color32,
    pub danger: Color32,
    /// Behind what is selected.
    pub selection: Color32,
    /// The colours the user set, by `Role`, over the defaults; those not set are `None`.
    pub own: [Option<Color32>; Role::ALL.len()],
}

/// The amber of the warnings, where the user has set none.
const AMBER: Color32 = Color32::from_rgb(230, 160, 40);

/// The colour of a Pass (plan §7.14), in the node view, where the user has set none.
const PASS: Color32 = Color32::from_rgb(226, 120, 72);

/// The dark red that says a keyframe will be made, where the user has set none: on the Animate
/// switch while it is on, and on a folded row's marks for the keyframes it puts away.
const KEY_RED: Color32 = Color32::from_rgb(150, 44, 44);

/// `a` with `t` of `b` mixed in.
pub(crate) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let channel = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgb(channel(a.r(), b.r()), channel(a.g(), b.g()), channel(a.b(), b.b()))
}

impl Theme {
    pub fn dark() -> Self {
        let (surface, accent) = (Color32::from_rgb(0x1e, 0x1f, 0x22), Color32::from_rgb(0xf2, 0x7a, 0x2e));
        Self {
            dark: true,
            surface,
            surface_raised: Color32::from_rgb(0x2b, 0x2d, 0x31),
            border: Color32::from_rgb(0x3b, 0x3e, 0x44),
            text: Color32::from_rgb(0xd9, 0xdb, 0xdf),
            text_muted: Color32::from_rgb(0x8b, 0x90, 0x98),
            accent,
            danger: Color32::from_rgb(0xf0, 0x5d, 0x5e),
            selection: mix(surface, accent, 0.4),
            own: [None; Role::ALL.len()],
        }
    }

    pub fn light() -> Self {
        let (surface, accent) = (Color32::from_rgb(0xee, 0xef, 0xf1), Color32::from_rgb(0xd9, 0x5f, 0x14));
        Self {
            dark: false,
            surface,
            surface_raised: Color32::from_rgb(0xfb, 0xfb, 0xfc),
            border: Color32::from_rgb(0xc9, 0xcc, 0xd2),
            text: Color32::from_rgb(0x1f, 0x23, 0x28),
            text_muted: Color32::from_rgb(0x69, 0x70, 0x7a),
            accent,
            danger: Color32::from_rgb(0xc6, 0x28, 0x2d),
            selection: mix(surface, accent, 0.3),
            own: [None; Role::ALL.len()],
        }
    }

    /// The fill of a check box, a radio button and a slider's rail at rest: seen on a panel,
    /// on a window and on a selected row. In the dark theme it is lighter than a window, as a
    /// darker one was the panel's colour; in the light theme darker than a panel.
    pub fn well(&self) -> Color32 {
        if self.dark { mix(self.surface_raised, self.text, 0.2) } else { mix(self.surface_raised, Color32::BLACK, 0.18) }
    }

    /// The theme for what the user chose, given what the system says it is using.
    /// A keyframe on a timeline that a cycle repeats, which is shown and not picked: a darker
    /// grey than a keyframe.
    pub fn repeated_key(&self) -> Color32 {
        mix(self.surface, self.text, 0.35)
    }

    /// The light behind a row of a timeline that is selected.
    pub fn row_light(&self) -> Color32 {
        mix(self.surface, self.text, 0.06)
    }

    /// The fill of a block on a row of a timeline, a hold of a drawing. On the selected row
    /// it has to stand out from the row's light, which in the dark theme is as dark as a
    /// block is elsewhere: there it is lighter, so that where a hold ends is seen.
    pub fn block_fill(&self, selected: bool) -> Color32 {
        if selected && self.dark { mix(self.surface_raised, self.text, 0.16) } else { self.surface_raised }
    }

    pub fn for_appearance(appearance: Appearance, system: Option<egui::Theme>) -> Self {
        match (appearance, system) {
            (Appearance::Light, _) | (Appearance::System, Some(egui::Theme::Light)) => Self::light(),
            _ => Self::dark(),
        }
    }

    /// The theme with the colours the user set. A colour of the accent's own is the accent,
    /// and what is selected is lit by it.
    pub fn with_colours(mut self, colours: &Colours) -> Self {
        for (at, role) in Role::ALL.into_iter().enumerate() {
            self.own[at] = colours.get(&role).map(|[r, g, b, a]| Color32::from_rgba_unmultiplied(*r, *g, *b, *a));
        }
        if let Some(accent) = self.own[Role::Accent as usize] {
            let accent = Color32::from_rgb(accent.r(), accent.g(), accent.b());
            self.selection = mix(self.surface, accent, if self.dark { 0.4 } else { 0.3 });
            self.accent = accent;
        }
        self
    }

    /// The colour of `role`: the user's, or its default.
    pub fn colour(&self, role: Role) -> Color32 {
        self.own[role as usize].unwrap_or_else(|| role.default_in(self))
    }

    /// The theme in use, for widgets that paint themselves.
    pub fn of(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp(egui::Id::NULL)).unwrap_or_else(Self::dark)
    }

    /// Make this the theme in use: egui's own widgets take their colours from it too.
    pub fn apply(self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, self));
        let mut visuals = if self.dark { Visuals::dark() } else { Visuals::light() };
        visuals.panel_fill = self.surface;
        visuals.window_fill = self.surface_raised;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        // Text fields and scroll areas sit a little below the surface.
        visuals.extreme_bg_color = mix(self.surface, if self.dark { Color32::BLACK } else { Color32::WHITE }, 0.35);
        visuals.faint_bg_color = self.row_light();
        visuals.selection.bg_fill = self.selection;
        visuals.selection.stroke = Stroke::new(1.0, self.accent);
        visuals.hyperlink_color = self.accent;
        visuals.error_fg_color = self.danger;
        visuals.warn_fg_color = self.accent;
        let widgets = &mut visuals.widgets;
        for (widget, fill, outline) in [
            (&mut widgets.noninteractive, self.surface, self.border),
            (&mut widgets.inactive, self.surface_raised, self.surface_raised),
            (&mut widgets.hovered, mix(self.surface_raised, self.text, 0.1), self.border),
            (&mut widgets.active, mix(self.surface_raised, self.text, 0.18), self.accent),
            (&mut widgets.open, mix(self.surface_raised, self.text, 0.1), self.border),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = Stroke::new(1.0, outline);
            widget.fg_stroke = Stroke::new(1.0, self.text);
            widget.corner_radius = CornerRadius::same(4);
        }
        // A check box, a radio button and a slider's rail at rest are set off from a window and
        // a panel, so that one that is off is seen on both; buttons keep the raised surface.
        widgets.inactive.bg_fill = self.well();
        ctx.set_theme(if self.dark { egui::Theme::Dark } else { egui::Theme::Light });
        ctx.set_visuals(visuals);
    }
}

/// The font family of headings and of whatever else is set in semibold.
pub const SEMIBOLD: &str = "semibold";

/// The typeface of frame numbers and timecode.
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Install the interface's typefaces, Inter and JetBrains Mono, with egui's own fonts behind
/// them for the characters they lack, and the compact spacing of a tool that stays open all day.
pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let faces: [(&str, &'static [u8], FontFamily); 3] = [
        ("Inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"), FontFamily::Proportional),
        ("Inter SemiBold", include_bytes!("../assets/fonts/Inter-SemiBold.ttf"), FontFamily::Name(SEMIBOLD.into())),
        ("JetBrains Mono", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"), FontFamily::Monospace),
    ];
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (name, bytes, family) in faces {
        fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
        let stack = fonts.families.entry(family).or_default();
        if stack.is_empty() {
            stack.extend(fallback.iter().cloned());
        }
        stack.insert(0, name.to_string());
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
        style.spacing.interact_size.y = 22.0;
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, FontId::new(15.0, FontFamily::Name(SEMIBOLD.into()))),
            (TextStyle::Monospace, mono(12.0)),
        ]
        .into();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contrast ratio of two colours, as WCAG defines it: 1 (none) to 21 (black on white).
    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |c: Color32| {
            let linear = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn both_themes_keep_text_readable_and_the_chrome_quiet() {
        for theme in [Theme::dark(), Theme::light()] {
            for surface in [theme.surface, theme.surface_raised, theme.selection] {
                assert!(contrast(theme.text, surface) >= 4.5, "text on {surface:?} in {theme:?}");
            }
            assert!(contrast(theme.text_muted, theme.surface) >= 3.0, "muted text is still legible");
            assert!(contrast(theme.accent, theme.surface) >= 3.0, "the accent stands out from the chrome");
            assert!(contrast(theme.border, theme.surface) < 2.0, "borders are low-contrast");
            assert!(contrast(theme.block_fill(true), theme.row_light()) >= 1.2, "a hold is told from the selected row in {theme:?}");
            assert!(contrast(theme.block_fill(false), theme.surface) >= 1.1, "and from a row that is not, in {theme:?}");
            assert!(contrast(theme.text, theme.block_fill(true)) >= 4.5, "its name is read on it");
            for (under, on) in [(theme.surface_raised, "a window"), (theme.surface, "a panel"), (theme.row_light(), "a selected row")] {
                assert!(contrast(theme.well(), under) >= 1.2, "a check box that is off is seen on {on} in {theme:?}");
            }
        }
    }

    #[test]
    fn the_appearance_picks_the_theme_and_can_follow_the_system() {
        let theme = |appearance, system| Theme::for_appearance(appearance, system).dark;
        assert!(theme(Appearance::Dark, Some(egui::Theme::Light)) && !theme(Appearance::Light, Some(egui::Theme::Dark)));
        assert!(!theme(Appearance::System, Some(egui::Theme::Light)) && theme(Appearance::System, Some(egui::Theme::Dark)));
        assert!(theme(Appearance::System, None), "dark when the system doesn't say");
    }

    #[test]
    fn the_users_colours_are_over_the_defaults_and_the_accent_lights_the_selection() {
        let theme = Theme::dark();
        assert_eq!((theme.colour(Role::Accent), theme.colour(Role::Animate)), (theme.accent, KEY_RED));
        let colours: Colours = [(Role::Accent, [0, 120, 255, 255]), (Role::Mask, [0, 0, 0, 40])].into();
        let own = theme.with_colours(&colours);
        assert_eq!((own.accent, own.colour(Role::Mask).a(), own.colour(Role::Animate)), (Color32::from_rgb(0, 120, 255), 40, KEY_RED));
        assert!(own.selection != theme.selection && own.selection.b() > own.selection.r());
        assert_eq!(own.with_colours(&Colours::new()).own, theme.own, "taken back, none is the user's");
        // The breakdowns' lines are grey until they are given their own colour.
        assert_eq!(theme.colour(Role::Breakdowns), Color32::from_rgb(90, 90, 90));
        let blue = theme.with_colours(&[(Role::Breakdowns, [40, 90, 220, 255])].into());
        assert_eq!((blue.colour(Role::Breakdowns), blue.colour(Role::Animate)), (Color32::from_rgb(40, 90, 220), KEY_RED));
        // The node kinds, the shots' statuses and the warnings start as they were, and are
        // listed under their own headings.
        assert_eq!(
            (theme.colour(Role::NodeLayer), theme.colour(Role::StatusOut), theme.colour(Role::Warning)),
            (Color32::from_rgb(86, 140, 220), AMBER, AMBER)
        );
        let groups: std::collections::BTreeSet<&str> = Role::ALL.iter().map(|role| role.group()).collect();
        assert_eq!(groups.len(), 3);
        let own = theme.with_colours(&[(Role::NodePeg, [1, 2, 3, 255])].into());
        assert_eq!(
            (own.colour(Role::NodePeg), own.colour(Role::StatusFinal)),
            (Color32::from_rgb(1, 2, 3), Color32::from_rgb(84, 170, 110)),
            "a status of the same colour is apart"
        );
    }

    #[test]
    fn an_applied_theme_is_the_one_widgets_get() {
        let ctx = egui::Context::default();
        assert_eq!(Theme::of(&ctx), Theme::dark(), "before any is applied");
        Theme::light().apply(&ctx);
        assert_eq!(Theme::of(&ctx), Theme::light());
        assert_eq!(ctx.global_style().visuals.panel_fill, Theme::light().surface);
    }
}
