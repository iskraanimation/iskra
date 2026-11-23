//! A storyboard laid out on pages, to print (plan §4.7): its panels in the order of the film,
//! each its picture of the boards, its shot and panel number, its length, and what is written
//! of it, in one of four modes. This is the layout alone, as data: where each picture, text
//! and line lands on each page, in points (a 72nd of an inch) from the page's top left. Writing
//! it as a PDF, and the pictures of the pages, is done from this.
//!
//! Texts are measured in the app's own Inter, the font the PDF embeds, so that what is laid
//! out here is what is printed.

use iskra_model::{Clip, FrameRate, Panel, Project, ShotId, Storyboard, TransitionKind, timecode};
use serde::{Deserialize, Serialize};
use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider};

/// Points in a millimetre.
pub const POINTS_PER_MM: f32 = 72.0 / 25.4;

/// A line of text is this many times as tall as its size.
pub const LINE: f32 = 1.25;

/// The room for a line written by hand, in points: about 7 mm.
pub const WRITING_LINE: f32 = 20.0;

/// Between two panels, across and down, in points.
const GAP: f32 = 12.0;

/// The size of the header's and the footer's texts, in points.
const HEAD_SIZE: f32 = 9.0;

/// The faces the pages are set in, as their files are.
pub(crate) const REGULAR: &[u8] = include_bytes!("../../ui/assets/fonts/Inter-Regular.ttf");
pub(crate) const SEMI_BOLD: &[u8] = include_bytes!("../../ui/assets/fonts/Inter-SemiBold.ttf");

/// How the panels are laid on the pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// The classic storyboard page: a grid of panels, the texts under each.
    #[default]
    Sheets,
    /// A panel a row, its picture on the left and its texts beside it: for reading with the
    /// dialogue.
    Script,
    /// One panel a page, large, its texts under it: for a pitch.
    Presentation,
    /// Many small panels a page, with no texts but the shot and panel number: for a wall or a
    /// beat board.
    Thumbnails,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Sheets, Mode::Script, Mode::Presentation, Mode::Thumbnails];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Sheets => "Sheets",
            Mode::Script => "Script",
            Mode::Presentation => "Presentation",
            Mode::Thumbnails => "Thumbnails",
        }
    }
}

/// A size of paper.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Paper {
    A3,
    #[default]
    A4,
    A5,
    Letter,
    Legal,
    Tabloid,
    /// A size of the user's own, in millimetres, upright.
    Own {
        width: f32,
        height: f32,
    },
}

impl Paper {
    /// The sizes that have names.
    pub const NAMED: [Paper; 6] = [Paper::A3, Paper::A4, Paper::A5, Paper::Letter, Paper::Legal, Paper::Tabloid];

    pub fn name(self) -> &'static str {
        match self {
            Paper::A3 => "A3",
            Paper::A4 => "A4",
            Paper::A5 => "A5",
            Paper::Letter => "Letter",
            Paper::Legal => "Legal",
            Paper::Tabloid => "Tabloid",
            Paper::Own { .. } => "Own size",
        }
    }

    /// Its width and height upright, in millimetres.
    pub fn millimetres(self) -> [f32; 2] {
        match self {
            Paper::A3 => [297.0, 420.0],
            Paper::A4 => [210.0, 297.0],
            Paper::A5 => [148.0, 210.0],
            Paper::Letter => [215.9, 279.4],
            Paper::Legal => [215.9, 355.6],
            Paper::Tabloid => [279.4, 431.8],
            Paper::Own { width, height } => [width.max(10.0), height.max(10.0)],
        }
    }
}

/// How long a panel lasts, as it is printed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Length {
    #[default]
    Seconds,
    Frames,
}

/// Which of the storyboard's panels are printed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Range {
    #[default]
    All,
    /// Those of these shots.
    Shots(Vec<ShotId>),
    /// Those in this stretch of the film: its first frame and the one after its last.
    Film { first: u32, after: u32 },
}

