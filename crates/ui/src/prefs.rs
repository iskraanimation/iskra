//! The user's preferences: what belongs to the person, not to the film (plan §7.7). They are
//! one JSON file in the user's config folder, read when the app starts and written when one
//! changes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::dock::{DockNode, Page, Panel};
use crate::theme::Appearance;

/// How long a new drawing holds (plan §7.2b).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NewDrawingHold {
    /// Until the next drawing, or the end of the shot: a board drawn on a few frames fills it.
    #[default]
    ToNext,
    /// This many frames: 1 for frame-by-frame work, 2 for animating on twos.
    Frames(u32),
}

impl NewDrawingHold {
    /// As the `hold` parameter of the commands that can make a drawing.
    pub fn param(self) -> Value {
        match self {
            NewDrawingHold::ToNext => json!("toNext"),
            NewDrawingHold::Frames(frames) => json!(frames.max(1)),
        }
    }

    pub fn label(self) -> String {
        match self {
            NewDrawingHold::ToNext => "Until the next drawing".to_string(),
            NewDrawingHold::Frames(1) => "1 frame".to_string(),
            NewDrawingHold::Frames(frames) => format!("{frames} frames"),
        }
    }
}

/// Where a pen's pressure is read from on Windows (Settings ▸ General): the tablet driver's
/// WinTab, or the touch events Windows Ink makes of the pen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TabletInput {
    /// WinTab where the driver offers it, else Windows Ink.
    #[default]
    Automatic,
    WindowsInk,
    WinTab,
}

impl TabletInput {
    pub const ALL: [TabletInput; 3] = [TabletInput::Automatic, TabletInput::WindowsInk, TabletInput::WinTab];

    pub fn label(self) -> &'static str {
        match self {
            TabletInput::Automatic => "Automatic",
            TabletInput::WindowsInk => "Windows Ink",
            TabletInput::WinTab => "WinTab",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            TabletInput::Automatic => "WinTab where the tablet's driver offers it, with pressure, tilt, rotation and the eraser end; else Windows Ink",
            TabletInput::WindowsInk => "The pen as Windows Ink gives it: pressure alone, and none with Windows Ink off in the tablet's driver",
            TabletInput::WinTab => "The tablet's driver alone, with Windows Ink's pressure left out: with no WinTab, a pen draws as a mouse",
        }
    }

    /// Whether the driver's WinTab is read.
    pub fn reads_wintab(self) -> bool {
        self != TabletInput::WindowsInk
    }

    /// Whether the force of Windows Ink's touch events is read.
    pub fn reads_ink(self) -> bool {
        self != TabletInput::WinTab
    }
}

/// The graphics API to draw with (Settings ▸ Memory and GPU).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GraphicsBackend {
    /// What the platform draws with best: Metal, DirectX 12 or Vulkan, else OpenGL.
    #[default]
    Auto,
    Metal,
    DirectX12,
    Vulkan,
    OpenGl,
}

impl GraphicsBackend {
    /// What this platform offers.
    #[cfg(target_os = "macos")]
    pub const OFFERED: &[GraphicsBackend] = &[GraphicsBackend::Auto, GraphicsBackend::Metal];
    #[cfg(windows)]
    pub const OFFERED: &[GraphicsBackend] = &[GraphicsBackend::Auto, GraphicsBackend::DirectX12, GraphicsBackend::Vulkan];
    #[cfg(not(any(target_os = "macos", windows)))]
    pub const OFFERED: &[GraphicsBackend] = &[GraphicsBackend::Auto, GraphicsBackend::Vulkan, GraphicsBackend::OpenGl];

    pub fn label(self) -> &'static str {
        match self {
            GraphicsBackend::Auto => "Auto",
            GraphicsBackend::Metal => "Metal",
            GraphicsBackend::DirectX12 => "DirectX 12",
            GraphicsBackend::Vulkan => "Vulkan",
            GraphicsBackend::OpenGl => "OpenGL",
        }
    }

    /// The backends wgpu may pick from, or None for its own choice.
    pub fn backends(self) -> Option<wgpu::Backends> {
        match self {
            GraphicsBackend::Auto => None,
            GraphicsBackend::Metal => Some(wgpu::Backends::METAL),
            GraphicsBackend::DirectX12 => Some(wgpu::Backends::DX12),
            GraphicsBackend::Vulkan => Some(wgpu::Backends::VULKAN),
            GraphicsBackend::OpenGl => Some(wgpu::Backends::GL),
        }
    }
}

