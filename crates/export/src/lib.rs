//! Export (plan §7.2a, §7.14, R6): a shot rendered to files. For now image sequences; movies
//! come next. It is headless: the Export page, the engine's commands and the command line all
//! render through this.
//!
//! A shot is rendered frame by frame with the reference compositor (`iskra-render`), several
//! frames at a time, and each frame is written as a file of its own in a folder that is the
//! sequence's alone.

pub mod burn;
mod editlist;
mod movie;
pub mod pages;
pub mod pdf;
mod preset;
mod record;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use iskra_model::{Project, ShotId, Work, WorkingSpace};
use iskra_render::View;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

pub use burn::BurnIn;
pub use editlist::{ListFormat, edit_list, export_edit_list};
pub use movie::{export_movie, export_movie_picked, export_storyboard, export_storyboard_named, movie_file};
pub use preset::{PRESET_EXTENSION, Preset, PresetPasses};
pub use record::{is_stale, mark_of, record_of, settings_of};

/// The kind of picture file a sequence is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Png,
    Jpeg,
    Tiff,
    Exr,
    /// A QuickTime movie: ProRes or H.264, with uncompressed or AAC sound.
    Mov,
    /// An MP4 movie: H.264, with AAC sound.
    Mp4,
}

/// How a movie's picture is encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Codec {
    /// Apple ProRes: every frame whole, for post. In a QuickTime movie only.
    #[default]
    ProRes,
    H264,
}

/// The kind of ProRes: from the smallest to the best.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProRes {
    Proxy,
    Lt,
    Standard,
    #[default]
    Hq,
    /// Colour at every pixel, and transparency if it is asked for.
    P4444,
    P4444Xq,
}

impl ProRes {
    pub const ALL: [ProRes; 6] = [ProRes::Proxy, ProRes::Lt, ProRes::Standard, ProRes::Hq, ProRes::P4444, ProRes::P4444Xq];

    pub fn label(self) -> &'static str {
        match self {
            ProRes::Proxy => "ProRes 422 Proxy",
            ProRes::Lt => "ProRes 422 LT",
            ProRes::Standard => "ProRes 422",
            ProRes::Hq => "ProRes 422 HQ",
            ProRes::P4444 => "ProRes 4444",
            ProRes::P4444Xq => "ProRes 4444 XQ",
        }
    }

    /// Whether it can hold transparency.
    pub fn has_alpha(self) -> bool {
        matches!(self, ProRes::P4444 | ProRes::P4444Xq)
    }
}

/// How good an H.264 picture is, and so how large its file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    Best,
    /// Near what the eye can't tell from the picture itself.
    #[default]
    High,
    Medium,
    Low,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Best, Quality::High, Quality::Medium, Quality::Low];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Best => "Best",
            Quality::High => "High",
            Quality::Medium => "Medium",
            Quality::Low => "Low",
        }
    }
}

/// How a movie's sound is encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SoundCodec {
    /// Uncompressed, 16 bits. In a QuickTime movie only.
    #[default]
    Pcm,
    Aac,
}

/// The rates a movie's sound can have, in samples a second.
pub const SOUND_RATES: [u32; 3] = [44_100, 48_000, 96_000];

/// The depths uncompressed sound is written at: 16 and 24 bits of integers, 32-bit floats.
pub const SOUND_BITS: [u16; 3] = [16, 24, 32];

impl Format {
    pub const ALL: [Format; 6] = [Format::Png, Format::Jpeg, Format::Tiff, Format::Exr, Format::Mov, Format::Mp4];

    /// One file for the shot, with its sound, where the others are a file a frame.
    pub fn is_movie(self) -> bool {
        matches!(self, Format::Mov | Format::Mp4)
    }

    /// The ending of its files' names.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
            Format::Exr => "exr",
            Format::Mov => "mov",
            Format::Mp4 => "mp4",
        }
    }

    /// How many bits a channel it can hold, the least first.
    pub fn depths(self) -> &'static [u8] {
        match self {
            Format::Png | Format::Tiff => &[8, 16],
            Format::Jpeg | Format::Mov | Format::Mp4 => &[8],
            Format::Exr => &[32],
        }
    }

    /// Whether it can hold transparency.
    pub fn has_alpha(self) -> bool {
        !matches!(self, Format::Jpeg | Format::Mov | Format::Mp4)
    }
}

/// Which of a shot's frames are rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Range {
    /// The part of the shot that is in the cut, with this many frames of its handles before
    /// and after, as far as the shot has them. A shot on no storyboard is rendered whole.
    Cut { handles: u32 },
    /// All of the shot.
    Whole,
}

/// How a shot is rendered to a sequence.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub format: Format,
    /// Bits a channel: 8, 16, or 32 for floats (R24).
    pub bits: u8,
    /// With transparency where nothing is painted. Without, the picture is on the matte.
    pub alpha: bool,
    /// The colour under the picture when it has no transparency: the paper.
    pub matte: [u8; 3],
    /// The picture's size, as a part of the shot's: 1 is the shot's own.
    pub scale: f64,
    pub range: Range,
    /// A movie has the shot's sound in it.
    pub sound: bool,
    /// A movie's picture: the codec, and the kind of it that is its own.
    pub codec: Codec,
    pub prores: ProRes,
    pub quality: Quality,
    /// H.264 is held to this many thousand bits a second, in place of a quality.
    pub video_kbps: Option<u32>,
    /// The picture's size in pixels, if it is not the shot's at `scale`: the shot is fitted
    /// into it whole, with the matte, or nothing, at the sides it does not reach.
    pub size: Option<[u32; 2]>,
    /// A movie's sound: the codec, the bits a second of AAC in thousands, both channels
    /// together, and the samples a second.
    pub sound_codec: SoundCodec,
    pub sound_kbps: u32,
    pub sound_rate: u32,
    /// Bits of a sample of uncompressed sound: 16 or 24, or 32 for floats.
    pub sound_bits: u16,
    /// An image sequence has the sound under its frames beside them, as a WAV file.
    pub sound_beside: bool,
    /// The rate the render is written at; the project's where none is said. At another, each
    /// frame shows the project's frame that is on screen at its moment: frames are repeated
    /// to go faster and left out to go slower, and the sound is as long as the picture.
    pub frame_rate: Option<iskra_model::FrameRate>,
    /// Which of a shot is in the picture (plan §7.2c): its animation, unless an animatic of
    /// its boards is what is made.
    pub work: Work,
    /// What is burned into the picture over the film: guides, the mask, the shot's name, the
    /// frame's count.
    pub burn_in: BurnIn,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            format: Format::Png,
            bits: 8,
            alpha: true,
            matte: [255, 255, 255],
            scale: 1.0,
            range: Range::Cut { handles: 0 },
            sound: true,
            codec: Codec::ProRes,
            prores: ProRes::Hq,
            quality: Quality::High,
            video_kbps: None,
            size: None,
            sound_codec: SoundCodec::Pcm,
            sound_kbps: 256,
            sound_rate: 48_000,
            sound_bits: 16,
            sound_beside: false,
            frame_rate: None,
            work: Work::Animation,
            burn_in: BurnIn::default(),
        }
    }
}

impl Settings {
    /// The settings as the format can hold them: a depth it has, and no transparency where
    /// it has none.
    pub fn fitted(mut self) -> Self {
        let depths = self.format.depths();
        if !depths.contains(&self.bits) {
            // The nearest depth it has that is no less, else its deepest.
            self.bits = depths.iter().copied().find(|bits| *bits >= self.bits).unwrap_or(depths[depths.len() - 1]);
        }
        // An MP4 file is H.264 with AAC.
        if self.format == Format::Mp4 {
            (self.codec, self.sound_codec) = (Codec::H264, SoundCodec::Aac);
        }
        self.alpha &= self.can_alpha();
        self.size = self.size.filter(|[width, height]| *width > 0 && *height > 0);
        self.sound_kbps = self.sound_kbps.clamp(64, 512);
        self.video_kbps = self.video_kbps.map(|kbps| kbps.clamp(100, 200_000));
        if !SOUND_RATES.contains(&self.sound_rate) {
            self.sound_rate = 48_000;
        }
        if !SOUND_BITS.contains(&self.sound_bits) {
            self.sound_bits = 16;
        }
        self.frame_rate = self.frame_rate.filter(|rate| rate.num > 0 && rate.den > 0);
        self.burn_in = self.burn_in.fitted();
        self
    }

    /// Whether what is made can hold transparency: a sequence of a format that has it, or a
    /// QuickTime movie of ProRes 4444.
    pub fn can_alpha(&self) -> bool {
        match self.format {
            Format::Mov => self.codec == Codec::ProRes && self.prores.has_alpha(),
            format => format.has_alpha(),
        }
    }
}

/// How what a render writes is named (plan §7.14).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Naming {
    /// The name in place of the shot's, or of the storyboard's for a film.
    pub name: Option<String>,
    /// What comes after the name: `_v2`, say.
    pub suffix: String,
    /// The folder under the place renders go to, `dailies/monday`. Empty for none.
    pub subfolder: String,
    /// How many digits a frame's number has in a sequence: 4 where none is said.
    pub digits: Option<u8>,
    /// A sequence's frames are numbered as they are in the film, not from the shot's first.
    pub film_numbers: bool,
}

/// What a render's files are named for (plan §7.14).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NameFrom {
    /// Each for what it is of: the shot, or the storyboard where that is one movie.
    #[default]
    Own,
    Storyboard,
    Custom,
    /// Made of the parts that are picked: the project's name, the storyboard's, the shot's
    /// sequence and scene, the shot's own.
    Parts,
}