/// What is printed, and how: the window's options, kept in its presets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PageOptions {
    pub mode: Mode,
    pub paper: Paper,
    pub landscape: bool,
    /// On each side of the page, in millimetres.
    pub margin: f32,
    /// The grid of panels on a page: of Sheets and Thumbnails; Script has rows only, and
    /// Presentation one panel.
    pub columns: u32,
    pub rows: u32,
    /// Each shot starts a page of its own.
    pub page_per_shot: bool,
    /// Which texts are printed: what happens, what is said, the notes.
    pub action: bool,
    pub dialogue: bool,
    pub notes: bool,
    /// The texts' size, in points.
    pub text_size: f32,
    /// Lines left empty under a panel's texts, to write on by hand.
    pub writing_lines: u32,
    /// The shot's name and the panel's number among the shot's.
    pub numbers: bool,
    pub length: Option<Length>,
    /// Where the panel starts in the film.
    pub timecode: bool,
    /// A frame round each picture.
    pub framed: bool,
    /// A mark on a panel where a transition leads into its shot, or out of it to nothing.
    pub transitions: bool,
    /// The project's and storyboard's names, the date and the version, over each page.
    pub header: bool,
    /// Typed in by the user: "v3", "Animatic, second pass".
    pub version: String,
    /// "Page n of m" at the foot of each page.
    pub footer: bool,
    pub range: Range,
    /// How many pixels an inch the pictures are made at.
    pub dpi: u32,
    pub grey: bool,
}

impl Default for PageOptions {
    fn default() -> Self {
        PageOptions::of(Mode::Sheets)
    }
}

impl PageOptions {
    /// The options a mode starts with.
    pub fn of(mode: Mode) -> Self {
        let sheets = PageOptions {
            mode,
            paper: Paper::A4,
            landscape: false,
            margin: 12.0,
            columns: 2,
            rows: 3,
            page_per_shot: false,
            action: true,
            dialogue: true,
            notes: false,
            text_size: 9.0,
            writing_lines: 0,
            numbers: true,
            length: Some(Length::Seconds),
            timecode: false,
            framed: true,
            transitions: true,
            header: true,
            version: String::new(),
            footer: true,
            range: Range::All,
            dpi: 150,
            grey: false,
        };
        match mode {
            Mode::Sheets => sheets,
            Mode::Script => PageOptions { columns: 1, rows: 4, notes: true, text_size: 10.0, timecode: true, ..sheets },
            Mode::Presentation => PageOptions { landscape: true, columns: 1, rows: 1, text_size: 14.0, ..sheets },
            Mode::Thumbnails => PageOptions { columns: 4, rows: 6, action: false, dialogue: false, length: None, text_size: 7.0, ..sheets },
        }
    }

    /// The page's width and height, in points.
    pub fn page_size(&self) -> [f32; 2] {
        let [width, height] = self.paper.millimetres().map(|mm| mm * POINTS_PER_MM);
        if self.landscape { [height, width] } else { [width, height] }
    }

    /// The grid as the mode has it: columns and rows.
    fn grid(&self) -> (u32, u32) {
        match self.mode {
            Mode::Sheets | Mode::Thumbnails => (self.columns.max(1), self.rows.max(1)),
            Mode::Script => (1, self.rows.max(1)),
            Mode::Presentation => (1, 1),
        }
    }

    /// Which texts are printed, by role: Thumbnails prints none.
    fn texts(&self) -> Vec<Role> {
        let on = [(self.action, Role::Action), (self.dialogue, Role::Dialogue), (self.notes, Role::Notes)];
        on.into_iter().filter(|(on, _)| *on && self.mode != Mode::Thumbnails).map(|(_, role)| role).collect()
    }
}

/// What a text on a page is: the PDF sets each kind in its own weight and colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// The project's and storyboard's names, over the page.
    Title,
    /// The date and the version, over the page on the right.
    Version,
    /// "Page n of m".
    Footer,
    /// The shot's name and the panel's number.
    Number,
    Length,
    Timecode,
    Action,
    Dialogue,
    Notes,
    /// Over the picture's corner: a transition leads into the shot here, or out of it.
    Transition,
}

impl Role {
    /// It is set in Inter SemiBold.
    pub fn bold(self) -> bool {
        matches!(self, Role::Title | Role::Number)
    }
}

/// A place on a page, in points from its top left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Area {
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }
}

/// A panel's picture: of the boards of `shot` on its frame `frame`, made `pixels` large.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Picture {
    pub shot: ShotId,
    pub frame: u32,
    pub area: Area,
    pub pixels: [u32; 2],
    pub framed: bool,
}

/// One line of text, its top left at `at`, `size` points tall.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Text {
    pub role: Role,
    pub text: String,
    pub at: [f32; 2],
    pub size: f32,
}

/// What is on one page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub pictures: Vec<Picture>,
    pub texts: Vec<Text>,
    /// Lines to write on by hand, from one end to the other.
    pub lines: Vec<[[f32; 2]; 2]>,
}