/// How the Export page reads the playhead's place (the right-click menu under its viewer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimeReading {
    /// The film's time from its frame 0.
    #[default]
    TimelineTimecode,
    /// The time from the shot's in point.
    ShotTimecode,
    /// The film's frame.
    TimelineFrame,
    /// The shot's own frame, its handles counted, as the Animation timeline numbers it.
    SourceFrame,
}

impl TimeReading {
    pub const ALL: [TimeReading; 4] = [TimeReading::TimelineTimecode, TimeReading::ShotTimecode, TimeReading::TimelineFrame, TimeReading::SourceFrame];

    pub fn label(self) -> &'static str {
        match self {
            TimeReading::TimelineTimecode => "Timeline Timecode",
            TimeReading::ShotTimecode => "Shot Timecode",
            TimeReading::TimelineFrame => "Timeline Frame",
            TimeReading::SourceFrame => "Source Frame",
        }
    }
}

/// The guides, which a render can have burned in, are the export's.
pub use iskra_export::burn::{GUIDE_COLOUR, Guide, GuideKind};

/// The aspect ratios a mask is offered in, with the names they go by.
pub const ASPECTS: [(&str, f64); 6] = [("4:3", 4.0 / 3.0), ("16:9", 16.0 / 9.0), ("1.85:1", 1.85), ("2.39:1", 2.39), ("9:16", 9.0 / 16.0), ("1:1", 1.0)];

/// A guide of the user's own (plan §4.4): what it draws, in which colour, and whether it shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserGuide {
    pub name: String,
    pub kind: GuideKind,
    pub colour: [u8; 3],
    pub shown: bool,
}

impl UserGuide {
    /// The guide a click on *Add a Guide* makes: the frame of a wide film, in the guides' blue.
    pub fn new(name: String) -> Self {
        Self { name, kind: GuideKind::Aspect(2.39), colour: GUIDE_COLOUR, shown: true }
    }
}

/// The window's size the first time the app is opened, and the smallest it can be made.
pub const WINDOW_SIZE: [f32; 2] = [1280.0, 800.0];
pub const WINDOW_SMALLEST: [f32; 2] = [640.0, 400.0];
/// The start window's size at an interface of 100 %: the app's window is as large while it
/// shows the start window, and can't be made larger or smaller.
pub const START_SIZE: [f32; 2] = [680.0, 380.0];