/// What a shot's files can be named from: its own name, and the sequence and the scene it is
/// given to, empty for none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShotNames<'a> {
    pub name: &'a str,
    pub sequence: &'a str,
    pub scene: &'a str,
}

impl<'a> From<&'a iskra_model::Shot> for ShotNames<'a> {
    fn from(shot: &'a iskra_model::Shot) -> Self {
        ShotNames { name: &shot.name, sequence: &shot.sequence, scene: &shot.scene }
    }
}

/// How the files of a render are to be named, as the Export page's File tab says, and a
/// render preset keeps: what the name is made of, what comes after it, the folder it goes
/// into, and how a sequence's frames are numbered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileNaming {
    pub from: NameFrom,
    pub custom: String,
    pub suffix: String,
    pub subfolder: String,
    /// With [`NameFrom::Parts`]: the project's name, the storyboard's and the shot's, each
    /// in the name or not, in that order.
    pub parts: [bool; 3],
    /// With [`NameFrom::Parts`]: the shot's sequence and its scene, between the storyboard's
    /// name and the shot's, as `SQ010_SC020_SH030`. A shot in none is named without.
    pub sequence: bool,
    pub scene: bool,
    /// How many digits a frame's number has, 3 to 8.
    pub digits: u8,
    pub film_numbers: bool,
}

impl Default for FileNaming {
    fn default() -> Self {
        Self {
            from: NameFrom::Own,
            custom: String::new(),
            suffix: String::new(),
            subfolder: String::new(),
            parts: [false, false, true],
            sequence: false,
            scene: false,
            digits: 4,
            film_numbers: false,
        }
    }
}

impl FileNaming {
    /// The naming of one render: of the shot `own`, or of the storyboard as one movie
    /// without; `several` says that other shots are rendered beside it, which a name that is
    /// not the shot's own has to be told apart from, by the shot's name after it.
    pub fn naming(&self, project: &str, board: &str, own: Option<ShotNames>, several: bool) -> Naming {
        let shot = own.map(|own| own.name);
        let stem = match self.from {
            NameFrom::Own => None,
            NameFrom::Storyboard => Some(board.to_string()),
            NameFrom::Custom => Some(self.custom.trim().to_string()).filter(|name| !name.is_empty()),
            NameFrom::Parts => {
                // The shot's name is in it wherever several are rendered, so no two are alike.
                let [with_project, with_board, with_shot] = self.parts;
                let named = shot.filter(|_| with_shot || several);
                let sequence = own.map(|own| own.sequence).filter(|_| self.sequence);
                let scene = own.map(|own| own.scene).filter(|_| self.scene);
                let picked = [with_project.then_some(project), with_board.then_some(board), sequence, scene, named];
                let joined = picked.into_iter().flatten().map(str::trim).filter(|part| !part.is_empty()).collect::<Vec<_>>().join("_");
                return self.with(Some(joined).filter(|name| !name.is_empty()));
            }
        };
        let name = match (stem, shot) {
            (Some(stem), Some(own)) if several => Some(format!("{stem}_{own}")),
            (stem, _) => stem,
        };
        self.with(name)
    }

    fn with(&self, name: Option<String>) -> Naming {
        let digits = (self.digits != 4).then(|| self.digits.clamp(3, 8));
        Naming { name, suffix: self.suffix.trim().to_string(), subfolder: self.subfolder.trim().to_string(), digits, film_numbers: self.film_numbers }
    }
}

impl Naming {
    /// The number a sequence's frame `frame` of `shot` is written with, as the naming says:
    /// from 1, or as the film numbers it from 1.
    pub fn number(&self, project: &Project, shot: ShotId, frame: u32) -> i64 {
        i64::from(frame) + self.film_offset(project, shot) + 1
    }

    /// How far the film's numbers are ahead of the shot's, where frames are numbered as in
    /// the film: the film's frame of the shot's frame 0.
    fn film_offset(&self, project: &Project, shot: ShotId) -> i64 {
        let film = self.film_numbers.then(|| project.storyboard_of(shot)?.clip(shot).map(|clip| i64::from(clip.start) - i64::from(clip.in_point)));
        film.flatten().unwrap_or(0)
    }

    /// The number of the `at`th frame of a render at `rate` whose first frame shows the shot's
    /// frame `first`: counted at that rate from where the first is, in the shot or the film.
    pub fn number_at(&self, project: &Project, shot: ShotId, first: u32, at: u32, rate: iskra_model::FrameRate) -> i64 {
        let from = i64::from(first) + self.film_offset(project, shot);
        rescaled(from, project.settings.frame_rate, rate) + i64::from(at) + 1
    }

    /// The file of a frame numbered `number` of the sequence named `base`, in `folder`.
    pub fn frame_path(&self, folder: &Path, base: &str, number: i64, format: Format) -> PathBuf {
        let digits = usize::from(self.digits.unwrap_or(4));
        folder.join(format!("{base}.{number:0digits$}.{}", format.extension()))
    }

    /// Where the render goes, under `dest`. A subfolder never leads out of `dest`.
    pub fn dest(&self, dest: &Path) -> PathBuf {
        let parts = self.subfolder.split(['/', '\\']).map(str::trim).filter(|part| !part.is_empty() && *part != "." && *part != "..");
        parts.fold(dest.to_path_buf(), |dest, part| dest.join(file_name(part)))
    }

    /// What the render is called, where `own` is the name of what is rendered.
    pub fn named(&self, own: &str) -> String {
        let name = self.name.as_deref().map(str::trim).filter(|name| !name.is_empty()).unwrap_or(own);
        format!("{name}{}", self.suffix.trim())
    }
}

/// The picture a render makes: the shot as it is seen, and the size of the picture it is in,
/// which is larger where the shot is fitted into a size of another shape.
pub(crate) struct Canvas {
    view: View,
    pub width: u32,
    pub height: u32,
    left: u32,
    top: u32,
    /// What went through effect nodes, kept from frame to frame: what stands still under a
    /// blur is blurred once for the whole render.
    cache: iskra_render::Cache,
}

impl Canvas {
    /// The picture `settings` ask for. A movie's has an even number of pixels each way.
    pub fn of(project: &Project, settings: &Settings, movie: bool) -> Result<Canvas, ExportError> {
        let even = |pixels: u32| if movie { pixels & !1 } else { pixels };
        let (view, width, height) = match settings.size {
            Some([width, height]) => {
                let (width, height) = (even(width), even(height));
                let (wide, tall) = (f64::from(project.settings.width.max(1)), f64::from(project.settings.height.max(1)));
                let mut view = View::frame(&project.settings, (f64::from(width) / wide).min(f64::from(height) / tall));
                (view.width, view.height) = (view.width.min(width), view.height.min(height));
                (view, width, height)
            }
            None => {
                if !(settings.scale > 0.0 && settings.scale.is_finite()) {
                    return Err(ExportError::Invalid(format!("a picture at {} times the shot's size can't be made", settings.scale)));
                }
                let mut view = View::frame(&project.settings, settings.scale);
                (view.width, view.height) = (even(view.width), even(view.height));
                let (width, height) = (view.width, view.height);
                (view, width, height)
            }
        };
        let fits = if movie { (2..=16_384).contains(&width) && (2..=16_384).contains(&height) } else { u64::from(width) * u64::from(height) <= 1 << 28 };
        if !fits || view.width == 0 || view.height == 0 {
            return Err(ExportError::Invalid(match settings.size {
                Some([width, height]) => format!("a picture of {width} by {height} pixels can't be made"),
                None => format!("a picture at {} times the shot's size can't be made", settings.scale),
            }));
        }
        Ok(Canvas { left: (width - view.width) / 2, top: (height - view.height) / 2, view, width, height, cache: iskra_render::Cache::default() })
    }

    /// Burn what the settings say into a picture of the canvas, over the shot's frame in it:
    /// with `name` as the shot's name and the frame `count`ed from 0 at the project's rate.
    pub fn burn(&self, picture: &mut [[f32; 4]], project: &Project, settings: &Settings, name: Option<&str>, count: i64) {
        if settings.burn_in.any() {
            let counter = settings.burn_in.count(count, project.settings.frame_rate);
            let frame = [self.left, self.top, self.view.width, self.view.height];
            burn::burn(picture, self.width, self.height, frame, project.settings.working_space, &settings.burn_in, name, &counter);
        }
    }

    /// A frame of a shot, composited: colour times alpha, with nothing around the shot where
    /// the picture is larger.
    pub fn composite(&self, shot: &iskra_model::Shot, palette: &iskra_model::Palette, space: WorkingSpace, frame: u32) -> Vec<[f32; 4]> {
        let inner = self.cache.composite(shot, palette, space, frame, &self.view);
        if (self.view.width, self.view.height) == (self.width, self.height) {
            return inner;
        }
        let (wide, across) = (self.view.width as usize, self.width as usize);
        let mut whole = vec![[0.0; 4]; across * self.height as usize];
        for (row, line) in inner.chunks(wide).enumerate() {
            let at = (self.top as usize + row) * across + self.left as usize;
            whole[at..at + wide].copy_from_slice(line);
        }
        whole
    }
}

/// Which of a shot's passes a render shows (plan §7.14).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pick {
    /// The Pass rendered, by its name: what flows into the shot's Pass of that name. None, or
    /// a name the shot has no Pass of, is the shot's whole picture, its main display.
    pub pass: Option<String>,
    /// The render is an output of its own, among others of the shot: its name goes into the
    /// names of the files, which lie a folder down, `Shot_01/Background/Shot_01_Background.0001.png`
    /// for a sequence and `Background/Shot_01_Background.mov` for a movie.
    pub named: Option<String>,
}