/// The storyboard laid out: the pages' size in points, and the pages.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Laid {
    pub size: [f32; 2],
    pub pages: Vec<Page>,
}

/// The day a time is on, in UTC, as year-month-day. `secs` are seconds since the start of 1970.
pub fn date(secs: u64) -> String {
    // Days to a date of the calendar, by the eras of 400 years that it repeats in.
    let days = (secs / 86_400) as i64 + 719_468;
    let (era, day_of_era) = (days.div_euclid(146_097), days.rem_euclid(146_097));
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    // Months counted from March, so that the leap day is the year's last.
    let month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month + 2) / 5 + 1;
    let month = if month < 10 { month + 3 } else { month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02}")
}

/// Today, as [`date`] says it.
pub fn today() -> String {
    date(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs()))
}

/// Options for the pages under a name of the user's own, to print alike again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintPreset {
    pub name: String,
    pub options: PageOptions,
    /// A PNG of each page beside the PDF.
    #[serde(default)]
    pub pngs: bool,
}

/// What the header says that the project does not: the project's name, as its file has it,
/// and the date, as the app writes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heading {
    pub project: String,
    pub date: String,
}

/// One panel to print: the stretch of it that is in the cut.
#[derive(Clone, Debug, PartialEq)]
struct Board {
    shot: ShotId,
    name: String,
    /// Its place among the shot's panels, and how many the shot has.
    place: usize,
    count: usize,
    /// The frame of the shot it starts on in the cut, and where that is in the film.
    frame: u32,
    film: u32,
    frames: u32,
    panel: Panel,
    transition: Option<String>,
}

/// How a transition is said on a panel.
fn transition_name(kind: TransitionKind) -> &'static str {
    match kind {
        TransitionKind::CrossDissolve => "Dissolve",
        TransitionKind::DipToBlack => "Dip to black",
        TransitionKind::DipToWhite => "Dip to white",
    }
}

/// The panels of `board` to print, in the order of the film, as `range` picks them.
fn boards(project: &Project, board: &Storyboard, range: &Range) -> Vec<Board> {
    let lying = board.lying();
    let mut boards = Vec::new();
    for clip in board.clips() {
        let Some(shot) = project.shot(clip.shot) else { continue };
        if let Range::Shots(picked) = range
            && !picked.contains(&shot.id)
        {
            continue;
        }
        let panels = shot.panels();
        let stretches = stretches_in_cut(&panels, |frame| shot.panel_end(frame), clip);
        let first = stretches.first().map(|(place, ..)| *place);
        let last = stretches.last().map(|(place, ..)| *place);
        // A transition into the shot is marked on its first panel; one out of it to nothing,
        // a fade, on its last.
        let into = lying.iter().find(|lying| lying.incoming.is_some_and(|incoming| incoming.shot == clip.shot));
        let out = lying.iter().find(|lying| lying.incoming.is_none() && lying.outgoing.is_some_and(|outgoing| outgoing.shot == clip.shot));
        for (place, start, end) in stretches {
            let film = clip.start + start - clip.in_point;
            if let Range::Film { first, after } = range
                && (film + (end - start) <= *first || film >= *after)
            {
                continue;
            }
            let said = |lying: &iskra_model::Lying, how: &str| format!("{} {how}, {} fr", transition_name(lying.transition.kind), lying.transition.length);
            let transition = match (into, out) {
                (Some(lying), _) if Some(place) == first => Some(said(lying, "in")),
                (_, Some(lying)) if Some(place) == last => Some(said(lying, "out")),
                _ => None,
            };
            boards.push(Board {
                shot: shot.id,
                name: shot.name.clone(),
                place,
                count: panels.len(),
                frame: start,
                film,
                frames: end - start,
                panel: panels[place].clone(),
                transition,
            });
        }
    }
    boards
}

/// The stretches of a shot's panels that are in the cut of `clip`: for each, its place among
/// the panels, the frame it starts on in the cut, and the frame after its last there.
fn stretches_in_cut(panels: &[Panel], end_of: impl Fn(u32) -> u32, clip: &Clip) -> Vec<(usize, u32, u32)> {
    let stretches = panels.iter().enumerate().filter_map(|(place, panel)| {
        let (start, end) = (panel.frame.max(clip.in_point), end_of(panel.frame).min(clip.out_point));
        (start < end).then_some((place, start, end))
    });
    stretches.collect()
}