/// Every preference has a default, so a file from an older version, or none, still reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preferences {
    pub new_drawing_hold: NewDrawingHold,
    /// Where a pen's pressure is read from, on Windows.
    #[serde(deserialize_with = "or_default")]
    pub tablet_input: TabletInput,
    /// In the substitution picker: how many frames a picked drawing shows for before the
    /// playhead moves on by as many. 0 keeps the playhead where it is.
    pub substitution_advance: u32,
    pub appearance: Appearance,
    /// The size of the interface in percent, on top of the system's display scale.
    pub ui_scale: u32,
    /// The grey around the camera frame, 0 (black) to 255 (white). It is not themed: colour is
    /// best judged against a neutral mid-grey (plan §7.5).
    pub canvas_surround: u8,
    /// The colours of the interface the user set, by what they are used for (Settings ▸
    /// Interface ▸ Colours): one set for Dark and Light. Those not set are the defaults.
    #[serde(deserialize_with = "or_default")]
    pub colours: crate::theme::Colours,
    /// Save a project that has a file on its own while it has changes. These saves leave the
    /// backup alone, so that stays the version the user last saved.
    pub autosave: bool,
    /// How long changes wait before they are autosaved.
    pub autosave_minutes: u32,
    /// How much memory the pictures of what went through effect nodes may take, in MB, in
    /// the viewport and in each render (plan §8): the ones not used for longest go first.
    pub effect_memory: u32,
    /// How much memory the playback cache's frames may take on the card, in MB.
    pub playback_memory: u32,
    /// The graphics API the window and the compositor draw with, taken when the app starts.
    #[serde(deserialize_with = "or_default")]
    pub graphics_backend: GraphicsBackend,
    /// The graphics card to draw with, by its name, taken when the app starts. None leaves the
    /// choice to the system: the fastest card that can draw the window.
    pub graphics_card: Option<String>,
    /// The keymap in use: a preset's name, or a file's in the `keymaps` folder (plan §7.6).
    pub keymap: String,
    /// Each page's layout as it was left (plan §7.4). One this version can't read, from a
    /// version with other panels, gives way to the default.
    #[serde(deserialize_with = "or_default")]
    pub layouts: Layouts,
    /// Keep each page's layout as it is left. Off, each start opens `fixed_layouts`.
    pub keep_layouts: bool,
    /// The pages' layouts as they were when keeping changes was switched off, or a workspace
    /// was last saved or taken since.
    #[serde(deserialize_with = "or_default")]
    pub fixed_layouts: Option<Layouts>,
    /// The panels that are in windows of their own (plan §7.4).
    #[serde(deserialize_with = "or_default")]
    pub detached: Vec<Panel>,
    /// Which defaults `layouts` grew from. When the defaults of a page change in a way that
    /// an old layout would hide, that page's layout is given up for the new default.
    pub layouts_version: u32,
    /// The pages the user took out of the page bar.
    #[serde(deserialize_with = "or_default")]
    pub hidden_pages: Vec<Page>,
    /// The layouts the user saved under a name (plan §7.4).
    #[serde(deserialize_with = "or_default")]
    pub workspaces: std::collections::BTreeMap<String, DockNode>,
    /// The folders of the library (plan §4.12): where the user keeps templates.
    #[serde(deserialize_with = "or_default")]
    pub library_folders: Vec<PathBuf>,
    /// The drawing converter the Import Rig window was last given (plan §4.17).
    #[serde(deserialize_with = "or_default")]
    pub rig_converter: Option<PathBuf>,
    /// The folders pinned in the Media Storage panel.
    #[serde(deserialize_with = "or_default")]
    pub storage_favourites: Vec<PathBuf>,
    /// The project files last opened or saved, the newest first (*File ▸ Open Recent*).
    #[serde(deserialize_with = "or_default")]
    pub recent_files: Vec<PathBuf>,
    /// The guides that are shown over the camera's frame.
    #[serde(deserialize_with = "or_default")]
    pub guides: Vec<Guide>,
    /// The aspect ratio outside which the frame is darkened, if a mask is on.
    pub aspect_mask: Option<f64>,
    /// Whether the viewer on the Storyboard and Animation pages shows only what is in the
    /// camera's frame, as a render does; off, a drawing larger than the frame is seen whole.
    #[serde(deserialize_with = "or_default")]
    pub clip: bool,
    /// The guides of the user's own.
    #[serde(deserialize_with = "or_default")]
    pub user_guides: Vec<UserGuide>,
    /// The render presets of the user's own (plan §7.14).
    #[serde(deserialize_with = "or_default")]
    pub render_presets: Vec<iskra_export::Preset>,
    /// The Storyboard PDF window's options as they were last set, whether it writes a PNG of
    /// each page too, and its presets of the user's own (plan §4.7).
    #[serde(deserialize_with = "or_default")]
    pub print_options: iskra_export::pages::PageOptions,
    #[serde(deserialize_with = "or_default")]
    pub print_pngs: bool,
    #[serde(deserialize_with = "or_default")]
    pub print_presets: Vec<iskra_export::pages::PrintPreset>,
    /// Moving a peg in depth in the Properties panel keeps the size it is seen at.
    pub keep_size: bool,
    /// Moving a peg in depth in the top and side views keeps the size it is seen at: the
    /// views' own switch, in their bars, whatever Properties' says.
    pub depth_keep_size: bool,
    /// The log says what the pen does at each touch, and every stroke that is painted: for
    /// finding out what a tablet sends. Off, both are left out, and the log is of the rest.
    pub log_strokes: bool,
    /// The log says how long frames, their parts and commands take (`timings`), to find out
    /// why the app slows down.
    pub log_timings: bool,
    /// Scripts may write files, and read them outside the project's folder (plan §9.3).
    pub scripts_write_files: bool,
    /// The user's name, which the retake notes they write are signed with (plan §9.6).
    pub user_name: String,
    /// The row of script buttons, one shared by every bar it is put in (plan §9.4).
    #[serde(deserialize_with = "or_default")]
    pub script_buttons: Vec<crate::buttons::ScriptButton>,
    /// The Animate switch: what is moved, turned or scaled is keyed on the frame it is done
    /// on. Off, no key is made, and what has keys is moved on all of them.
    pub animate: bool,
    /// What the Animate switch is at each start of the app, on the Animation page; the
    /// Storyboard page's starts off.
    pub animate_at_start: bool,
    /// Adjust, the switch beside Animate: what is keyed on one frame is changed alike on the
    /// keyframes picked on the timeline, of the same peg, bones or lens.
    pub adjust: bool,
    /// The Brushes panel lists the strokes alone, smaller, without their names.
    pub brush_strokes_only: bool,
    /// The Text panel shows each font's name set in it (plan §4.19).
    pub font_previews: bool,
    /// The Export page: its film's duration in frames, not timecode; and how the playhead's
    /// place under its viewer is read.
    pub duration_in_frames: bool,
    #[serde(deserialize_with = "or_default")]
    pub time_reading: TimeReading,
    /// Standing still, the viewport is drawn on the graphics card (View menu).
    pub viewport_on_card: bool,
    /// Playing, the playhead waits for a frame that is slow to make, and no frame is skipped;
    /// the sound is stopped while the picture is behind, and goes on with it once it keeps
    /// time (Play menu). Off, playing keeps time, and frames that are not ready are skipped.
    pub play_every_frame: bool,
    /// The Animation timeline has no rows for the boards (View ▸ Boards as Reference, off).
    pub hide_boards: bool,
    /// A click on a timeline's row or keyframe selects it and leaves the playhead where it is
    /// (Settings ▸ Interface). The ruler and a drag still move it.
    pub hold_playhead: bool,
    /// The node view shows the whole graph small in its corner (its bar's *Map*).
    pub node_map: bool,
    /// On the Storyboard page the viewer shows the two shots of a transition mixed while the
    /// film stands still or is scrubbed, as it does while it plays (View menu). Off, it shows
    /// the shot in hand alone, as it is drawn on.
    pub mix_at_rest: bool,
    /// The Brushes panel shows the settings of the brush in hand under the presets.
    pub brush_settings: bool,
    /// The Palette panel lists the swatches with their names, one to a row.
    pub palette_names: bool,
    /// The art layer in hand is the colour art, under the lines, and not the line art.
    pub colour_art: bool,
    /// The onion skin: how many drawings or frames before and after the current one, how
    /// opaque the nearest is, and whether its drawings show their line art or outlines alone.
    #[serde(default)]
    pub onion: OnionCounts,
    /// Which art layer alone the viewport shows, if one is (All, Line and Colour in the bar).
    #[serde(default)]
    pub shown_art: Option<iskra_model::ArtLayerKind>,
    /// It is shown so of every drawing, and not only of the one in hand.
    #[serde(default)]
    pub shown_art_all: bool,
    /// The chains of every rigged layer are seen faint, whatever tool is in hand (the Bones
    /// tool's *Always Visible*).
    #[serde(default)]
    pub bones_always: bool,
    /// On vector art the Brush leaves surfaces, not lines (its *Surface* row's Brush).
    #[serde(default)]
    pub brush_surface: bool,
    /// The colour painted on free colour, where it is not the swatch's (sRGB, straight alpha).
    pub free_colour: Option<[u8; 4]>,
    /// The colours painted on free colour lately, the last first, to take again.
    pub recent_colours: Vec<[u8; 4]>,
    /// How large the window was when the app was last closed, inside its frame, in points of
    /// the system's, and whether it filled the screen. The app opens so again.
    pub window_size: Option<[f32; 2]>,
    pub window_maximized: bool,
    /// Where the window was last put, its outer corner in the system's points, and how large
    /// the display it was on was: it opens there while that display is there, and as large.
    pub window_position: Option<[f32; 2]>,
    pub window_display: Option<[f32; 2]>,
    /// The drawing guide that was in hand, with the project file it is of: the app takes it in
    /// hand again when that project is opened.
    pub guide_in_hand: Option<(std::path::PathBuf, u64)>,
    /// The bars over the two timelines, as preferences of before 2026-11-01 kept them: read
    /// only, and put in `panel_bars` when the preferences are read.
    #[serde(deserialize_with = "or_default", skip_serializing)]
    pub(crate) timeline_bar: crate::bar::BarLayout,
    #[serde(deserialize_with = "or_default", skip_serializing)]
    pub(crate) storyboard_bar: crate::bar::BarLayout,
    /// How the timing charts' lines are shown and dragged, on both timelines.
    #[serde(deserialize_with = "or_default")]
    pub breakdowns: crate::breakdown::Breakdowns,
    /// What the storyboard timeline's clips are coloured by, and the colours of the scenes
    /// and sequences: the user's own, not the project's.
    #[serde(deserialize_with = "or_default")]
    pub clip_colours: crate::scenes::ClipColours,
    /// How the Rename Shots window last named shots, which it opens with.
    #[serde(deserialize_with = "or_default")]
    pub shot_numbering: iskra_model::Numbering,
    /// The bars over the panels that have one (`bar::of`), by the panel: which of their parts
    /// are shown, and in what order. A bar as it starts is not kept.
    #[serde(deserialize_with = "or_default")]
    pub panel_bars: std::collections::BTreeMap<crate::dock::Panel, crate::bar::BarLayout>,
    /// How wide the Animation timeline's column of names was dragged to.
    pub timeline_names: Option<f32>,
    /// The same for the storyboard timeline's column of names.
    pub storyboard_names: Option<f32>,
    /// How many points a frame of the storyboard timeline is wide, if it was zoomed.
    /// How wide a frame is on the Animation timeline, where it was zoomed.
    pub timeline_zoom: Option<f32>,
    pub storyboard_zoom: Option<f32>,
    /// How wide a card's picture is in the Panels panel, in points.
    pub card_width: f32,
    /// The Panels panel shows the notes of the panel under the playhead at its foot.
    pub panel_notes: bool,
    /// What a new project is made with in the start window, and the presets it offers, kept
    /// in Settings ▸ New Projects.
    pub new_project: NewProject,
}