impl Pick {
    /// The shot as the pick sees it: through its Pass, or whole.
    pub fn of(&self, shot: &iskra_model::Shot) -> Option<iskra_model::Shot> {
        shot.seen_as_pass(self.pass.as_deref()?)
    }
}

/// A number of frames at `from` as many at `to`, as long, rounded down.
fn rescaled(frames: i64, from: iskra_model::FrameRate, to: iskra_model::FrameRate) -> i64 {
    if from == to {
        return frames;
    }
    let (frames, from_num, from_den, to_num, to_den) = (i128::from(frames), i128::from(from.num), i128::from(from.den), i128::from(to.num), i128::from(to.den));
    ((frames * from_den * to_num).div_euclid(from_num * to_den)) as i64
}

/// The rate a render is written at: the settings', or the project's.
pub(crate) fn rate_of(project: &Project, settings: &Settings) -> iskra_model::FrameRate {
    settings.frame_rate.unwrap_or(project.settings.frame_rate)
}

/// Which of `count` frames of the project each frame of a render at `rate` shows, counted
/// from the first: the one on screen at its moment. At the project's rate, each in turn.
pub(crate) fn resampled(project: &Project, count: u32, rate: iskra_model::FrameRate) -> Vec<u32> {
    let own = project.settings.frame_rate;
    if rate == own || count == 0 {
        return (0..count).collect();
    }
    let (own_num, own_den, num, den) = (i128::from(own.num), i128::from(own.den), i128::from(rate.num), i128::from(rate.den));
    // As long as the project's frames: every frame of the render that begins before they end.
    let made = (i128::from(count) * own_den * num + own_num * den - 1) / (own_num * den);
    (0..made).map(|at| ((at * den * own_num) / (num * own_den)).min(i128::from(count) - 1) as u32).collect()
}

/// What to do when a sequence's folder has frames in it already.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IfExists {
    /// Don't render: say that it is there.
    #[default]
    Fail,
    /// Write over what is there.
    Replace,
    /// Write beside it, as the next version: `Shot_01_v002`.
    Version,
    /// Leave it, and count the shot as done.
    Skip,
}

#[derive(Debug)]
pub enum ExportError {
    /// No such shot, a shot whose drawings are not loaded, settings that make no picture.
    Invalid(String),
    /// The sequence's folder has frames already, and nothing said what to do about it.
    Exists(PathBuf),
    /// A file could not be written.
    Io { path: PathBuf, why: String },
    /// The render was stopped before its end. The frames written so far are left.
    Stopped,
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Invalid(why) => write!(f, "{why}"),
            ExportError::Exists(folder) => write!(f, "{} has rendered frames in it already", folder.display()),
            ExportError::Io { path, why } => write!(f, "{} could not be written: {why}", path.display()),
            ExportError::Stopped => write!(f, "the render was stopped"),
        }
    }
}

impl std::error::Error for ExportError {}

/// What a render wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The folder the sequence is in.
    pub folder: PathBuf,
    /// The frames of the shot that were rendered: the first, and one past the last.
    pub frames: (u32, u32),
    /// The files, in order. Empty for a sequence that was there already and left.
    pub files: Vec<PathBuf>,
    /// What was left out of the render, and why: a sound whose file is gone, say.
    pub notes: Vec<String>,
}

/// The frames of `shot` that `range` asks for: the first, and one past the last.
pub fn frames_of(project: &Project, shot: ShotId, range: Range) -> Option<(u32, u32)> {
    let duration = project.shot(shot)?.duration.max(1);
    let cut = project.storyboard_of(shot).and_then(|board| board.clip(shot)).map(|clip| (clip.in_point, clip.out_point));
    Some(match (range, cut) {
        (Range::Cut { handles }, Some((start, end))) => (start.saturating_sub(handles), end.saturating_add(handles).min(duration).max(start + 1)),
        _ => (0, duration),
    })
}

/// What is to be said when a render of one work of a shot has nothing in it, because all that
/// is drawn in the shot is of the other: boards are not rendered (plan §7.2c).
pub(crate) fn nothing_of(shot: &iskra_model::Shot, work: Work) -> Option<String> {
    let drawn = |work| {
        let seen = shot.only(work).stack();
        seen.into_iter().filter_map(|id| shot.layer(id)).any(|layer| layer.drawings.iter().any(|drawing| !drawing.drawing.bounds().is_empty()))
    };
    let (lacks, has, other) = match work {
        Work::Animation => ("animation", "only boards", Work::Boards),
        Work::Boards => ("boards", "only animation", Work::Animation),
        Work::Whole => return None,
    };
    (!drawn(work) && drawn(other))
        .then(|| format!("`{}` has no {lacks} drawn, {has}: its picture is empty. Which of the two is rendered is in the render settings", shot.name))
}

/// A name as part of a file's: what a file system may refuse becomes `_`.
fn file_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    if safe.trim().is_empty() { "Shot".to_string() } else { safe.trim().to_string() }
}

/// The folder a shot's sequence goes into, in `dest`: one of its own, named for the shot
/// (plan §7.14), or for a later version `Shot_01_v002`.
pub fn sequence_folder(dest: &Path, shot_name: &str, version: u32) -> PathBuf {
    let name = file_name(shot_name);
    dest.join(if version > 1 { format!("{name}_v{version:03}") } else { name })
}

/// The file of one frame of a sequence in `folder`: the folder's name, the frame's number in
/// the shot counted from 1, and the format's ending. `Shot_01/Shot_01.0001.png`.
pub fn frame_file(folder: &Path, frame: u32, format: Format) -> PathBuf {
    let name = folder.file_name().map_or("Shot".to_string(), |name| name.to_string_lossy().into_owned());
    frame_named(folder, &name, frame, format)
}

/// The file of one frame of the sequence whose files are named `base`, in `folder`.
fn frame_named(folder: &Path, base: &str, frame: u32, format: Format) -> PathBuf {
    folder.join(format!("{base}.{:04}.{}", frame + 1, format.extension()))
}

/// Whether `folder` has frames in it already of the sequence whose files are named `base`.
fn has_frames(folder: &Path, base: &str) -> bool {
    let name = format!("{base}.");
    std::fs::read_dir(folder).is_ok_and(|mut entries| entries.any(|entry| entry.is_ok_and(|entry| entry.file_name().to_string_lossy().starts_with(&name))))
}

/// An sRGB value, 0 to 1, in linear light.
pub(crate) fn to_linear(value: f32) -> f32 {
    if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

/// A value in linear light, 0 to 1, as sRGB.
fn to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 { value * 12.92 } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 }
}

/// How good a JPEG is written, of 100.
const JPEG_QUALITY: u8 = 95;

/// A composited picture (colour times alpha, in `space`) as the file is to hold it.
enum Pixels {
    /// Straight alpha, sRGB.
    Eight(Vec<u8>),
    Sixteen(Vec<u16>),
    /// Linear light, colour times alpha: what an EXR file holds.
    Float(Vec<f32>),
}

/// A composited picture on the matte, where the settings want no transparency: every pixel
/// is then opaque. With transparency it is as it was.
pub fn on_matte(composited: &[[f32; 4]], space: WorkingSpace, settings: &Settings) -> Vec<[f32; 4]> {
    if settings.alpha {
        return composited.to_vec();
    }
    let matte = settings.matte.map(|value| {
        let value = f32::from(value) / 255.0;
        if space == WorkingSpace::Linear { to_linear(value) } else { value }
    });
    composited.iter().map(|&[r, g, b, a]| [r + matte[0] * (1.0 - a), g + matte[1] * (1.0 - a), b + matte[2] * (1.0 - a), 1.0]).collect()
}

fn pixels(composited: &[[f32; 4]], space: WorkingSpace, settings: &Settings) -> Pixels {
    let matte = settings.matte.map(|value| {
        let value = f32::from(value) / 255.0;
        if space == WorkingSpace::Linear { to_linear(value) } else { value }
    });
    // Each pixel, on the matte if there is to be no transparency, with what is not a number
    // taken as nothing.
    let pixel = |[r, g, b, a]: [f32; 4]| {
        let a = if a.is_nan() { 0.0 } else { a.clamp(0.0, 1.0) };
        let clean = |value: f32| if value.is_nan() { 0.0 } else { value.max(0.0) };
        let (r, g, b) = (clean(r), clean(g), clean(b));
        if settings.alpha { [r, g, b, a] } else { [r + matte[0] * (1.0 - a), g + matte[1] * (1.0 - a), b + matte[2] * (1.0 - a), 1.0] }
    };
    let channels = if settings.alpha { 4 } else { 3 };
    if settings.bits == 32 {
        let linear = |value: f32| if space == WorkingSpace::Linear { value } else { to_linear(value.min(1.0)) };
        let values = composited.iter().flat_map(|&composited| {
            let [r, g, b, a] = pixel(composited);
            // Colour times alpha stays so: in linear light, a straight colour is taken first.
            let straight = |value: f32| if a > 0.0 { linear(value / a) * a } else { 0.0 };
            [straight(r), straight(g), straight(b), a].into_iter().take(channels)
        });
        return Pixels::Float(values.collect());
    }
    // Straight alpha, as sRGB, from 0 to 1.
    let shown = |composited: [f32; 4]| {
        let [r, g, b, a] = pixel(composited);
        let straight = |value: f32| {
            let straight = if a > 0.0 { (value / a).clamp(0.0, 1.0) } else { 0.0 };
            if space == WorkingSpace::Linear { to_srgb(straight) } else { straight }
        };
        [straight(r), straight(g), straight(b), a]
    };
    if settings.bits == 16 {
        Pixels::Sixteen(
            composited.iter().flat_map(|&composited| shown(composited).map(|value| (value * 65535.0 + 0.5) as u16).into_iter().take(channels)).collect(),
        )
    } else {
        let eight = |composited: [f32; 4]| {
            let shown = shown(composited).map(|value| (value * 255.0 + 0.5) as u8);
            // So little that it is none at 8 bits is nothing, and has no colour.
            if shown[3] == 0 { [0; 4] } else { shown }
        };
        Pixels::Eight(composited.iter().flat_map(|&composited| eight(composited).into_iter().take(channels)).collect())
    }
}