/// How wide `text` is set at `size` points, in Inter, SemiBold if `bold`.
pub fn text_width(text: &str, size: f32, bold: bool) -> f32 {
    let Ok(font) = FontRef::new(if bold { SEMI_BOLD } else { REGULAR }) else { return 0.0 };
    let (charmap, advances) = (font.charmap(), font.glyph_metrics(Size::new(size), LocationRef::default()));
    text.chars().map(|c| advances.advance_width(charmap.map(c).unwrap_or_default()).unwrap_or(size * 0.5)).sum()
}

/// How far under the top of its line a text's baseline is, at `size` points.
pub fn ascent(size: f32, bold: bool) -> f32 {
    FontRef::new(if bold { SEMI_BOLD } else { REGULAR }).map_or(size * 0.97, |font| font.metrics(Size::new(size), LocationRef::default()).ascent)
}

/// `text` broken into lines at most `width` wide, at most `most` of them: its own line breaks
/// kept, a word wider than a line broken where it must be. What does not fit ends the last
/// line with an ellipsis; the second is whether anything was cut off.
pub fn wrap(text: &str, width: f32, size: f32, bold: bool, most: usize) -> (Vec<String>, bool) {
    let fits = |line: &str| text_width(line, size, bold) <= width;
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let tried = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if fits(&tried) {
                line = tried;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word too wide for a line alone is broken.
            for c in word.chars() {
                line.push(c);
                if !fits(&line) && line.chars().count() > 1 {
                    line.pop();
                    lines.push(std::mem::replace(&mut line, c.to_string()));
                }
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    if lines.len() <= most {
        return (lines, false);
    }
    lines.truncate(most);
    if let Some(last) = lines.last_mut() {
        while !last.is_empty() && !fits(&format!("{last}…")) {
            last.pop();
        }
        *last = format!("{}…", last.trim_end());
    }
    (lines, true)
}

/// How long a panel lasts, as `length` says it.
fn said_length(frames: u32, rate: FrameRate, length: Length) -> String {
    match length {
        Length::Seconds => format!("{:.1} s", f64::from(frames) / rate.as_f64().max(1.0)),
        Length::Frames => format!("{frames} fr"),
    }
}

/// Lay out the panels of `board` on pages as `options` say.
pub fn lay_out(project: &Project, board: &Storyboard, options: &PageOptions, heading: &Heading) -> Laid {
    let size = options.page_size();
    let margin = (options.margin * POINTS_PER_MM).clamp(0.0, size[0].min(size[1]) / 4.0);
    let head = HEAD_SIZE * LINE + 6.0;
    let top = margin + if options.header { head } else { 0.0 };
    let bottom = size[1] - margin - if options.footer { head } else { 0.0 };
    let content = Area { x: margin, y: top, width: size[0] - 2.0 * margin, height: (bottom - top).max(1.0) };
    let aspect = project.settings.width.max(1) as f32 / project.settings.height.max(1) as f32;
    let rate = project.settings.frame_rate;
    let (columns, rows) = options.grid();
    let cells = (columns * rows) as usize;
    let cell = |at: usize| {
        let (column, row) = ((at % columns as usize) as f32, (at / columns as usize) as f32);
        let width = (content.width - GAP * (columns - 1) as f32) / columns as f32;
        let height = (content.height - GAP * (rows - 1) as f32) / rows as f32;
        Area { x: content.x + column * (width + GAP), y: content.y + row * (height + GAP), width, height }
    };

    let mut pages: Vec<Page> = Vec::new();
    let mut used = cells;
    let mut last_shot = None;
    for board in boards(project, board, &options.range) {
        let new_shot = last_shot.is_some_and(|shot| shot != board.shot);
        if used == cells || (options.page_per_shot && new_shot) {
            pages.push(Page::default());
            used = 0;
        }
        last_shot = Some(board.shot);
        let page = pages.last_mut().expect("a page was started");
        lay_panel(page, &board, cell(used), aspect, rate, options);
        used += 1;
    }

    // Over and under each page, once it is known how many there are.
    let count = pages.len();
    for (at, page) in pages.iter_mut().enumerate() {
        if options.header {
            let title = format!("{} · {}", heading.project, board.name);
            page.texts.push(Text { role: Role::Title, text: title, at: [margin, margin], size: HEAD_SIZE });
            let version = [heading.date.as_str(), options.version.trim()].into_iter().filter(|said| !said.is_empty()).collect::<Vec<_>>().join(" · ");
            if !version.is_empty() {
                let x = size[0] - margin - text_width(&version, HEAD_SIZE, false);
                page.texts.push(Text { role: Role::Version, text: version, at: [x, margin], size: HEAD_SIZE });
            }
        }
        if options.footer {
            let said = format!("Page {} of {count}", at + 1);
            let x = (size[0] - text_width(&said, HEAD_SIZE, false)) / 2.0;
            page.texts.push(Text { role: Role::Footer, text: said, at: [x, size[1] - margin - HEAD_SIZE * LINE], size: HEAD_SIZE });
        }
    }
    Laid { size, pages }
}

/// Lay one panel into `cell` of `page`: its picture, the line of its number, length and
/// timecode, and its texts, under the picture or, in Script, beside it.
fn lay_panel(page: &mut Page, board: &Board, cell: Area, aspect: f32, rate: FrameRate, options: &PageOptions) {
    let size = options.text_size.max(4.0);
    let line = size * LINE;
    let roles = options.texts();
    let captioned = options.numbers || options.length.is_some() || options.timecode;
    let caption = if captioned { line + 2.0 } else { 0.0 };
    let writing = options.writing_lines as f32 * WRITING_LINE;
    let (picture, beside) = if options.mode == Mode::Script {
        // The picture at most a little under half the row across; the texts in the rest.
        let width = (cell.width * 0.45).min(cell.height * aspect);
        let picture = Area { x: cell.x, y: cell.y, width, height: width / aspect };
        let beside = Area { x: picture.right() + GAP, y: cell.y, width: cell.width - width - GAP, height: cell.height };
        (picture, beside)
    } else {
        // The picture as wide as the cell, made smaller if the texts would not have two lines each.
        let wanted = line * 2.0 * roles.len() as f32 + writing;
        let height = (cell.width / aspect).min(cell.height - caption - wanted).max(cell.height * 0.2);
        let width = height * aspect;
        let picture = Area { x: cell.x + (cell.width - width) / 2.0, y: cell.y, width, height };
        // The texts under it as wide as it is, lined up with it.
        let under = Area { x: picture.x, y: picture.bottom() + 3.0, width: picture.width, height: cell.bottom() - picture.bottom() - 3.0 };
        (picture, under)
    };
    let pixels = [picture.width, picture.height].map(|points| (points * options.dpi.max(1) as f32 / 72.0).round().max(1.0) as u32);
    page.pictures.push(Picture { shot: board.shot, frame: board.frame, area: picture, pixels, framed: options.framed });
    if let Some(said) = board.transition.clone().filter(|_| options.transitions) {
        let at = [picture.x + 3.0, picture.y + 3.0];
        page.texts.push(Text { role: Role::Transition, text: said, at, size: (size * 0.85).max(4.0) });
    }

    // The number on the left, the length on the right, the timecode before the length.
    let mut right = beside.right();
    if let Some(length) = options.length {
        let said = said_length(board.frames, rate, length);
        right -= text_width(&said, size, false);
        page.texts.push(Text { role: Role::Length, text: said, at: [right, beside.y], size });
        right -= size;
    }
    if options.timecode {
        let said = timecode(i64::from(board.film), rate);
        right -= text_width(&said, size, false);
        page.texts.push(Text { role: Role::Timecode, text: said, at: [right, beside.y], size });
    }
    if options.numbers {
        let said = if board.count > 1 { format!("{} · {}/{}", board.name, board.place + 1, board.count) } else { board.name.clone() };
        let (said, _) = wrap(&said, (right - beside.x - size).max(size), size, true, 1);
        page.texts.push(Text { role: Role::Number, text: said.concat(), at: [beside.x, beside.y], size });
    }

    // The texts share the room left, each at least its part of it if it needs that, what is
    // left over going to the first that want more; the lines to write on are at its foot.
    let room = Area { y: beside.y + caption, height: (beside.height - caption - writing).max(0.0), ..beside };
    let said: Vec<(Role, String)> = roles
        .into_iter()
        .map(|role| match role {
            Role::Dialogue if !board.panel.dialogue.trim().is_empty() => (role, format!("“{}”", board.panel.dialogue.trim())),
            Role::Dialogue => (role, String::new()),
            Role::Notes => (role, board.panel.notes.trim().to_string()),
            _ => (role, board.panel.action.trim().to_string()),
        })
        .filter(|(_, words)| !words.is_empty())
        .collect();
    let lines = (room.height / line).floor() as usize;
    let needs: Vec<usize> = said.iter().map(|(_, words)| wrap(words, room.width, size, false, usize::MAX).0.len()).collect();
    let share = if said.is_empty() { 0 } else { lines / said.len() };
    let mut given: Vec<usize> = needs.iter().map(|need| (*need).min(share)).collect();
    let mut left = lines - given.iter().sum::<usize>();
    for (given, need) in given.iter_mut().zip(&needs) {
        let more = (need - *given).min(left);
        (*given, left) = (*given + more, left - more);
    }
    let mut y = room.y;
    for ((role, words), most) in said.iter().zip(given) {
        for text in wrap(words, room.width, size, false, most).0 {
            page.texts.push(Text { role: *role, text, at: [room.x, y], size });
            y += line;
        }
    }
    for at in 1..=options.writing_lines {
        let y = room.bottom() + at as f32 * WRITING_LINE - 2.0;
        page.lines.push([[beside.x, y], [beside.right(), y]]);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{Edge, Transition};

    use super::*;

    /// A film at 24 frames a second, 1920 by 1080: Shot_01 of 48 frames with panels on its
    /// frames 0, 12 and 24, the first with an action and a dialogue; Shot_02 of 24, after it, a
    /// dissolve of 12 frames into it; Shot_03 of 24, after that, with two panels.
    fn film() -> (Project, usize) {
        let mut project = Project::default();
        let first = project.shots[0].id;
        let (second, third) = (project.new_shot("Shot_02", 24), project.new_shot("Shot_03", 24));
        let (two, three) = (second.id, third.id);
        project.shots.extend([Arc::new(second), Arc::new(third)]);
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.duration = 48;
        shot.panels = vec![
            Panel { frame: 0, action: "A dark hall.".to_string(), dialogue: "Hello?".to_string(), ..Panel::default() },
            Panel { frame: 12, ..Panel::default() },
            Panel { frame: 24, ..Panel::default() },
        ];
        Arc::make_mut(&mut project.shots[2]).panels = vec![Panel { frame: 12, notes: "Hold".to_string(), ..Panel::default() }];
        let board = Arc::make_mut(&mut project.storyboards[0]);
        board.put(Clip { shot: first, start: 0, in_point: 0, out_point: 48 }, false).unwrap();
        board.put(Clip { shot: two, start: 48, in_point: 0, out_point: 24 }, false).unwrap();
        board.put(Clip { shot: three, start: 72, in_point: 0, out_point: 24 }, false).unwrap();
        board.transitions.push(Transition { shot: two, edge: Edge::Start, kind: TransitionKind::CrossDissolve, length: 12 });
        (project, 0)
    }

    fn lay(options: &PageOptions) -> Laid {
        let (project, board) = film();
        lay_out(&project, &project.storyboards[board], options, &Heading { project: "Hall".to_string(), date: "2026-10-28".to_string() })
    }

    fn texts(page: &Page, role: Role) -> Vec<&str> {
        page.texts.iter().filter(|text| text.role == role).map(|text| text.text.as_str()).collect()
    }

    #[test]
    fn a_paper_is_its_size_in_points_upright_or_on_its_side() {
        let mut options = PageOptions::default();
        let [width, height] = options.page_size();
        assert!((width - 595.3).abs() < 0.1 && (height - 841.9).abs() < 0.1, "A4: {width} by {height}");
        options.landscape = true;
        assert_eq!(options.page_size(), [height, width]);
        options.paper = Paper::Letter;
        options.landscape = false;
        assert_eq!(options.page_size().map(f32::round), [612.0, 792.0]);
        options.paper = Paper::Own { width: 100.0, height: 50.0 };
        assert_eq!(options.page_size().map(f32::round), [283.0, 142.0]);
    }

    #[test]
    fn sheets_lay_the_panels_in_a_grid_across_then_down_page_after_page() {
        let laid = lay(&PageOptions::of(Mode::Sheets));
        // Six panels: three of Shot_01, one of Shot_02, two of Shot_03; six a page.
        assert_eq!(laid.pages.len(), 1);
        let pictures = &laid.pages[0].pictures;
        let at: Vec<(ShotId, u32)> = pictures.iter().map(|picture| (picture.shot, picture.frame)).collect();
        let (project, _) = film();
        let id = |at: usize| project.shots[at].id;
        assert_eq!(at, [(id(0), 0), (id(0), 12), (id(0), 24), (id(1), 0), (id(2), 0), (id(2), 12)]);
        let margin = 12.0 * POINTS_PER_MM;
        let areas: Vec<Area> = pictures.iter().map(|picture| picture.area).collect();
        assert!((areas[0].x - margin).abs() < 0.01, "the first at the margin: {:?}", areas[0]);
        assert!(areas[1].x > areas[0].right() && areas[1].y == areas[0].y, "the second beside it");
        assert!(areas[2].y > areas[0].bottom() && areas[2].x == areas[0].x, "the third under the first");
        assert!(areas.iter().all(|area| (area.width / area.height - 16.0 / 9.0).abs() < 0.01), "the film's shape");
        assert!(areas.iter().all(|area| area.right() <= laid.size[0] - margin + 0.01 && area.bottom() <= laid.size[1] - margin));
        // A seventh panel starts a page: with two rows of two, two pages.
        let mut options = PageOptions { rows: 2, ..PageOptions::of(Mode::Sheets) };
        assert_eq!(lay(&options).pages.iter().map(|page| page.pictures.len()).collect::<Vec<_>>(), [4, 2]);
        // Each shot a page of its own.
        options.page_per_shot = true;
        assert_eq!(lay(&options).pages.iter().map(|page| page.pictures.len()).collect::<Vec<_>>(), [3, 1, 2]);
    }

    #[test]
    fn under_each_picture_are_its_number_length_and_texts_within_its_cell() {
        let laid = lay(&PageOptions { notes: true, ..PageOptions::of(Mode::Sheets) });
        let page = &laid.pages[0];
        assert_eq!(texts(page, Role::Number), ["Shot_01 · 1/3", "Shot_01 · 2/3", "Shot_01 · 3/3", "Shot_02", "Shot_03 · 1/2", "Shot_03 · 2/2"]);
        assert_eq!(texts(page, Role::Length), ["0.5 s", "0.5 s", "1.0 s", "1.0 s", "0.5 s", "0.5 s"]);
        assert_eq!((texts(page, Role::Action), texts(page, Role::Dialogue), texts(page, Role::Notes)), (vec!["A dark hall."], vec!["“Hello?”"], vec!["Hold"]));
        // The texts of the first panel are under its picture, the dialogue under the action.
        let picture = page.pictures[0].area;
        let at = |role: Role| page.texts.iter().find(|text| text.role == role).unwrap().at;
        let (number, action, dialogue) = (at(Role::Number), at(Role::Action), at(Role::Dialogue));
        assert!(number[1] > picture.bottom() && action[1] > number[1] && dialogue[1] > action[1], "{number:?} {action:?} {dialogue:?}");
        // The dissolve into Shot_02 is marked over its picture's corner.
        let mark = page.texts.iter().find(|text| text.role == Role::Transition).unwrap();
        assert_eq!(mark.text, "Dissolve in, 12 fr");
        assert!(mark.at[0] > page.pictures[3].area.x && mark.at[1] > page.pictures[3].area.y);
        // The header and the footer.
        assert_eq!(
            (texts(page, Role::Title), texts(page, Role::Version), texts(page, Role::Footer)),
            (vec!["Hall · Storyboard"], vec!["2026-10-28"], vec!["Page 1 of 1"])
        );
    }

    #[test]
    fn a_long_text_is_wrapped_to_its_cell_and_cut_with_an_ellipsis() {
        let (lines, cut) = wrap("one two three four five six seven eight nine ten", 60.0, 9.0, false, 3);
        assert!(cut && lines.len() == 3 && lines[2].ends_with('…'), "{lines:?}");
        assert!(lines.iter().all(|line| text_width(line, 9.0, false) <= 60.0), "{lines:?}");
        // Its own line breaks are kept; a word too long for a line is broken.
        assert_eq!(wrap("one\ntwo", 200.0, 9.0, false, 9), (vec!["one".to_string(), "two".to_string()], false));
        let (lines, _) = wrap("Supercalifragilistic", 30.0, 9.0, false, 9);
        assert!(lines.len() > 1 && lines.iter().all(|line| text_width(line, 9.0, false) <= 30.0), "{lines:?}");

        // On a page: a long action in a small cell keeps to the cell, and leaves the dialogue room.
        let (mut project, _) = film();
        let long = "Someone is out there in the dark. They call again, louder, and the hall answers with nothing but their own voice. ".repeat(4);
        Arc::make_mut(&mut project.shots[0]).panels[0].action = long;
        let options = PageOptions { columns: 3, rows: 4, ..PageOptions::of(Mode::Sheets) };
        let laid = lay_out(&project, &project.storyboards[0], &options, &Heading::default());
        let page = &laid.pages[0];
        let cell_right = page.pictures[1].area.x - GAP;
        let action: Vec<&Text> = page.texts.iter().filter(|text| text.role == Role::Action).collect();
        assert!(action.last().unwrap().text.ends_with('…'), "cut off");
        assert!(action.iter().all(|text| text.at[0] + text_width(&text.text, text.size, false) <= cell_right + 0.01));
        assert_eq!(texts(page, Role::Dialogue), ["“Hello?”"], "the dialogue has its share");
        let foot = page.pictures[3].area.y - GAP;
        assert!(page.texts.iter().filter(|text| text.role == Role::Dialogue).all(|text| text.at[1] + text.size * LINE <= foot + 0.01));
    }

    #[test]
    fn script_has_a_panel_a_row_its_texts_beside_its_picture() {
        let laid = lay(&PageOptions::of(Mode::Script));
        assert_eq!(laid.pages.iter().map(|page| page.pictures.len()).collect::<Vec<_>>(), [4, 2]);
        let page = &laid.pages[0];
        let picture = page.pictures[0].area;
        let action = page.texts.iter().find(|text| text.role == Role::Action).unwrap();
        assert!(action.at[0] > picture.right() && action.at[1] < picture.bottom(), "beside: {:?} {picture:?}", action.at);
        assert!(page.pictures[1].area.y > picture.bottom() && page.pictures[1].area.x == picture.x, "rows");
        // With the timecode where each starts in the film.
        assert_eq!(texts(page, Role::Timecode), ["00:00:00:00", "00:00:00:12", "00:00:01:00", "00:00:02:00"]);
    }

    #[test]
    fn presentation_has_a_panel_a_page_and_thumbnails_many_with_numbers_alone() {
        let laid = lay(&PageOptions::of(Mode::Presentation));
        assert!(laid.size[0] > laid.size[1], "landscape");
        assert_eq!(laid.pages.len(), 6);
        assert!(laid.pages[0].pictures[0].area.width > laid.size[0] * 0.5, "large");
        assert_eq!(texts(&laid.pages[5], Role::Footer), ["Page 6 of 6"]);

        let laid = lay(&PageOptions { action: true, ..PageOptions::of(Mode::Thumbnails) });
        let page = &laid.pages[0];
        assert_eq!(page.pictures.len(), 6);
        assert!(texts(page, Role::Action).is_empty() && texts(page, Role::Length).is_empty(), "no texts, even asked for");
        assert_eq!(texts(page, Role::Number).len(), 6);
    }

    #[test]
    fn the_range_picks_shots_or_a_stretch_of_the_film() {
        let (project, _) = film();
        let options = PageOptions { range: Range::Shots(vec![project.shots[2].id]), ..PageOptions::default() };
        let page = &lay(&options).pages[0];
        assert_eq!(texts(page, Role::Number), ["Shot_03 · 1/2", "Shot_03 · 2/2"]);
        // Frames 20 to 50: the second and third panels of Shot_01, and Shot_02.
        let options = PageOptions { range: Range::Film { first: 20, after: 50 }, ..PageOptions::default() };
        let page = &lay(&options).pages[0];
        assert_eq!(texts(page, Role::Number), ["Shot_01 · 2/3", "Shot_01 · 3/3", "Shot_02"]);
    }

    #[test]
    fn lines_to_write_on_and_the_pictures_pixels_follow_the_options() {
        let laid = lay(&PageOptions { writing_lines: 2, dpi: 300, ..PageOptions::default() });
        let page = &laid.pages[0];
        assert_eq!(page.lines.len(), 12, "two a panel");
        let picture = &page.pictures[0];
        assert_eq!(picture.pixels[0], (picture.area.width * 300.0 / 72.0).round() as u32);
        // The lines are in the panel's cell, under its texts.
        let cell_foot = page.pictures[2].area.y - GAP;
        let [[x, y], [right, _]] = page.lines[1];
        assert!(x <= picture.area.x && right > x && y <= cell_foot + 0.01 && y > picture.area.bottom(), "{:?}", page.lines[1]);
    }
}