/// 2: the Storyboard page lost the shot timeline and gained the Layers panel.
/// 3: the Media page became bins beside a media pool.
/// 4: the Media page gained media storage over them.
/// 5: the Storyboard page gained the Panels and the Shot Notes panels.
/// 6: Shot Notes became the foot of the Panels panel, which took its place.
const LAYOUTS_VERSION: u32 = 8;

/// The layout of each page that has a dock.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layouts {
    pub media: DockNode,
    pub storyboard: DockNode,
    pub animation: DockNode,
}

impl Default for Layouts {
    fn default() -> Self {
        Self { media: DockNode::for_page(Page::Media), storyboard: DockNode::for_page(Page::Storyboard), animation: DockNode::for_page(Page::Animation) }
    }
}

impl Layouts {
    /// A page's layout. The Export page has no dock and stands in with the Animation page's.
    pub fn of(&self, page: Page) -> &DockNode {
        match page {
            Page::Media => &self.media,
            Page::Storyboard => &self.storyboard,
            Page::Animation | Page::Export => &self.animation,
        }
    }

    /// Every page's layout.
    pub fn all_mut(&mut self) -> [&mut DockNode; 3] {
        [&mut self.media, &mut self.storyboard, &mut self.animation]
    }

    pub fn of_mut(&mut self, page: Page) -> &mut DockNode {
        match page {
            Page::Media => &mut self.media,
            Page::Storyboard => &mut self.storyboard,
            Page::Animation | Page::Export => &mut self.animation,
        }
    }
}