fn write(path: &Path, width: u32, height: u32, pixels: Pixels, settings: &Settings) -> Result<(), String> {
    use image::{DynamicImage, ImageBuffer, ImageFormat};
    let format = match settings.format {
        Format::Png => ImageFormat::Png,
        Format::Jpeg => ImageFormat::Jpeg,
        Format::Tiff => ImageFormat::Tiff,
        Format::Exr => ImageFormat::OpenExr,
        Format::Mov | Format::Mp4 => return Err("a movie is not written a picture at a time".to_string()),
    };
    let wrong = || "the picture is not of the size it was made at".to_string();
    let picture = match (pixels, settings.alpha) {
        (Pixels::Eight(values), true) => DynamicImage::ImageRgba8(ImageBuffer::from_raw(width, height, values).ok_or_else(wrong)?),
        (Pixels::Eight(values), false) => DynamicImage::ImageRgb8(ImageBuffer::from_raw(width, height, values).ok_or_else(wrong)?),
        (Pixels::Sixteen(values), true) => DynamicImage::ImageRgba16(ImageBuffer::from_raw(width, height, values).ok_or_else(wrong)?),
        (Pixels::Sixteen(values), false) => DynamicImage::ImageRgb16(ImageBuffer::from_raw(width, height, values).ok_or_else(wrong)?),
        (Pixels::Float(values), true) => DynamicImage::ImageRgba32F(ImageBuffer::from_raw(width, height, values).ok_or_else(wrong)?),
        (Pixels::Float(values), false) => DynamicImage::ImageRgb32F(ImageBuffer::from_raw(width, height, values).ok_or_else(wrong)?),
    };
    if settings.format == Format::Jpeg {
        // Near the best a JPEG gives: a render is kept, not sent over a slow line.
        let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(file), JPEG_QUALITY);
        return picture.write_with_encoder(encoder).map_err(|e| e.to_string());
    }
    picture.save_with_format(path, format).map_err(|e| e.to_string())
}

/// Render `shot` of `project` to an image sequence in a folder of its own in `dest`
/// (plan §7.14). The shot's drawings have to be loaded.
///
/// `progress` is told, after each frame, how many are done of how many; while it returns
/// false, no further frame is begun and the render ends as [`ExportError::Stopped`]. It is
/// called from the threads that render, several at once.
pub fn export_sequence(
    project: &Project,
    shot: ShotId,
    settings: &Settings,
    dest: &Path,
    if_exists: IfExists,
    progress: &(dyn Fn(usize, usize) -> bool + Sync),
) -> Result<Report, ExportError> {
    export_sequence_picked(project, shot, settings, None, &Naming::default(), dest, if_exists, None, progress)
}