/// Read a value, or take its default if what is there doesn't fit.
fn or_default<'de, D: serde::Deserializer<'de>, T: Default + serde::de::DeserializeOwned>(deserializer: D) -> Result<T, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(deserializer)?).unwrap_or_default())
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            new_drawing_hold: NewDrawingHold::default(),
            tablet_input: TabletInput::default(),
            substitution_advance: 0,
            appearance: Appearance::default(),
            ui_scale: 100,
            canvas_surround: 128,
            colours: Default::default(),
            autosave: true,
            autosave_minutes: 1,
            effect_memory: 512,
            playback_memory: 768,
            graphics_backend: GraphicsBackend::Auto,
            graphics_card: None,
            keymap: "Harmony".to_string(),
            layouts: Layouts::default(),
            keep_layouts: true,
            fixed_layouts: None,
            hidden_pages: Vec::new(),
            layouts_version: LAYOUTS_VERSION,
            detached: Vec::new(),
            workspaces: Default::default(),
            library_folders: Vec::new(),
            rig_converter: None,
            storage_favourites: Vec::new(),
            recent_files: Vec::new(),
            guides: Vec::new(),
            aspect_mask: None,
            clip: false,
            user_guides: Vec::new(),
            render_presets: Vec::new(),
            print_options: iskra_export::pages::PageOptions::default(),
            print_pngs: false,
            print_presets: Vec::new(),
            keep_size: false,
            depth_keep_size: false,
            log_strokes: false,
            log_timings: false,
            scripts_write_files: false,
            script_buttons: Vec::new(),
            user_name: String::new(),
            animate: true,
            animate_at_start: true,
            adjust: false,
            brush_strokes_only: false,
            font_previews: true,
            viewport_on_card: false,
            duration_in_frames: false,
            time_reading: TimeReading::TimelineTimecode,
            play_every_frame: true,
            hide_boards: false,
            hold_playhead: false,
            node_map: false,
            mix_at_rest: true,
            brush_settings: false,
            palette_names: false,
            colour_art: false,
            shown_art: None,
            shown_art_all: false,
            bones_always: false,
            brush_surface: false,
            onion: OnionCounts::default(),
            free_colour: None,
            recent_colours: Vec::new(),
            window_size: None,
            window_maximized: false,
            window_position: None,
            window_display: None,
            guide_in_hand: None,
            timeline_bar: Default::default(),
            storyboard_bar: Default::default(),
            breakdowns: Default::default(),
            clip_colours: Default::default(),
            shot_numbering: Default::default(),
            panel_bars: Default::default(),
            timeline_names: None,
            storyboard_names: None,
            storyboard_zoom: None,
            timeline_zoom: None,
            card_width: crate::boards::CARD_WIDTH,
            panel_notes: true,
            new_project: NewProject::default(),
        }
    }
}

impl Preferences {
    /// The preferences in the file at `path`, or the defaults if it is missing or unreadable.
    pub fn load(path: &Path) -> Self {
        let mut read: Self = std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        if read.layouts_version < 2 {
            read.layouts.storyboard = DockNode::for_page(Page::Storyboard);
        }
        if read.layouts_version < 4 {
            read.layouts.media = DockNode::for_page(Page::Media);
        }
        // A layout the user arranged is kept: the panels the page gained are put into it, each
        // as a tab behind the one it is beside in the page's own layout.
        if read.layouts_version < 5 && !read.layouts.storyboard.contains(Panel::Panels) {
            read.layouts.storyboard.open_near(Panel::Panels, Panel::Layers);
            read.layouts.storyboard.activate(Panel::Layers);
        }
        // Shot Notes is in the Panels panel now: the Panels panel takes its place, and is
        // shown where it was shown.
        if read.layouts_version < 6 {
            for layout in read.layouts.all_mut() {
                if !layout.contains(Panel::ShotNotes) {
                    continue;
                }
                let shown = layout.shows(Panel::ShotNotes);
                layout.close(Panel::Panels);
                layout.open_near(Panel::Panels, Panel::ShotNotes);
                layout.close(Panel::ShotNotes);
                if shown {
                    layout.activate(Panel::Panels);
                }
            }
        }
        // The tool's options are in Tool Properties alone, the bar over the window hidden: the
        // Storyboard page, which had none, has it as a tab beside the Panels panel, behind
        // what is shown there.
        if read.layouts_version < 7 && !read.layouts.storyboard.contains(Panel::ToolProperties) {
            let layout = &mut read.layouts.storyboard;
            let shown: Vec<Panel> = layout.panels().into_iter().filter(|panel| layout.shows(*panel)).collect();
            layout.open_near(Panel::ToolProperties, Panel::Panels);
            for panel in shown {
                layout.activate(panel);
            }
        }
        // The Media page's Preview, at the top right beside the disk, where the page has none.
        if read.layouts_version < 8 && !read.layouts.media.contains(Panel::Preview) {
            let layout = &mut read.layouts.media;
            if !(layout.contains(Panel::Storage) && layout.dock_panel(Panel::Preview, Panel::Storage, crate::dock::Zone::Right)) {
                layout.open_near(Panel::Preview, Panel::Pool);
            }
        }
        read.layouts_version = LAYOUTS_VERSION;
        // The timelines' bars were kept apart from the other panels'.
        for (panel, layout) in [(Panel::Timeline, std::mem::take(&mut read.timeline_bar)), (Panel::Storyboard, std::mem::take(&mut read.storyboard_bar))] {
            if layout != Default::default() {
                read.panel_bars.entry(panel).or_insert(layout);
            }
        }
        // The Animation timeline's Ripple is in Edit now, with Snap, as the storyboard's is; and
        // its Keys are Convert to Hold, and its Drawings and Cells, with New Drawings Hold, are
        // Drawings, as the storyboard's Add Layer and Drawing are.
        if let Some(layout) = read.panel_bars.get_mut(&Panel::Timeline) {
            layout.rename("ripple", "edit");
            layout.rename("keys", "convert");
            layout.rename("cell", "drawings");
        }
        if let Some(layout) = read.panel_bars.get_mut(&Panel::Storyboard) {
            layout.rename("drawing", "drawings");
        }
        read
    }

    /// How the bar over `panel` is laid out.
    pub fn bar(&self, panel: Panel) -> crate::bar::BarLayout {
        self.panel_bars.get(&panel).cloned().unwrap_or_default()
    }