/// [`export_sequence`] of some of the shot's passes: only what `pick` names is in the picture,
/// and a pass that is rendered as an output of its own goes a folder down, under its name.
/// `naming` says what it is called, if not as the shot is, and the folder under `dest`. With
/// `sound_beside` in the settings, the sound under its frames is a WAV file beside them, its
/// sounds read from their files as `folder`, the project's own, places them.
#[allow(clippy::too_many_arguments)]
pub fn export_sequence_picked(
    project: &Project,
    shot: ShotId,
    settings: &Settings,
    pick: Option<&Pick>,
    naming: &Naming,
    dest: &Path,
    if_exists: IfExists,
    sounds: Option<&Path>,
    progress: &(dyn Fn(usize, usize) -> bool + Sync),
) -> Result<Report, ExportError> {
    let settings = settings.fitted();
    if settings.format.is_movie() {
        return Err(ExportError::Invalid("a movie is one file, not a sequence".to_string()));
    }
    let Some(of) = project.shot(shot) else { return Err(ExportError::Invalid(format!("no shot {}", shot.0))) };
    if !of.loaded {
        return Err(ExportError::Invalid(format!("the drawings of `{}` are not loaded: load the shot first", of.name)));
    }
    let canvas = Canvas::of(project, &settings, false)?;
    let dest = &naming.dest(dest);
    let frames = frames_of(project, shot, settings.range).ok_or_else(|| ExportError::Invalid(format!("no shot {}", shot.0)))?;

    // Only what was picked of the shot is seen; all of it, if nothing was.
    let notes: Vec<String> = nothing_of(of, settings.work).into_iter().collect();
    let part = of.only(settings.work);
    let picked = pick.and_then(|pick| pick.of(&part));
    let of_name = naming.named(&of.name);
    let of = picked.as_ref().unwrap_or(&part);
    // Where it goes: the shot's folder, or beside what is there as the next version; a pass
    // of its own a folder down, under its name.
    let pass = pick.and_then(|pick| pick.named.as_deref()).map(file_name);
    let place = |version: u32| {
        let folder = sequence_folder(dest, &of_name, version);
        let shot = folder.file_name().map_or(String::new(), |name| name.to_string_lossy().into_owned());
        match &pass {
            Some(pass) => (folder.join(pass), format!("{shot}_{pass}")),
            None => (folder, shot),
        }
    };
    let (mut folder, mut base) = place(1);
    if has_frames(&folder, &base) {
        match if_exists {
            IfExists::Fail => return Err(ExportError::Exists(folder)),
            IfExists::Skip => return Ok(Report { folder, frames, files: Vec::new(), notes: Vec::new() }),
            IfExists::Replace => {}
            IfExists::Version => {
                (folder, base) = (2..).map(place).find(|(folder, base)| !has_frames(folder, base)).unwrap_or((folder, base));
            }
        }
    }
    std::fs::create_dir_all(&folder).map_err(|e| ExportError::Io { path: folder.clone(), why: e.to_string() })?;

    // At another rate, which of the shot's frames each of the render's shows.
    let rate = rate_of(project, &settings);
    let shown = resampled(project, frames.1 - frames.0, rate);
    let (space, total) = (project.settings.working_space, shown.len());
    let (done, stopped) = (AtomicUsize::new(0), AtomicBool::new(false));
    let written: Vec<Result<PathBuf, ExportError>> = shown
        .par_iter()
        .enumerate()
        .map(|(at, offset)| {
            if stopped.load(Ordering::Relaxed) {
                return Err(ExportError::Stopped);
            }
            let frame = frames.0 + offset;
            let path = naming.frame_path(&folder, &base, naming.number_at(project, shot, frames.0, at as u32, rate), settings.format);
            let mut composited = canvas.composite(of, &project.palette, space, frame);
            canvas.burn(&mut composited, project, &settings, Some(&of.name), i64::from(frame));
            write(&path, canvas.width, canvas.height, pixels(&composited, space, &settings), &settings)
                .map_err(|why| ExportError::Io { path: path.clone(), why })?;
            if !progress(done.fetch_add(1, Ordering::Relaxed) + 1, total) {
                stopped.store(true, Ordering::Relaxed);
            }
            Ok(path)
        })
        .collect();
    // A file that could not be written is what went wrong, before a stop.
    let mut files = Vec::with_capacity(total);
    let mut failed = None;
    for file in written {
        match file {
            Ok(path) => files.push(path),
            Err(ExportError::Stopped) => failed = failed.or(Some(ExportError::Stopped)),
            Err(problem) => failed = Some(problem),
        }
    }
    if let Some(problem) = failed {
        return Err(problem);
    }
    // The sound under the frames, beside them, named as they are.
    let mut notes = notes;
    if settings.sound_beside
        && let Some(wav) = movie::sound_file(project, shot, frames, &settings, sounds, &mut notes)
    {
        let path = folder.join(format!("{base}.wav"));
        std::fs::write(&path, wav).map_err(|e| ExportError::Io { path: path.clone(), why: e.to_string() })?;
    }
    Ok(Report { folder, frames, files, notes })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_drawing::Rect;
    use iskra_model::{ArtContent, Clip, DrawingId, PoolDrawing, SwatchId};

    use super::*;

    /// A project of 32 by 18 with one shot of 6 frames: a square of the palette's red, 8
    /// pixels a side at the drawings' two pixels a unit, in the middle of frames 0 to 4. Its
    /// clip is frames 2 to 5 of it.
    fn project() -> (Project, ShotId) {
        let mut project = Project::default();
        (project.settings.width, project.settings.height) = (32, 18);
        let id = DrawingId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.duration = 6;
        let layer = Arc::make_mut(&mut shot.layers[0]);
        let mut drawing = iskra_model::Drawing::default();
        if let ArtContent::Indexed(line) = &mut drawing.art_layers[1].content {
            line.paint(Rect::new(-8, -8, 8, 8), &[1.0; 256], SwatchId(3));
        }
        layer.drawings.push(PoolDrawing { id, name: "1".to_string(), drawing: Arc::new(drawing) });
        layer.exposure.set(0, 5, Some(id));
        let shot = shot.id;
        let board = Arc::make_mut(&mut project.storyboards[0]);
        board.put(Clip { shot, start: 0, in_point: 2, out_point: 5 }, false).unwrap();
        (project, shot)
    }

    const ALWAYS: &(dyn Fn(usize, usize) -> bool + Sync) = &|_, _| true;

    #[test]
    fn a_render_has_burned_into_it_what_is_asked_for() {
        let (project, shot) = project();
        let dir = tempfile::tempdir().unwrap();
        let burn_in = BurnIn { name: true, counter: true, opacity: 1.0, size: 0.1, ..BurnIn::default() };
        let settings = Settings { size: Some([320, 180]), burn_in, ..Settings::default() };
        let report = export_sequence(&project, shot, &settings, dir.path(), IfExists::Fail, ALWAYS).unwrap();
        let pictures: Vec<image::RgbaImage> = report.files.iter().map(|file| image::open(file).unwrap().into_rgba8()).collect();
        let region = |picture: &image::RgbaImage, x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| picture.get_pixel(x, y).0).collect::<Vec<_>>()
        };
        // The name on its box in the top left, the counter in the bottom right, over what is
        // transparent; the red square in the middle as it was.
        let (name, counter) = (region(&pictures[0], 0, 160, 0, 40), region(&pictures[0], 160, 320, 140, 180));
        for corner in [&name, &counter] {
            assert!(corner.contains(&[0, 0, 0, 255]) && corner.iter().any(|pixel| pixel[0] > 230 && pixel[3] == 255));
        }
        assert_eq!(pictures[0].get_pixel(160, 90).0, [220, 50, 47, 255]);
        // The counter counts; the name stays.
        assert_ne!(region(&pictures[1], 160, 320, 140, 180), counter);
        assert_eq!(region(&pictures[1], 0, 160, 0, 40), name);
        // Without, nothing is there.
        let plain =
            export_sequence(&project, shot, &Settings { size: Some([320, 180]), ..Settings::default() }, dir.path(), IfExists::Version, ALWAYS).unwrap();
        let plain = image::open(&plain.files[0]).unwrap().into_rgba8();
        assert!(region(&plain, 0, 160, 0, 40).iter().all(|pixel| pixel[3] == 0));
    }

    #[test]
    fn a_shot_is_rendered_to_a_folder_of_its_own_a_file_a_frame() {
        let (project, shot) = project();
        let dir = tempfile::tempdir().unwrap();
        // The cut, frames 2 to 4, numbered as the shot's frames are, from 1.
        let report = export_sequence(&project, shot, &Settings::default(), dir.path(), IfExists::Fail, ALWAYS).unwrap();
        let folder = dir.path().join("Shot_01");
        let names: Vec<String> = report.files.iter().map(|file| file.strip_prefix(&folder).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(
            (report.folder.clone(), report.frames, names),
            (folder.clone(), (2, 5), vec!["Shot_01.0003.png".to_string(), "Shot_01.0004.png".to_string(), "Shot_01.0005.png".to_string()])
        );
        let picture = image::open(&report.files[0]).unwrap().into_rgba8();
        assert_eq!((picture.width(), picture.height()), (32, 18));
        assert_eq!((picture.get_pixel(16, 9).0, picture.get_pixel(2, 2).0), ([220, 50, 47, 255], [0; 4]), "the red square over transparency");

        // With a frame of handles each side, and all of it; the handles end where the shot does.
        assert_eq!(frames_of(&project, shot, Range::Cut { handles: 1 }), Some((1, 6)));
        assert_eq!(frames_of(&project, shot, Range::Cut { handles: 9 }), Some((0, 6)));
        assert_eq!(frames_of(&project, shot, Range::Whole), Some((0, 6)));
        let mut loose = project.clone();
        Arc::make_mut(&mut loose.storyboards[0]).take(shot);
        assert_eq!(frames_of(&loose, shot, Range::Cut { handles: 0 }), Some((0, 6)), "a shot on no storyboard is rendered whole");

        // What is there already is not written over unless that is said.
        assert!(
            matches!(export_sequence(&project, shot, &Settings::default(), dir.path(), IfExists::Fail, ALWAYS), Err(ExportError::Exists(at)) if at == folder)
        );
        let skipped = export_sequence(&project, shot, &Settings::default(), dir.path(), IfExists::Skip, ALWAYS).unwrap();
        assert!(skipped.files.is_empty() && skipped.folder == folder);
        let next = export_sequence(&project, shot, &Settings::default(), dir.path(), IfExists::Version, ALWAYS).unwrap();
        assert_eq!(next.files[0], dir.path().join("Shot_01_v002").join("Shot_01_v002.0003.png"));
        let third = export_sequence(&project, shot, &Settings::default(), dir.path(), IfExists::Version, ALWAYS).unwrap();
        assert_eq!(third.folder, dir.path().join("Shot_01_v003"));
        let replaced =
            export_sequence(&project, shot, &Settings { range: Range::Whole, ..Settings::default() }, dir.path(), IfExists::Replace, ALWAYS).unwrap();
        assert_eq!((replaced.folder, replaced.files.len()), (folder, 6));
    }

    #[test]
    fn each_format_holds_the_picture_at_its_depths() {
        let (project, shot) = project();
        let dir = tempfile::tempdir().unwrap();
        let one = Range::Cut { handles: 0 };
        let render = |name: &str, settings: Settings| {
            let report = export_sequence(&project, shot, &settings, &dir.path().join(name), IfExists::Fail, ALWAYS).unwrap();
            image::open(&report.files[0]).unwrap()
        };
        // 16 bits: the palette's red as it is at 8, with more behind it.
        let deep = render("png16", Settings { bits: 16, range: one, ..Settings::default() }).into_rgba16();
        assert_eq!(deep.get_pixel(16, 9).0, [220 * 257, 50 * 257, 47 * 257, 65535]);
        let tiff = render("tif", Settings { format: Format::Tiff, bits: 16, range: one, ..Settings::default() });
        assert_eq!((tiff.color(), tiff.into_rgba16().get_pixel(2, 2).0), (image::ColorType::Rgba16, [0; 4]));
        // A JPEG has no transparency: the picture is on the paper, at 8 bits whatever was asked.
        let jpeg = render("jpg", Settings { format: Format::Jpeg, bits: 16, range: one, ..Settings::default() });
        assert_eq!(jpeg.color(), image::ColorType::Rgb8);
        let jpeg = jpeg.into_rgb8();
        // A JPEG is not exact, least of all where colours meet: near enough.
        let (paper, red) = (jpeg.get_pixel(2, 2).0, jpeg.get_pixel(16, 9).0);
        assert!(paper.iter().all(|value| *value > 240) && red[0] > 170 && red[1] < 110 && red[2] < 110, "{paper:?} {red:?}");
        // Without transparency, on a matte of the user's.
        let matted = render("matte", Settings { alpha: false, matte: [10, 20, 30], range: one, ..Settings::default() });
        assert_eq!((matted.color(), matted.into_rgb8().get_pixel(2, 2).0), (image::ColorType::Rgb8, [10, 20, 30]));
        // An EXR holds floats in linear light: the red's 220 of 255 is about 0.716 there.
        let exr = render("exr", Settings { format: Format::Exr, range: one, ..Settings::default() }).into_rgba32f();
        let [r, g, _, a] = exr.get_pixel(16, 9).0;
        assert!((r - 0.7157).abs() < 0.002 && (g - 0.0319).abs() < 0.002 && a == 1.0, "{r} {g} {a}");
        assert_eq!(exr.get_pixel(2, 2).0, [0.0; 4]);
        // At half the size, half the pixels.
        let small = render("half", Settings { scale: 0.5, range: one, ..Settings::default() });
        assert_eq!((small.width(), small.height()), (16, 9));

        assert_eq!(Settings { format: Format::Exr, bits: 8, ..Settings::default() }.fitted().bits, 32);
        assert!(!Settings { format: Format::Jpeg, ..Settings::default() }.fitted().alpha);
    }

    #[test]
    fn a_shot_is_rendered_to_a_movie_with_its_sound() {
        use filmcraft_isobmff::TrackKind;
        let (mut project, shot) = project();
        let dir = tempfile::tempdir().unwrap();
        // A second and a half of a steady tone, laid under the film from its first frame.
        let wav = dir.path().join("line.wav");
        std::fs::write(&wav, iskra_audio::write_wav16(&vec![0.25; 12_000], 1, 8000)).unwrap();
        let media = iskra_model::MediaId(project.new_id());
        let kind = iskra_model::MediaKind::Sound { sample_rate: 8000, channels: 1, frames: 12_000 };
        project.media.push(Arc::new(iskra_model::Media {
            id: media,
            name: "line".to_string(),
            bin: None,
            path: PathBuf::from("line.wav"),
            kind,
            transcript: Vec::new(),
        }));
        let clip = iskra_model::SoundClip { id: iskra_model::SoundClipId(project.new_id()), media, start: 0, offset: 0, length: 36, gain: 1.0 };
        Arc::make_mut(&mut project.storyboards[0]).put_sound(clip, None);

        let read = |path: &Path| {
            let bytes = std::fs::read(path).unwrap();
            (filmcraft_isobmff::open(&bytes).unwrap(), bytes)
        };
        // ProRes in a QuickTime file, beside the others' movies: the cut, three frames at 24 a second.
        let mov = export_movie(&project, shot, &Settings { format: Format::Mov, ..Settings::default() }, dir.path(), IfExists::Fail, Some(dir.path()), ALWAYS)
            .unwrap();
        assert_eq!((mov.files.clone(), mov.frames, mov.notes.len()), (vec![dir.path().join("Shot_01.mov")], (2, 5), 0));
        let (file, bytes) = read(&mov.files[0]);
        let picture = file.tracks.iter().position(|track| track.kind == TrackKind::Video).expect("a picture track");
        let sound = file.tracks.iter().find(|track| track.kind == TrackKind::Audio).expect("a sound track");
        assert!(file.is_quicktime && (file.tracks[picture].width, file.tracks[picture].height) == (32, 18));
        assert_eq!((file.tracks[picture].samples.len(), file.tracks[picture].timescale, file.tracks[picture].samples[0].duration), (3, 24, 1));
        // An eighth of a second of sound, at 48000 samples a second.
        assert_eq!((sound.timescale, sound.samples.iter().map(|sample| u64::from(sample.duration)).sum::<u64>()), (48_000, 6000));
        // The first picture, decoded: the red square on the paper, in video's colours.
        let frame = filmcraft_prores::decode_frame(&file.read_sample(&bytes, picture, 0).unwrap()).unwrap();
        let luma = |x: usize, y: usize| frame.y[y * 32 + x];
        assert!(luma(2, 2) > 900 && luma(16, 9) > 200 && luma(16, 9) < 400, "white paper and a red square: {} {}", luma(2, 2), luma(16, 9));
        // The sound is the tone, not silence: 16-bit samples of a quarter of full scale.
        let heard = file.read_sample(&bytes, file.tracks.iter().position(|track| track.kind == TrackKind::Audio).unwrap(), 0).unwrap();
        let first = i16::from_le_bytes([heard[400], heard[401]]);
        assert!((i32::from(first) - 8192).abs() < 40, "{first}");
        // Deeper: 24 bits, three bytes a sample, and floats, four.
        for (bits, folder) in [(24, "deep"), (32, "float")] {
            let deep = Settings { format: Format::Mov, sound_bits: bits, ..Settings::default() };
            let mov = export_movie(&project, shot, &deep, &dir.path().join(folder), IfExists::Fail, Some(dir.path()), ALWAYS).unwrap();
            let (file, bytes) = read(&mov.files[0]);
            let track = file.tracks.iter().position(|track| track.kind == TrackKind::Audio).unwrap();
            let heard = file.read_sample(&bytes, track, 0).unwrap();
            // A moment of both channels is six bytes, or eight; the 100th's left channel.
            let level = match bits {
                24 => (i32::from_le_bytes([0, heard[600], heard[601], heard[602]]) >> 8) as f32 / 8_388_607.0,
                _ => f32::from_le_bytes([heard[800], heard[801], heard[802], heard[803]]),
            };
            assert_eq!(heard.len(), 6000 * usize::from(bits / 8) * 2, "{bits}");
            assert!((level - 0.25).abs() < 0.01, "{bits}: {level}");
        }
        // An image sequence with the sound beside its frames, as a WAV file named as they are.
        let beside = Settings { sound_beside: true, sound_bits: 24, ..Settings::default() };
        let frames =
            export_sequence_picked(&project, shot, &beside, None, &Naming::default(), &dir.path().join("frames"), IfExists::Fail, Some(dir.path()), ALWAYS)
                .unwrap();
        let heard = iskra_audio::read_wav(&std::fs::read(frames.folder.join("Shot_01.wav")).unwrap()).unwrap();
        assert_eq!((heard.info.sample_rate, heard.info.channels, heard.info.frames, frames.files.len()), (48_000, 2, 6000, 3));
        assert!((heard.samples[200] - 0.25).abs() < 0.01, "{}", heard.samples[200]);
        // Without it, none.
        let bare = export_sequence(&project, shot, &Settings::default(), &dir.path().join("bare"), IfExists::Fail, ALWAYS).unwrap();
        assert!(!bare.folder.join("Shot_01.wav").exists());

        // H.264 in an MP4, the whole shot, without its sound.
        let whole = Settings { format: Format::Mp4, range: Range::Whole, sound: false, ..Settings::default() };
        let mp4 = export_movie(&project, shot, &whole, dir.path(), IfExists::Fail, Some(dir.path()), ALWAYS).unwrap();
        let (file, _) = read(&mp4.files[0]);
        assert_eq!((mp4.files[0].clone(), file.is_quicktime, file.tracks.len()), (dir.path().join("Shot_01.mp4"), false, 1));
        assert_eq!((file.tracks[0].samples.len(), file.tracks[0].samples.iter().filter(|sample| sample.is_sync).count() >= 1), (6, true));
        // With it: AAC, which a player starts after the encoder's run-up.
        let with_sound = export_movie(&project, shot, &Settings { sound: true, ..whole }, dir.path(), IfExists::Version, Some(dir.path()), ALWAYS).unwrap();
        let (file, _) = read(&with_sound.files[0]);
        assert_eq!(with_sound.files[0], dir.path().join("Shot_01_v002.mp4"));
        assert!(file.tracks.iter().any(|track| track.kind == TrackKind::Audio && track.timescale == 48_000 && !track.samples.is_empty()));

        // What is there is not written over unless that is said; a stopped render leaves no file.
        assert!(matches!(export_movie(&project, shot, &whole, dir.path(), IfExists::Fail, None, ALWAYS), Err(ExportError::Exists(_))));
        assert!(export_movie(&project, shot, &whole, dir.path(), IfExists::Skip, None, ALWAYS).unwrap().files.is_empty());
        let stopped = export_movie(&project, shot, &whole, &dir.path().join("stopped"), IfExists::Fail, None, &|done, _| done < 2);
        assert!(matches!(stopped, Err(ExportError::Stopped)) && !dir.path().join("stopped").join("Shot_01.mp4").exists());
        // A sound whose file is gone is said, and the movie is made without it.
        std::fs::remove_file(&wav).unwrap();
        let silent = export_movie(
            &project,
            shot,
            &Settings { format: Format::Mov, ..Settings::default() },
            &dir.path().join("silent"),
            IfExists::Fail,
            Some(dir.path()),
            ALWAYS,
        )
        .unwrap();
        assert!(silent.notes.len() == 1 && silent.notes[0].contains("line") && silent.files[0].is_file());
        // The storyboard as one file: the shot's cut, then a gap of black, then the shot again
        // as a second shot would lie, six frames in all, with the sound under all of it.
        let mut film = project.clone();
        let second = iskra_model::ShotId(film.new_id());
        let mut copy = iskra_model::Shot::clone(&film.shots[0]);
        (copy.id, copy.name) = (second, "Shot_02".to_string());
        film.shots.push(Arc::new(copy));
        Arc::make_mut(&mut film.storyboards[0]).put(Clip { shot: second, start: 5, in_point: 0, out_point: 1 }, false).unwrap();
        let board = film.storyboards[0].id;
        let whole_film =
            export_storyboard(&film, board, &Settings { format: Format::Mov, ..Settings::default() }, &dir.path().join("film"), IfExists::Fail, None, ALWAYS)
                .unwrap();
        assert_eq!((whole_film.files.clone(), whole_film.frames), (vec![dir.path().join("film").join("Storyboard.mov")], (0, 6)));
        let (file, bytes) = read(&whole_film.files[0]);
        let picture = file.tracks.iter().position(|track| track.kind == TrackKind::Video).unwrap();
        assert_eq!(file.tracks[picture].samples.len(), 6);
        let luma_of = |sample: usize| filmcraft_prores::decode_frame(&file.read_sample(&bytes, picture, sample).unwrap()).unwrap().y[2 * 32 + 2];
        assert!(
            luma_of(0) > 900 && luma_of(3) < 100 && luma_of(5) > 900,
            "paper, black between the shots, paper: {} {} {}",
            luma_of(0),
            luma_of(3),
            luma_of(5)
        );
        assert!(export_storyboard(&film, iskra_model::StoryboardId(999), &whole, dir.path(), IfExists::Fail, None, ALWAYS).is_err());
        // A fade on the first shot's end is in the film: its last two frames go towards black.
        let fade = iskra_model::Transition { shot, edge: iskra_model::Edge::End, kind: iskra_model::TransitionKind::CrossDissolve, length: 2 };
        Arc::make_mut(&mut film.storyboards[0]).transitions.push(fade);
        let faded =
            export_storyboard(&film, board, &Settings { format: Format::Mov, ..Settings::default() }, &dir.path().join("faded"), IfExists::Fail, None, ALWAYS)
                .unwrap();
        let (file, bytes) = read(&faded.files[0]);
        let luma_of = |sample: usize| filmcraft_prores::decode_frame(&file.read_sample(&bytes, picture, sample).unwrap()).unwrap().y[2 * 32 + 2];
        let (full, most, little) = (luma_of(0), luma_of(1), luma_of(2));
        assert!(
            full > 900 && most < full - 100 && most > 600 && little < 400 && little > 100,
            "paper, three quarters of it, a quarter: {full} {most} {little}"
        );

        // A sequence is not a movie, nor a movie a sequence.
        assert!(export_sequence(&project, shot, &whole, dir.path(), IfExists::Replace, ALWAYS).is_err());
        assert!(export_movie(&project, shot, &Settings::default(), dir.path(), IfExists::Replace, None, ALWAYS).is_err());
    }

    #[test]
    fn a_render_is_of_the_kind_the_size_and_the_name_that_are_asked_for() {
        use filmcraft_isobmff::TrackKind;
        let (mut project, shot) = project();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("line.wav"), iskra_audio::write_wav16(&vec![0.25; 12_000], 1, 8000)).unwrap();
        let media = iskra_model::MediaId(project.new_id());
        let kind = iskra_model::MediaKind::Sound { sample_rate: 8000, channels: 1, frames: 12_000 };
        project.media.push(Arc::new(iskra_model::Media {
            id: media,
            name: "line".to_string(),
            bin: None,
            path: PathBuf::from("line.wav"),
            kind,
            transcript: Vec::new(),
        }));
        let clip = iskra_model::SoundClip { id: iskra_model::SoundClipId(project.new_id()), media, start: 0, offset: 0, length: 36, gain: 1.0 };
        Arc::make_mut(&mut project.storyboards[0]).put_sound(clip, None);
        let read = |path: &Path| {
            let bytes = std::fs::read(path).unwrap();
            (filmcraft_isobmff::open(&bytes).unwrap(), bytes)
        };
        let named = |name: &str| Naming { name: Some(name.to_string()), ..Naming::default() };
        let mov = Settings { format: Format::Mov, alpha: false, sound: false, ..Settings::default() };

        // ProRes 4444 holds transparency: clear around the square. The name, what comes
        // after it and the folder are as asked, and the folder does not lead out.
        let clear = Settings { prores: ProRes::P4444, alpha: true, ..mov };
        assert!(clear.can_alpha() && !mov.can_alpha() && !Settings { alpha: true, ..mov }.fitted().alpha);
        let naming = Naming { name: Some("Hero".to_string()), suffix: "_v2".to_string(), subfolder: "dailies/../monday".to_string(), ..Naming::default() };
        let made = export_movie_picked(&project, shot, &clear, None, &naming, dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(made.files, [dir.path().join("dailies").join("monday").join("Hero_v2.mov")]);
        let (file, bytes) = read(&made.files[0]);
        let frame = filmcraft_prores::decode_frame(&file.read_sample(&bytes, 0, 0).unwrap()).unwrap();
        let (alpha, full) = (frame.alpha.as_ref().expect("an alpha plane"), (1u32 << frame.bit_depth) - 1);
        assert_eq!((u32::from(alpha[2 * 32 + 2]), u32::from(alpha[9 * 32 + 16]), frame.chroma), (0, full, filmcraft_prores::ChromaFormat::Yuv444));
        // Every kind of ProRes is written, the better the larger.
        let sizes: Vec<u64> = ProRes::ALL
            .iter()
            .map(|kind| {
                let made = export_movie_picked(
                    &project,
                    shot,
                    &Settings { prores: *kind, ..mov },
                    None,
                    &named(kind.label()),
                    dir.path(),
                    IfExists::Fail,
                    None,
                    ALWAYS,
                );
                let path = made.unwrap().files[0].clone();
                let (file, bytes) = read(&path);
                assert!(filmcraft_prores::decode_frame(&file.read_sample(&bytes, 0, 0).unwrap()).is_ok(), "{}", kind.label());
                std::fs::metadata(&path).unwrap().len()
            })
            .collect();
        assert!(sizes[0] <= sizes[3] && sizes[3] <= sizes[5], "{sizes:?}");

        // H.264 in a QuickTime file, with AAC at the rate that is asked for; and uncompressed
        // sound at 96000 samples a second.
        let small =
            Settings { codec: Codec::H264, quality: Quality::Low, sound: true, sound_codec: SoundCodec::Aac, sound_rate: 44_100, sound_kbps: 128, ..mov };
        let made = export_movie_picked(&project, shot, &small, None, &named("small"), dir.path(), IfExists::Fail, Some(dir.path()), ALWAYS).unwrap();
        let (file, _) = read(&made.files[0]);
        assert!(file.is_quicktime && file.tracks.iter().any(|track| track.kind == TrackKind::Audio && track.timescale == 44_100 && !track.samples.is_empty()));
        let best = Settings { quality: Quality::Best, sound: false, ..small };
        let larger = export_movie_picked(&project, shot, &best, None, &named("best"), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        // Held to a data rate, the encoder is told that rate and not a quality.
        let held = Settings { video_kbps: Some(1), ..best };
        assert_eq!(held.fitted().video_kbps, Some(100));
        // The test's picture is too small and plain for a rate to show in the file's size:
        // that the movie is written is what is seen here, and the rate the encoder is given
        // is tested where it is chosen.
        let made = export_movie_picked(&project, shot, &held, None, &named("held"), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert!(read(&made.files[0]).0.tracks[0].samples.len() == 3);
        let silent =
            export_movie_picked(&project, shot, &Settings { sound: false, ..small }, None, &named("low"), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert!(std::fs::metadata(&larger.files[0]).unwrap().len() >= std::fs::metadata(&silent.files[0]).unwrap().len());
        let fine = Settings { sound: true, sound_rate: 96_000, ..mov };
        let made = export_movie_picked(&project, shot, &fine, None, &named("fine"), dir.path(), IfExists::Fail, Some(dir.path()), ALWAYS).unwrap();
        let (file, _) = read(&made.files[0]);
        let sound = file.tracks.iter().find(|track| track.kind == TrackKind::Audio).expect("a sound track");
        assert_eq!((sound.timescale, sound.samples.iter().map(|sample| u64::from(sample.duration)).sum::<u64>()), (96_000, 12_000));
        // An MP4 is H.264 with AAC whatever is asked, and a rate that is none is 48000.
        let fitted = Settings { format: Format::Mp4, sound_rate: 12_345, sound_kbps: 9, ..mov }.fitted();
        assert_eq!((fitted.codec, fitted.sound_codec, fitted.sound_rate, fitted.sound_kbps), (Codec::H264, SoundCodec::Aac, 48_000, 64));

        // A size of another shape has the shot whole in its middle: nothing above and below
        // it, or the matte. A movie of that size too.
        let square = Settings { size: Some([64, 64]), range: Range::Whole, ..Settings::default() };
        let made = export_sequence_picked(&project, shot, &square, None, &named("square"), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(made.files[2], dir.path().join("square").join("square.0003.png"));
        let picture = image::open(&made.files[2]).unwrap().into_rgba8();
        assert_eq!((picture.dimensions(), picture.get_pixel(32, 32).0, picture.get_pixel(32, 5).0), ((64, 64), [220, 50, 47, 255], [0; 4]));
        let matted = Settings { alpha: false, matte: [0, 0, 255], ..square };
        let made = export_sequence_picked(&project, shot, &matted, None, &named("matted"), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(image::open(&made.files[2]).unwrap().into_rgba8().get_pixel(32, 5).0, [0, 0, 255, 255]);
        let made =
            export_movie_picked(&project, shot, &Settings { size: Some([65, 41]), ..mov }, None, &named("odd"), dir.path(), IfExists::Fail, None, ALWAYS);
        let (file, _) = read(&made.unwrap().files[0]);
        assert_eq!((file.tracks[0].width, file.tracks[0].height), (64, 40));
        let none = export_sequence_picked(
            &project,
            shot,
            &Settings { size: Some([1 << 20, 1 << 20]), ..square },
            None,
            &named("huge"),
            dir.path(),
            IfExists::Fail,
            None,
            ALWAYS,
        );
        assert!(matches!(none, Err(ExportError::Invalid(why)) if why.contains("1048576 by 1048576")));
        // The storyboard's film takes a name too.
        let film = export_storyboard_named(&project, project.storyboards[0].id, &mov, None, &named("Film"), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(film.files, [dir.path().join("Film.mov")]);
    }

    #[test]
    fn a_render_shows_the_pass_that_is_picked_and_a_pass_of_its_own_lies_a_folder_down() {
        let (mut project, shot) = project();
        // A second layer under the first, which the Background pass shows: a field of the
        // palette's blue over all of the frame. The square that is there is in no pass.
        let mut back = project.new_layer("Field", iskra_model::Role::Animation);
        let id = DrawingId(project.new_id());
        let mut drawing = iskra_model::Drawing::default();
        if let ArtContent::Indexed(line) = &mut drawing.art_layers[1].content {
            line.paint(Rect::new(-40, -40, 40, 40), &[1.0; 6400], SwatchId(5));
        }
        back.drawings.push(PoolDrawing { id, name: "1".to_string(), drawing: Arc::new(drawing) });
        back.exposure.set(0, 6, Some(id));
        let (back_id, pass_id) = (back.id, iskra_model::DisplayId(project.new_id()));
        let of = Arc::make_mut(&mut project.shots[0]);
        of.layers.push(Arc::new(back));
        // At the back of the shot, and the Background pass shows it alone.
        let main = of.main_composite().expect("a shot stacks its layers");
        if let Some(stack) = of.composites.iter_mut().find(|stack| stack.id == main) {
            Arc::make_mut(stack).inputs.push(iskra_model::Source::Layer(back_id));
        }
        let shows = Some(iskra_model::Source::Layer(back_id));
        of.displays.push(iskra_model::Display { id: pass_id, name: "Background".to_string(), shows, pass: true });
        let dir = tempfile::tempdir().unwrap();
        let (red, blue) = ([220, 50, 47, 255], [38, 110, 210, 255]);
        let look = |files: &[PathBuf]| image::open(&files[0]).unwrap().into_rgba8();

        // All of it: the square over the field. Without a pick it is the same.
        let whole = export_sequence(&project, shot, &Settings::default(), &dir.path().join("whole"), IfExists::Fail, ALWAYS).unwrap();
        assert_eq!((look(&whole.files).get_pixel(16, 9).0, look(&whole.files).get_pixel(4, 9).0), (red, blue));
        // The Background pass alone, where the shot's whole picture would be.
        let only = Pick { pass: Some("Background".to_string()), named: None };
        let back = export_sequence_picked(
            &project,
            shot,
            &Settings::default(),
            Some(&only),
            &Naming::default(),
            &dir.path().join("only"),
            IfExists::Fail,
            None,
            ALWAYS,
        )
        .unwrap();
        assert_eq!((back.files[0].clone(), look(&back.files).get_pixel(16, 9).0), (dir.path().join("only/Shot_01/Shot_01.0003.png"), blue));
        // A pass of its own, a folder down under its name; the whole picture as its own too.
        let pass = Pick { named: Some("Background".to_string()), ..only.clone() };
        let named =
            export_sequence_picked(&project, shot, &Settings::default(), Some(&pass), &Naming::default(), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(named.files[0], dir.path().join("Shot_01/Background/Shot_01_Background.0003.png"));
        let whole = Pick { pass: None, named: Some("Final".to_string()) };
        let final_ =
            export_sequence_picked(&project, shot, &Settings::default(), Some(&whole), &Naming::default(), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(
            (final_.folder.clone(), look(&final_.files).get_pixel(16, 9).0, look(&final_.files).get_pixel(4, 9).0),
            (dir.path().join("Shot_01/Final"), red, blue)
        );
        // A pass the shot has not got is its whole picture.
        let none = Pick { pass: Some("Character".to_string()), named: None };
        let lacking = export_sequence_picked(
            &project,
            shot,
            &Settings::default(),
            Some(&none),
            &Naming::default(),
            &dir.path().join("none"),
            IfExists::Fail,
            None,
            ALWAYS,
        )
        .unwrap();
        assert_eq!(look(&lacking.files).get_pixel(16, 9).0, red);
        // The passes are asked about apart: one being there does not stop the other.
        assert!(matches!(
            export_sequence_picked(&project, shot, &Settings::default(), Some(&pass), &Naming::default(), dir.path(), IfExists::Fail, None, ALWAYS),
            Err(ExportError::Exists(_))
        ));
        let again =
            export_sequence_picked(&project, shot, &Settings::default(), Some(&pass), &Naming::default(), dir.path(), IfExists::Version, None, ALWAYS).unwrap();
        assert_eq!(again.files[0], dir.path().join("Shot_01_v002/Background/Shot_01_v002_Background.0003.png"));

        // Movies of passes lie together under the pass's name.
        let movie = Settings { format: Format::Mp4, sound: false, ..Settings::default() };
        let made = export_movie_picked(&project, shot, &movie, Some(&pass), &Naming::default(), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(made.files, [dir.path().join("Background").join("Shot_01_Background.mp4")]);
        assert!(
            export_movie_picked(&project, shot, &movie, Some(&only), &Naming::default(), dir.path(), IfExists::Fail, None, ALWAYS).unwrap().files[0]
                .ends_with("Shot_01.mp4")
        );
        // The storyboard as one movie of a pass lies with the shots' movies of it.
        let board = &project.storyboards[0];
        let film = export_storyboard_named(&project, board.id, &movie, Some(&pass), &Naming::default(), dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        assert_eq!(film.files, [dir.path().join("Background").join(format!("{}_Background.mp4", board.name))]);
    }

    #[test]
    fn a_name_is_made_of_parts_and_a_sequences_frames_are_numbered_as_asked() {
        let parts = |picked: [bool; 3]| FileNaming { from: NameFrom::Parts, parts: picked, ..FileNaming::default() };
        let name = |naming: &FileNaming, own: Option<&str>, several: bool| {
            naming.naming("Ball", "Board", own.map(|name| ShotNames { name, ..ShotNames::default() }), several).name
        };
        assert_eq!(name(&parts([true, false, true]), Some("Shot_01"), false).as_deref(), Some("Ball_Shot_01"));
        assert_eq!(name(&parts([true, true, false]), Some("Shot_01"), false).as_deref(), Some("Ball_Board"));
        assert_eq!(name(&parts([true, true, false]), Some("Shot_01"), true).as_deref(), Some("Ball_Board_Shot_01"), "several shots are told apart");
        assert_eq!(name(&parts([false, true, true]), None, false).as_deref(), Some("Board"), "the film has no shot");
        assert_eq!(name(&parts([false, false, false]), Some("Shot_01"), false), None, "nothing picked is the shot's own");
        assert_eq!(name(&FileNaming { from: NameFrom::Storyboard, ..FileNaming::default() }, Some("Shot_01"), true).as_deref(), Some("Board_Shot_01"));
        // The shot's sequence and scene, before its name; a shot in neither is named without.
        let grouped = FileNaming { sequence: true, scene: true, ..parts([false, false, true]) };
        let shot = ShotNames { name: "SH030", sequence: "SQ010", scene: "SC020" };
        assert_eq!(grouped.naming("Ball", "Board", Some(shot), true).name.as_deref(), Some("SQ010_SC020_SH030"));
        assert_eq!(grouped.naming("Ball", "Board", Some(ShotNames { scene: "", ..shot }), true).name.as_deref(), Some("SQ010_SH030"));
        assert_eq!(grouped.naming("Ball", "Board", None, false).name.as_deref(), None, "the film is in no sequence");

        // Six digits, numbered as the film numbers its frames: from the shot's place in the cut.
        let (project, shot) = project();
        let clip = *project.storyboard_of(shot).unwrap().clip(shot).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let naming = FileNaming { digits: 6, film_numbers: true, ..FileNaming::default() }.naming(
            "Ball",
            "Board",
            Some(ShotNames { name: "Shot_01", ..ShotNames::default() }),
            false,
        );
        let made = export_sequence_picked(&project, shot, &Settings::default(), None, &naming, dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        let first = i64::from(clip.start) + 1;
        assert_eq!(made.files[0], dir.path().join("Shot_01").join(format!("Shot_01.{first:06}.png")));
        let plain = export_sequence(&project, shot, &Settings::default(), &dir.path().join("plain"), IfExists::Fail, ALWAYS).unwrap();
        assert_eq!(made.files.len(), plain.files.len());
        assert_eq!(plain.files[0].file_name().unwrap().to_string_lossy(), format!("Shot_01.{:04}.png", clip.in_point + 1));
    }

    #[test]
    fn a_render_at_another_frame_rate_repeats_or_leaves_out_frames() {
        let (project, shot) = project();
        let whole = Settings { range: Range::Whole, ..Settings::default() };
        let at = |num: i64| iskra_model::FrameRate { num, den: 1 };
        // Six frames at 24: at 12 every second one, at 48 each twice, and as long.
        assert_eq!(resampled(&project, 6, at(12)), [0, 2, 4]);
        assert_eq!(resampled(&project, 6, at(48)), [0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5]);
        assert_eq!(resampled(&project, 6, at(30)), [0, 0, 1, 2, 3, 4, 4, 5]);
        assert_eq!(resampled(&project, 6, at(24)), [0, 1, 2, 3, 4, 5]);
        let dir = tempfile::tempdir().unwrap();
        let slow = export_sequence(&project, shot, &Settings { frame_rate: Some(at(12)), ..whole }, &dir.path().join("slow"), IfExists::Fail, ALWAYS).unwrap();
        let names: Vec<String> = slow.files.iter().map(|file| file.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["Shot_01.0001.png", "Shot_01.0002.png", "Shot_01.0003.png"]);
        // A movie at 30 has as many frames as last as long, at 30 a second.
        let mp4 = Settings { format: Format::Mp4, sound: false, frame_rate: Some(at(30)), ..whole };
        let movie = export_movie(&project, shot, &mp4, dir.path(), IfExists::Fail, None, ALWAYS).unwrap();
        let file = filmcraft_isobmff::open(std::fs::read(&movie.files[0]).unwrap()).unwrap();
        assert_eq!((file.tracks[0].samples.len(), file.tracks[0].timescale), (8, 30));
        // A rate of nothing is the project's.
        assert_eq!(Settings { frame_rate: Some(iskra_model::FrameRate { num: 0, den: 1 }), ..whole }.fitted().frame_rate, None);
    }

    #[test]
    fn a_render_can_be_stopped_and_says_what_it_cant_do() {
        let (project, shot) = project();
        let dir = tempfile::tempdir().unwrap();
        let whole = Settings { range: Range::Whole, ..Settings::default() };
        let seen = AtomicUsize::new(0);
        let stop_after_one: &(dyn Fn(usize, usize) -> bool + Sync) = &|done, of| {
            seen.fetch_max(of, Ordering::Relaxed);
            done < 1
        };
        // One frame at a time here, so that the stop comes before the second is begun.
        let alone = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let stopped = alone.install(|| export_sequence(&project, shot, &whole, dir.path(), IfExists::Fail, stop_after_one));
        assert!(matches!(stopped, Err(ExportError::Stopped)));
        assert!(frame_file(&dir.path().join("Shot_01"), 0, Format::Png).is_file(), "the frame that was in hand is finished");
        assert_eq!(seen.load(Ordering::Relaxed), 6, "it is told how many frames there are");
        // A shot whose drawings are in the file only can't be rendered, nor one that isn't there.
        let mut unloaded = project.clone();
        unloaded.shots[0] = Arc::new(unloaded.shots[0].without_pixels());
        assert!(
            matches!(export_sequence(&unloaded, shot, &whole, dir.path(), IfExists::Replace, ALWAYS), Err(ExportError::Invalid(why)) if why.contains("not loaded"))
        );
        assert!(matches!(export_sequence(&project, ShotId(999), &whole, dir.path(), IfExists::Replace, ALWAYS), Err(ExportError::Invalid(_))));
        assert!(matches!(
            export_sequence(&project, shot, &Settings { scale: 0.0, ..whole }, dir.path(), IfExists::Replace, ALWAYS),
            Err(ExportError::Invalid(_))
        ));
        assert_eq!(file_name("Shot: 1/2?"), "Shot_ 1_2_");
    }
}