    /// Keep how the bar over `panel` is laid out: not at all, if it is as it starts.
    pub fn set_bar(&mut self, panel: Panel, layout: crate::bar::BarLayout) {
        if layout == crate::bar::BarLayout::default() {
            self.panel_bars.remove(&panel);
        } else {
            self.panel_bars.insert(panel, layout);
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// The size the window opens at: as it was left, or 1280 by 800 the first time. Never
    /// smaller than the window can be made, nor a size that is no size.
    pub fn opening_size(&self) -> [f32; 2] {
        let [width, height] = self.window_size.filter(|size| size.iter().all(|side| side.is_finite())).unwrap_or(WINDOW_SIZE);
        [width.clamp(WINDOW_SMALLEST[0], 16_384.0), height.clamp(WINDOW_SMALLEST[1], 16_384.0)]
    }

    /// The size the window opens at to show the start window, in the system's points: as
    /// large as the interface is.
    pub fn start_size(&self) -> [f32; 2] {
        let scale = self.ui_scale.clamp(50, 300) as f32 / 100.0;
        START_SIZE.map(|side| side * scale)
    }

    /// Where the window opens: where it was left, if that is known.
    pub fn opening_position(&self) -> Option<[f32; 2]> {
        self.window_position.filter(|corner| corner.iter().all(|at| at.is_finite()))
    }

    /// Whether a window put where it was left is there, with its corner at `corner` on a
    /// display of the size `display`, as it was: not where the display it was on is gone, or
    /// is another size now, and the system has put it elsewhere or left it off every screen.
    pub fn kept_place_holds(&self, corner: [f32; 2], display: [f32; 2]) -> bool {
        let near = |a: [f32; 2], b: [f32; 2]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 2.0);
        self.opening_position().is_some_and(|kept| near(kept, corner)) && self.window_display.is_some_and(|kept| near(kept, display))
    }

    /// Where the preferences file is on this system, if the system says where its config
    /// folder is.
    pub fn default_path() -> Option<PathBuf> {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
        let folder = if cfg!(target_os = "macos") {
            var("HOME")?.join("Library/Application Support/Iskra")
        } else if cfg!(windows) {
            var("APPDATA")?.join("Iskra")
        } else {
            var("XDG_CONFIG_HOME").or_else(|| Some(var("HOME")?.join(".config")))?.join("iskra")
        };
        Some(folder.join("preferences.json"))
    }
}

/// A resolution a new project can be made with: its name, and its width and height in pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// The start window's new project (the user's of 2026-10-26): the presets it offers, kept in
/// Settings ▸ New Projects, and what was chosen last, which it opens with again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NewProject {
    pub resolutions: Vec<Resolution>,
    /// Frames a second.
    pub frame_rates: Vec<u32>,
    /// The folder the project's own folder is made in.
    pub folder: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
}

impl Default for NewProject {
    fn default() -> Self {
        let resolutions =
            crate::RESOLUTIONS.iter().map(|(name, width, height)| Resolution { name: name.to_string(), width: *width, height: *height }).collect();
        let frame_rates = crate::FRAME_RATES.iter().map(|fps| *fps as u32).collect();
        NewProject { resolutions, frame_rates, folder: None, width: 1920, height: 1080, frame_rate: 24 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tablet_input_is_kept_and_a_file_without_it_is_automatic() {
        let read: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(read.tablet_input, TabletInput::Automatic);
        let kept = serde_json::to_string(&Preferences { tablet_input: TabletInput::WinTab, ..Preferences::default() }).unwrap();
        assert_eq!(serde_json::from_str::<Preferences>(&kept).unwrap().tablet_input, TabletInput::WinTab);
        let unknown: Preferences = serde_json::from_str(r#"{"tabletInput": "somethingElse"}"#).unwrap();
        assert_eq!(unknown.tablet_input, TabletInput::Automatic, "a value this version doesn't know");
        assert!(TabletInput::Automatic.reads_wintab() && TabletInput::Automatic.reads_ink());
        assert!(!TabletInput::WindowsInk.reads_wintab() && !TabletInput::WinTab.reads_ink());
    }

    #[test]
    fn preferences_are_saved_and_read_back_and_default_when_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("preferences.json");
        assert_eq!(Preferences::load(&path), Preferences::default(), "no file yet");
        let on_twos = Preferences { new_drawing_hold: NewDrawingHold::Frames(2), appearance: Appearance::Light, ..Preferences::default() };
        on_twos.save(&path).unwrap();
        assert_eq!(Preferences::load(&path), on_twos);
        std::fs::write(&path, "{}").unwrap();
        assert_eq!(Preferences::load(&path).new_drawing_hold, NewDrawingHold::ToNext, "a preference the file lacks has its default");
        std::fs::write(&path, r#"{ "uiScale": 125, "layouts": { "animation": { "tabs": { "panels": ["xSheet"], "active": 0 } } } }"#).unwrap();
        let read = Preferences::load(&path);
        assert_eq!((read.ui_scale, &read.layouts), (125, &Layouts::default()), "a layout that can't be read costs only the layout");
        std::fs::write(&path, r#"{ "layoutsVersion": 1, "layouts": { "storyboard": { "tabs": { "panels": ["timeline"], "active": 0 } } } }"#).unwrap();
        assert_eq!(Preferences::load(&path), Preferences::default(), "a page whose default changed starts from the new one");
        // A layout from before the Storyboard page had its Panels panel gets it, behind the
        // layers, and is otherwise as the user left it.
        let old = r#"{ "layoutsVersion": 4, "layouts": { "storyboard": { "split": { "vertical": false, "size": { "ratio": 0.5 },
            "a": { "tabs": { "panels": ["layers"], "active": 0 } }, "b": { "tabs": { "panels": ["viewport", "storyboard"], "active": 0 } } } } } }"#;
        std::fs::write(&path, old).unwrap();
        let read = Preferences::load(&path).layouts.storyboard;
        assert_eq!(read.panels(), [Panel::Layers, Panel::Panels, Panel::ToolProperties, Panel::Viewport, Panel::Storyboard]);
        assert!(read.shows(Panel::Layers) && read.shows(Panel::Viewport) && !read.shows(Panel::Panels) && !read.shows(Panel::ToolProperties));
        // One from before Shot Notes went into the Panels panel: the Panels panel takes its
        // place, shown as it was, and is no more behind the viewer.
        let old = r#"{ "layoutsVersion": 5, "layouts": { "storyboard": { "split": { "vertical": false, "size": { "ratio": 0.5 },
            "a": { "tabs": { "panels": ["viewport", "panels"], "active": 0 } }, "b": { "tabs": { "panels": ["layers", "shotNotes"], "active": 1 } } } } } }"#;
        std::fs::write(&path, old).unwrap();
        let read = Preferences::load(&path).layouts.storyboard;
        assert_eq!(read.panels(), [Panel::Viewport, Panel::Layers, Panel::Panels, Panel::ToolProperties]);
        assert!(read.shows(Panel::Viewport) && read.shows(Panel::Panels));
        // The timelines' bars, from before they were kept with the other panels': kept there,
        // and not written apart again.
        std::fs::write(&path, r#"{ "timelineBar": { "hidden": ["zoom", "ripple"] }, "storyboardBar": { "order": ["name"] } }"#).unwrap();
        let read = Preferences::load(&path);
        let (timeline, storyboard) = (read.bar(Panel::Timeline), read.bar(Panel::Storyboard));
        assert!(
            timeline.hidden.contains("zoom")
                && timeline.hidden.contains("edit")
                && storyboard.order == ["name"]
                && read.bar(Panel::Viewport) == Default::default()
        );
        read.save(&path).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("timelineBar") && Preferences::load(&path) == read, "{written}");
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(Preferences::load(&path), Preferences::default());
    }

    #[test]
    fn the_window_opens_as_large_as_it_was_left() {
        assert_eq!(Preferences::default().opening_size(), WINDOW_SIZE);
        let left = |size| Preferences { window_size: Some(size), ..Preferences::default() }.opening_size();
        assert_eq!(left([1600.0, 950.0]), [1600.0, 950.0]);
        assert_eq!(left([10.0, f32::NAN]), WINDOW_SIZE);
        assert_eq!(left([10.0, 900.0]), [640.0, 900.0], "never smaller than it can be made");
    }

    #[test]
    fn the_window_opens_where_it_was_left_while_its_display_is_as_it_was() {
        assert_eq!(Preferences::default().opening_position(), None, "the first time, centred");
        let left = Preferences { window_position: Some([2100.0, 140.0]), window_display: Some([1920.0, 1080.0]), ..Preferences::default() };
        assert_eq!(left.opening_position(), Some([2100.0, 140.0]));
        assert!(left.kept_place_holds([2100.0, 140.5], [1920.0, 1080.0]));
        assert!(!left.kept_place_holds([2100.0, 140.0], [2560.0, 1440.0]), "the display is another size");
        assert!(!left.kept_place_holds([300.0, 140.0], [1920.0, 1080.0]), "the system put it elsewhere");
        let nowhere = Preferences { window_position: Some([f32::NAN, 0.0]), ..left };
        assert_eq!(nowhere.opening_position(), None);
        assert_eq!(Preferences { ui_scale: 150, ..Preferences::default() }.start_size(), [1020.0, 570.0]);
    }

    #[test]
    fn the_hold_is_given_to_commands_as_frames_or_to_next() {
        assert_eq!(NewDrawingHold::ToNext.param(), json!("toNext"));
        assert_eq!((NewDrawingHold::Frames(2).param(), NewDrawingHold::Frames(0).param()), (json!(2), json!(1)));
    }
}

/// The onion skin's settings that are kept from one start to the next.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OnionCounts {
    pub before: u32,
    pub after: u32,
    pub opacity: f32,
    pub line_art_only: bool,
    pub outline: bool,
}

impl Default for OnionCounts {
    fn default() -> Self {
        let display = crate::onion::Display::default();
        Self { before: display.before, after: display.after, opacity: display.opacity, line_art_only: display.line_art_only, outline: display.outline }
    }
}
