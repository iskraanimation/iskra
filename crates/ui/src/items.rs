//! Panels that show the project's items (plan §7.2): the storyboard timeline, a film editor's
//! timeline with the shots as clips and one playhead; and the layers panel, which lists the
//! layers of the shot under the playhead. The bins and the media pool are in `media`.

use egui::{Align2, Color32, CursorIcon, FontId, Rect, Sense, Stroke, pos2, vec2};
use iskra_model::{Interp, LayerId, MediaId, MediaKind, PegId, Project, Shot, ShotId, SoundClipId, Status, Storyboard, StoryboardId};
use serde_json::{Value, json};

use crate::theme::{self, Theme};

/// What the user asked for in one of these panels. The app carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Run(&'static str, Value),
    /// Make this the shot the viewer and the timeline show.
    SelectShot(ShotId),
    /// Put the storyboard's playhead on this frame of the film.
    Seek(u32),
    /// Make this the layer that is drawn on.
    SelectLayer(LayerId),
    /// Put the open shot's camera in hand: the peg that moves it, with the Camera tool.
    SelectCamera,
    /// Run one of the app's commands, by its id, as the menus and the keys do.
    Command(&'static str),
    /// What a part of the bar shared with the other bars asked for (see `bar::Asked`).
    Bar(crate::bar::Asked),
    /// Split this shot on this frame of its own: the razor's click on its clip.
    Split(ShotId, u32),
    /// Show this shot on the Animation page.
    OpenShot(ShotId),
    /// Put this shot at this place among the storyboard's shots (0 is the first).
    Reorder(ShotId, usize),
    /// Show this storyboard on the Storyboard page.
    OpenStoryboard(StoryboardId),
    /// Send these shots out as files of their own, once the user has said where to.
    SendOut(Vec<ShotId>),
    /// Open the Rename Shots window on these shots.
    RenameShots(Vec<ShotId>),
    /// Bring a shot back from a file the user picks: this one, or the one the file is of.
    BringBack(Option<ShotId>),
    /// Items of the media pool are being dragged, to be put into a bin.
    Drag(Vec<iskra_model::Item>),
    /// Point a media item at another file, which the user picks.
    Relink(iskra_model::MediaId),
    /// Ask for a file with what is spoken in this sound, and give the sound that transcript.
    Transcript(iskra_model::MediaId),
    /// Put a picture or an image sequence into the shot that is worked on, as a layer.
    Place(iskra_model::MediaId),
    /// Listen to a sound item, or stop listening to it.
    Listen(iskra_model::MediaId),
    /// Start recording sound from the playhead on; with `true` the film plays meanwhile.
    Record(bool),
    /// End the take, and lay it under the shots where it was started.
    StopRecording,
}

/// The colour of a shot's status, the same in both themes: as the user set it, or its own.
pub fn status_color(ctx: &egui::Context, status: Status) -> egui::Color32 {
    let role = match status {
        Status::Board => theme::Role::StatusBoard,
        Status::Out => theme::Role::StatusOut,
        Status::Animation => theme::Role::StatusAnimation,
        Status::Post => theme::Role::StatusPost,
        Status::Final => theme::Role::StatusFinal,
    };
    Theme::of(ctx).colour(role)
}

/// A shot's status in words, with whom it is while it is out.
pub fn status_text(shot: &Shot) -> String {
    match (&shot.out_to, shot.status) {
        (Some(to), Status::Out) => format!("Out with {to}"),
        (_, status) => status.label().to_string(),
    }
}

/// Points per frame on the storyboard timeline.
const FRAME_WIDTH: f32 = 3.0;
/// The fewest and the most points a frame can be zoomed to.
const ZOOM: std::ops::RangeInclusive<f32> = 0.25..=24.0;
/// How near, in points, a dragged clip's end comes to another's before it is caught by it.
const SNAP_REACH: f32 = 8.0;
const CLIP_HEIGHT: f32 = 40.0;
const RULER_HEIGHT: f32 = 18.0;
/// The height of one layer's row under its shot's clip.
const ROW_HEIGHT: f32 = 18.0;
/// How much of each end of a clip is its trim handle.
const EDGE_WIDTH: f32 = 6.0;
/// How wide the column of names is: as on the Animation page's timeline.
const NAMES_WIDTH: f32 = 150.0;
/// The narrowest and the widest the column of names is dragged to.
const NAMES_RANGE: std::ops::RangeInclusive<f32> = 90.0..=600.0;

/// A row of the column of names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameRow {
    /// The one track of shots.
    Shots,
    Layer(LayerId),
    /// The track of the peg over a board, or of its bones, each with the board's layer.
    Peg(LayerId),
    Bones(LayerId),
    /// The open shot's camera.
    Camera,
    /// A sound track, counted from the top.
    Sound(usize),
}

/// How high a sound track's row is.
const SOUND_HEIGHT: f32 = 30.0;

/// What a waveform draws of a sound item: the lowest and the highest value of each frame of
/// the film it lasts. `None` for a sound whose file can't be read.
pub type Waves<'a> = &'a mut dyn FnMut(MediaId) -> Option<std::sync::Arc<Vec<[f32; 2]>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grip {
    /// The whole clip, to move it.
    Body,
    /// Its start: the in point.
    In,
    /// Its end: the out point.
    Out,
}

/// A clip being dragged, and by how many frames so far.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ClipDrag {
    shot: ShotId,
    grip: Grip,
    frames: i64,
}

/// A sound clip being dragged, by how many frames so far, and the track the pointer is over.
/// The kinds of transition, as commands name them and as menus do.
const TRANSITIONS: [(&str, &str); 3] = [("crossDissolve", "Cross Dissolve"), ("dipToBlack", "Dip to Black"), ("dipToWhite", "Dip to White")];

/// A panel's mark being dragged along its shot's clip: whose, the frame the panel starts on,
/// and by how many frames so far.
#[derive(Clone, Copy)]
struct PanelDrag {
    shot: ShotId,
    frame: u32,
    frames: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SoundDrag {
    clip: SoundClipId,
    grip: Grip,
    frames: i64,
    track: usize,
}

/// What the storyboard timeline is showing.
#[derive(Clone, Copy)]
pub struct Playhead {
    /// The frame of the film the playhead is on.
    pub frame: u32,
    /// The frame the bar says it is on: while the film plays or the playhead is dragged, the
    /// one it was on before, so that the number does not run.
    pub shown: u32,
    /// The shot under it, if it is on one, and the layer that is drawn on.
    pub shot: Option<ShotId>,
    pub layer: Option<LayerId>,
    /// Whether the camera is in hand, in place of the layer: its row is lit, and the keyframe
    /// commands key it.
    pub camera: bool,
    /// Whether what the keyframe commands work on, the layer in hand's peg or the camera, has
    /// a keyframe on the frame: `None` where there is no such peg.
    pub keyed: Option<bool>,
    /// Key All has something to key.
    pub can_key_all: bool,
    /// Extend has a hold to take on to the playhead.
    pub extendable: bool,
    /// What *Keyframe Cycle* shows of the keyframes picked.
    pub cycled: crate::bar::Cycled,
    pub playing: bool,
    /// A take of sound is being recorded: how long it is so far, in seconds, and how loud
    /// what the input heard last was, from 0 to 1.
    pub recording: Option<(f64, f32)>,
}

/// The storyboard timeline (plan §7.2b): a film editor's timeline. One shot track with free
/// placement and one playhead, which runs along the film; the shot under it is the one the
/// viewer shows, and its layers open up under its clip as rows of drawings, at film time.
#[derive(Default)]
pub struct StoryboardView {
    /// Which parts of the bar are shown, and in what order: the app's preferences keep it,
    /// and hand it over before the timeline is shown.
    pub bar: crate::bar::BarLayout,
    /// Snap, Ripple, the Razor and Loop, the app's: handed over before the timeline is shown,
    /// and taken back as it is left (see `bar::Switches`). With Ripple the later shots move
    /// along to make room; off, they stay and what would run into them is refused.
    pub bar_switches: crate::bar::Switches,
    /// The frames of a shot that the playback cache has made and keeps ready, for the line on
    /// the ruler over its clip: the app hands them over before the timeline is shown.
    pub cached: (Option<ShotId>, Vec<u32>),
    /// Where that line was drawn: the first frame of the film of each run, and the one after.
    #[cfg(test)]
    pub cache_marks: Vec<(u32, u32)>,
    /// The end of a board drawing's hold being dragged: its layer, its first frame of the
    /// shot, and how long it is where the pointer is now.
    hold_drag: Option<(LayerId, u32, u32)>,
    /// Where each board drawing's hold handle was drawn: layer, the hold's first frame, the handle.
    #[cfg(test)]
    pub hold_handles: Vec<(LayerId, u32, Rect)>,
    /// What the clips are coloured by, and the colours: the preferences', handed over before
    /// the timeline is shown and taken back after.
    pub clip_colours: crate::scenes::ClipColours,
    /// The colour each clip was drawn in, where it was its scene's or sequence's.
    #[cfg(test)]
    pub clip_fills: Vec<(ShotId, Color32)>,
    /// The Animate switch, the preferences': handed over before the storyboard is shown, and
    /// taken back as it is left.
    pub animate: bool,
    /// The Adjust switch beside it, handed over and taken back as `animate` is.
    pub adjust: bool,
    /// How long a new drawing holds, the preferences': handed over and taken back as `animate` is.
    pub hold: crate::prefs::NewDrawingHold,
    /// How the timing charts' lines are shown and dragged, handed over and taken back as
    /// `animate` is, the same on the Animation timeline.
    pub breakdowns: crate::breakdown::Breakdowns,
    /// A line of a timing chart being dragged.
    chart_drag: Option<crate::breakdown::ChartDrag>,
    /// Where each timing chart's line was drawn: whose row, the way's first frame, the line.
    #[cfg(test)]
    pub breakdown_lines: Vec<(KeyOwner, u32, Rect)>,
    /// The layers that are not shown in the viewer, and those that are not to be drawn on:
    /// switched on their rows. They are the window's, not the project's.
    pub hidden: std::collections::BTreeSet<LayerId>,
    pub locked: std::collections::BTreeSet<LayerId>,
    /// The shot track is locked: its clips are not moved or trimmed. And the sound tracks
    /// that are, by their places.
    pub shots_locked: bool,
    pub sound_locked: std::collections::BTreeSet<usize>,
    /// Where the switches of the rows were drawn: the row's name, what the switch is, its place.
    #[cfg(test)]
    pub switches: Vec<(String, &'static str, Rect)>,
    /// A marker being dragged along the ruler: the frame it is on, and by how many frames so far.
    marker_drag: Option<(u32, i64)>,
    /// The marker whose name is being typed in its menu, with the text so far.
    marker_name: Option<(u32, String)>,
    /// Where the markers were drawn, by their frames, and where the ends of the stretch that
    /// plays were, if one is set.
    #[cfg(test)]
    pub markers: Vec<(u32, Rect)>,
    /// Where the shots' retake notes were marked, by their shots and ids.
    #[cfg(test)]
    pub note_marks: Vec<(ShotId, u64, Rect)>,
    #[cfg(test)]
    pub range_ends: Option<(f32, f32)>,
    /// A take is recorded without the film playing meanwhile.
    pub record_still: bool,
    /// How many points a frame is wide, if the timeline was zoomed.
    pub frame_width: Option<f32>,
    /// The bar's Fit was clicked: the frames are made as wide as fits the whole film in.
    fitting: bool,
    /// Where the bar's Fit was drawn.
    #[cfg(test)]
    pub fit_button: Option<Rect>,
    /// How wide the column of names is, if its edge was dragged.
    pub names_wide: Option<f32>,
    /// The column's edge, where it is dragged.
    #[cfg(test)]
    pub names_edge: Option<Rect>,
    /// Where the bar's Add Key button was drawn.
    #[cfg(test)]
    pub key_button: Option<Rect>,
    /// The name that is being edited was opened just now: it is selected whole, once.
    rename_fresh: bool,
    /// How wide a frame was when the timeline was last shown, and how far it was scrolled
    /// sideways: zoomed, the playhead's frame stays where it is in the window.
    shown_width: f32,
    scrolled: f32,
    drag: Option<ClipDrag>,
    sound_drag: Option<SoundDrag>,
    /// Where sound from the media pool would be laid, as the timeline was last drawn.
    pub(crate) sound_drop: Option<crate::sound::SoundDrop>,
    panel_drag: Option<PanelDrag>,
    /// A transition being dragged longer or shorter: the edge it is kept on, and by how many
    /// frames so far.
    transition_drag: Option<(ShotId, iskra_model::Edge, i64)>,
    /// Where the transitions were drawn: the edge each is kept on, and its band.
    #[cfg(test)]
    pub transitions: Vec<(ShotId, iskra_model::Edge, Rect)>,
    /// Where the marks of the panels were drawn on their shots' clips: the shot, the frame of
    /// it the panel starts on, the mark.
    #[cfg(test)]
    pub panel_marks: Vec<(ShotId, u32, Rect)>,
    /// What is picked together on the timeline: shots, sound clips, and keyframes of the open
    /// shot's camera by their frames. Picked by a click with Shift held or by a box dragged
    /// out round them; one of them that is dragged takes the others along.
    pub(crate) picked_shots: Vec<ShotId>,
    /// Where the length of a transition was last offered to be typed, in its menu.
    #[cfg(test)]
    pub length_field: Option<Rect>,
    pub(crate) picked_sounds: Vec<SoundClipId>,
    pub(crate) picked_keys: Vec<(KeyOwner, u32)>,
    /// A volume that is being dragged: whose, a clip's by its id or the master's by 0, and
    /// what it is so far.
    volume_edit: Option<(u64, f32)>,
    /// The box that is being dragged out to pick with: where it was pressed, and where the
    /// pointer is.
    marquee: Option<(egui::Pos2, egui::Pos2)>,
    /// A keyframe being dragged: the shot, whose it is, the frame it is on, and the frame it
    /// would land on.
    key_drag: Option<(ShotId, KeyOwner, u32, u32)>,
    /// Where each of the camera's keyframes was drawn, by its frame of the shot.
    #[cfg(test)]
    pub camera_keys: Vec<(u32, Rect)>,
    /// Each keyframe a cycle repeats that was drawn, by its row and frame.
    #[cfg(test)]
    pub repeated_marks: Vec<(KeyOwner, u32)>,
    /// Where each of the keyframes of the boards' pegs was drawn, by the peg and its frame.
    #[cfg(test)]
    pub board_keys: Vec<(PegId, u32, Rect)>,
    /// And of the boards' bones.
    #[cfg(test)]
    pub bone_keys: Vec<(iskra_model::DeformerId, u32, Rect)>,
    /// The lines between the camera's keyframes that were drawn: from which frame of the
    /// shot to which.
    #[cfg(test)]
    pub camera_tweens: Vec<(u32, u32)>,
    /// The row whose name is being edited, with the text so far.
    renaming: Option<(NameRow, String)>,
    /// Where each row's name was drawn.
    #[cfg(test)]
    pub names: Vec<(String, Rect)>,
    /// Where each sound clip was drawn.
    #[cfg(test)]
    pub sound_clips: Vec<(SoundClipId, Rect)>,
    /// Where each clip was drawn, and where frame 0 of the film is.
    #[cfg(test)]
    pub clips: Vec<(ShotId, Rect)>,
    /// What each clip said of the styles its shot is shown in.
    #[cfg(test)]
    pub clip_styles: Vec<(ShotId, String)>,
    /// The shots whose clips were drawn lighter: every second one of the film.
    #[cfg(test)]
    pub lighter: Vec<ShotId>,
    #[cfg(test)]
    pub origin: egui::Pos2,
}

impl StoryboardView {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        project: &Project,
        board: &Storyboard,
        at: Playhead,
        waves: Waves,
        elsewhere: &mut dyn FnMut(&mut egui::Ui, &'static str),
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        // The bar's parts, each shown, hidden and put elsewhere by the bar itself.
        let mut bar = std::mem::take(&mut self.bar);
        let parts = bar.parts(crate::dock::Panel::Storyboard);
        bar.show(ui, "storyboard bar", &parts, |ui, part| match part {
            "play" => actions.extend(crate::bar::playback(ui, &mut self.bar_switches, at.playing, true).map(Action::Command)),
            "frame" => {
                ui.label(egui::RichText::new(format!("{:>5} / {}", at.shown + 1, board.end())).font(theme::mono(12.0)))
                    .on_hover_text("The playhead's frame of the film, and the film's length");
            }
            "animate" => {
                let drawn = crate::bar::animate(ui, &mut self.animate, &mut self.adjust, at.keyed, at.can_key_all);
                #[cfg(test)]
                {
                    self.key_button = Some(drawn.key);
                }
                actions.extend(drawn.asked.map(Action::Command));
            }
            "convert" => actions.extend(crate::bar::convert(ui, at.shot.is_some() && !self.picked_keys.is_empty()).map(Action::Bar)),
            "cycle" => actions.extend(crate::bar::cycle(ui, at.cycled).map(Action::Bar)),
            "breakdown" => {
                let shot = at.shot.and_then(|shot| project.shot(shot));
                if crate::breakdown::menu(ui, &mut self.breakdowns, shot.is_some() && !self.picked_keys.is_empty())
                    && let Some(shot) = shot
                {
                    let commands: Vec<Value> = self.picked_keys.iter().flat_map(|(owner, frame)| owner.charted(shot, *frame, &[])).collect();
                    if !commands.is_empty() {
                        actions.push(Action::Run("edit.batch", json!({ "commands": commands })));
                    }
                }
            }
            "layer" => actions.extend(crate::bar::add_layer(ui, at.shot.map(|shot| shot.0)).map(Action::Bar)),
            "drawings" => actions.extend(crate::bar::drawings(ui, &mut self.hold, at.layer.is_some(), at.extendable).map(Action::Bar)),
            "shots" => {
                let hint = "A new shot right after this one, as long as it, with a layer to draw on. With Ripple the later shots make room";
                if ui.button("Insert Shot").on_hover_text(hint).clicked() {
                    actions.push(Action::Run(
                        "shot.new",
                        match at.shot {
                            Some(shot) => json!({ "after": shot.0, "boards": true, "ripple": self.bar_switches.ripple }),
                            None => json!({ "storyboard": board.id.0, "boards": true }),
                        },
                    ));
                }
                if let Some(shot) = at.shot {
                    if ui.button("Duplicate").on_hover_text("Copy the shot with its drawings, for a continuing action").clicked() {
                        actions.push(Action::Run("shot.duplicate", json!({ "shot": shot.0, "ripple": self.bar_switches.ripple })));
                    }
                    if ui.button("Delete Shot").on_hover_text("Delete the shot with its drawings. Undo brings it back").clicked() {
                        actions.push(Action::Run("shot.delete", json!({ "shot": shot.0 })));
                    }
                }
            }
            "sound" => {
                // Sound is laid under the shots from the playhead on.
                let sounds: Vec<&iskra_model::Media> =
                    project.media.iter().map(|media| &**media).filter(|media| matches!(media.kind, MediaKind::Sound { .. })).collect();
                ui.menu_button("Add Sound", |ui| {
                    if sounds.is_empty() {
                        ui.weak("The project has no sound yet.\nFile › Import › Media… brings it in.");
                    }
                    for media in sounds {
                        if ui.button(&media.name).on_hover_text("Lay it on a sound track, starting at the playhead").clicked() {
                            actions.push(Action::Run("sound.place", json!({ "storyboard": board.id.0, "media": media.id.0, "start": at.frame })));
                            ui.close();
                        }
                    }
                });
                // Sound is recorded from the playhead on, and laid there when the take ends.
                match at.recording {
                    Some((seconds, level)) => {
                        let said = format!("Stop  {}:{:04.1}", (seconds / 60.0) as u32, seconds % 60.0);
                        let stop = ui.add(egui::Button::new(egui::RichText::new(said).color(Color32::WHITE)).fill(Color32::from_rgb(190, 50, 45)));
                        if stop.on_hover_text("End the take: it is kept in the project's Audio folder and laid under the shots where it was started").clicked()
                        {
                            actions.push(Action::StopRecording);
                        }
                        // How loud the input is.
                        let (meter, _) = ui.allocate_exact_size(vec2(40.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(meter, 2.0, Theme::of(ui.ctx()).surface_raised);
                        let heard = Rect::from_min_size(meter.min, vec2(meter.width() * level.clamp(0.0, 1.0), meter.height()));
                        ui.painter().rect_filled(heard, 2.0, if level > 0.95 { Color32::from_rgb(219, 68, 55) } else { Color32::from_rgb(15, 157, 88) });
                    }
                    None => {
                        ui.menu_button("Record", |ui| {
                            let hint =
                                "Record from the system's sound input, from the playhead on. The take is kept as a WAV file in the project's Audio folder";
                            if ui.button("Record from the Playhead").on_hover_text(hint).clicked() {
                                actions.push(Action::Record(!self.record_still));
                                ui.close();
                            }
                            let mut plays = !self.record_still;
                            ui.checkbox(&mut plays, "Play the film meanwhile")
                                .on_hover_text("To speak a line to the picture. With headphones, or the input hears the film too");
                            self.record_still = !plays;
                        });
                    }
                }
            }
            "edit" => crate::bar::edit(ui, &mut self.bar_switches, true),
            "marks" | "range" => {
                let here = board.markers.iter().find(|marker| marker.frame == at.frame).map(|marker| marker.colour);
                let marks = crate::bar::Marks { of: ("storyboard", board.id.0), frame: at.frame, here, ranged: board.range.is_some() };
                let asked = if part == "marks" { crate::bar::markers(ui, Some(&marks)) } else { crate::bar::range(ui, Some(&marks)) };
                actions.extend(asked.map(Action::Bar));
            }
            "zoom" => {
                // 100% is a frame as wide as it starts.
                let range = ZOOM.start() / FRAME_WIDTH * 100.0..=ZOOM.end() / FRAME_WIDTH * 100.0;
                let percent = self.frame_width.unwrap_or(FRAME_WIDTH) / FRAME_WIDTH * 100.0;
                let zoomed = crate::bar::zoom(ui, percent, range, "How wide a frame is: at 100%, as it starts", "Fit the whole film in the timeline's width");
                if let Some(percent) = zoomed.to {
                    self.frame_width = Some(percent / 100.0 * FRAME_WIDTH);
                }
                self.fitting |= zoomed.fit;
                #[cfg(test)]
                {
                    self.fit_button = Some(zoomed.fit_button);
                }
            }
            "volume" => {
                // The master volume, the project's: all of its sound, played and in a movie.
                ui.scope(|ui| {
                    ui.spacing_mut().slider_width = 60.0;
                    ui.weak("Volume").on_hover_text("The master volume: how loud all of the project's sound is, played and in a movie");
                    if let Some(volume) = crate::sound::volume_slider(ui, &mut self.volume_edit, 0, project.settings.volume) {
                        actions.push(Action::Run("project.setVolume", json!({ "volume": volume })));
                    }
                });
            }
            "name" => {
                ui.weak(&board.name);
            }
            // A part added to the bar that acts on the app: the app draws it.
            other => elsewhere(ui, other),
        });
        self.bar = bar;
        ui.separator();
        // The names stay on the left while the frames scroll sideways; up and down, both go
        // together.
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::TOP), |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let column = self.names(ui, project, board, at, &mut actions);
                // Zoomed, the frames go apart from the playhead, which stays where it is in
                // the window: the timeline is scrolled by as much as its frame moved.
                let fitting = std::mem::take(&mut self.fitting);
                if fitting {
                    self.frame_width = Some((ui.available_width() / board.end().max(1) as f32).clamp(*ZOOM.start(), *ZOOM.end()));
                }
                let width = self.frame_width.unwrap_or(FRAME_WIDTH);
                let mut frames = egui::ScrollArea::horizontal().auto_shrink(false);
                if fitting {
                    frames = frames.horizontal_scroll_offset(0.0);
                } else if self.shown_width > 0.0 && width != self.shown_width {
                    frames = frames.scroll_offset(vec2((self.scrolled + at.frame as f32 * (width - self.shown_width)).max(0.0), 0.0));
                }
                let shown = frames.show(ui, |ui| self.track(ui, project, board, at, waves, &mut actions));
                (self.shown_width, self.scrolled) = (width, shown.state.offset.x);
                // The column's right edge, the line between the names and the frames, is
                // dragged to make it wider or narrower; a double click puts it back as it starts.
                let (theme, wide) = (Theme::of(ui.ctx()), column.width());
                let edge = Rect::from_x_y_ranges(column.right() - 3.0..=column.right() + 3.0, column.y_range());
                let held = ui.interact(edge, ui.id().with("names edge"), Sense::click_and_drag());
                let held = held
                    .on_hover_cursor(CursorIcon::ResizeHorizontal)
                    .on_hover_text("Drag to make the names wider or narrower. A double click puts the column back");
                if held.dragged() {
                    self.names_wide = Some((wide + held.drag_delta().x).clamp(*NAMES_RANGE.start(), *NAMES_RANGE.end()));
                }
                if held.double_clicked() {
                    self.names_wide = None;
                }
                let line = if held.dragged() || held.hovered() { Stroke::new(2.0, theme.accent) } else { Stroke::new(1.0, theme.border) };
                ui.painter().vline(column.right(), column.y_range(), line);
                #[cfg(test)]
                {
                    self.names_edge = Some(edge);
                }
            });
        });
        actions
    }

    /// The column of names (plan §7.13): a row for each row of the timeline, in its order.
    /// The shot track, the layers of the shot under the playhead, the sound tracks.
    fn names(&mut self, ui: &mut egui::Ui, project: &Project, board: &Storyboard, at: Playhead, actions: &mut Vec<Action>) -> Rect {
        let theme = Theme::of(ui.ctx());
        let open = at.shot.and_then(|id| Some((project.shot(id)?, *board.clip(id)?)));
        let listed = open.map(|(shot, _)| board_rows(shot)).unwrap_or_default();
        // The open shot has a row for its camera under its layers.
        let rows = listed.len() + usize::from(open.is_some());
        let height = RULER_HEIGHT + CLIP_HEIGHT + ROW_HEIGHT * rows as f32 + SOUND_HEIGHT * board.sound.len() as f32 + 4.0;
        #[cfg(test)]
        self.names.clear();
        let wide = self.names_wide.unwrap_or(NAMES_WIDTH).clamp(*NAMES_RANGE.start(), *NAMES_RANGE.end());
        let column = ui.allocate_ui_with_layout(vec2(wide, height), egui::Layout::top_down(egui::Align::LEFT), |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.allocate_exact_size(vec2(wide, RULER_HEIGHT), Sense::hover());
            // The tracks whose mute switch was clicked.
            let mut switched = Vec::new();
            // One row of the kind `row`, `high` points high: its name, to click and to rename.
            let mut named = |ui: &mut egui::Ui, row: NameRow, high: f32, name: &str, selected: bool, size: f32| -> Option<egui::Ui> {
                let (rect, _) = ui.allocate_exact_size(vec2(wide, high), Sense::hover());
                ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, theme.border));
                #[cfg(test)]
                self.names.push((name.to_string(), rect));
                let mut inside =
                    ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 1.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                if let Some((_, text)) = self.renaming.as_mut().filter(|(renamed, _)| *renamed == row) {
                    let edit = inside.add(egui::TextEdit::singleline(text).desired_width(wide - 40.0));
                    if std::mem::take(&mut self.rename_fresh) {
                        // It opens selected whole, to be typed over.
                        edit.request_focus();
                        let mut state = egui::TextEdit::load_state(inside.ctx(), edit.id).unwrap_or_default();
                        let whole = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
                        state.cursor.set_char_range(Some(whole));
                        state.store(inside.ctx(), edit.id);
                    } else if edit.lost_focus() {
                        // Enter keeps the new name; Escape, or the same name, changes nothing.
                        let keep = !inside.input(|i| i.key_pressed(egui::Key::Escape)) && text != name && !text.trim().is_empty();
                        if let Some((_, text)) = self.renaming.take().filter(|_| keep) {
                            actions.push(match row {
                                NameRow::Layer(layer) => Action::Run("layer.rename", json!({ "layer": layer.0, "name": text })),
                                NameRow::Sound(track) => Action::Run("sound.setTrack", json!({ "storyboard": board.id.0, "track": track, "name": text })),
                                NameRow::Shots | NameRow::Camera | NameRow::Peg(_) | NameRow::Bones(_) => return None,
                            });
                        }
                    } else {
                        edit.request_focus();
                    }
                    return None;
                }
                let label = inside.add(egui::Button::selectable(selected, egui::RichText::new(name).size(size)).truncate());
                match row {
                    NameRow::Layer(layer) | NameRow::Peg(layer) | NameRow::Bones(layer) if label.clicked() => actions.push(Action::SelectLayer(layer)),
                    NameRow::Camera if label.clicked() => actions.push(Action::SelectCamera),
                    _ => {}
                }
                // A board's menu: what is done to the layer, and a drawing on it.
                if let NameRow::Layer(layer) = row {
                    let (index, count) = open.and_then(|(shot, _)| shot.place(layer)).unwrap_or((0, 0));
                    label.context_menu(|ui| {
                        if ui.button("Add New Layer").on_hover_text("A new board on top of the others").clicked() {
                            actions.push(Action::Command("layer.new"));
                            ui.close();
                        }
                        if ui.button("Add New Text Layer").on_hover_text("A new board of texts on top of the others").clicked() {
                            actions.push(Action::Command("layer.newText"));
                            ui.close();
                        }
                        if ui.button("Add New Drawing").on_hover_text("A new, empty drawing on this layer at the playhead").clicked() {
                            actions.push(Action::SelectLayer(layer));
                            actions.push(Action::Command("drawing.newDrawing"));
                            ui.close();
                        }
                        if ui.add_enabled(index + 1 < count, egui::Button::new("Move Down")).on_hover_text("Under the layer below it").clicked() {
                            actions.push(Action::Run("layer.move", json!({ "layer": layer.0, "index": index + 1 })));
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Delete").on_hover_text("Delete the layer with its drawings").clicked() {
                            actions.push(Action::Run("layer.delete", json!({ "layer": layer.0 })));
                            ui.close();
                        }
                    });
                }
                let renamed = matches!(row, NameRow::Layer(_) | NameRow::Sound(_));
                if label.double_clicked() && renamed {
                    self.renaming = Some((row, name.to_string()));
                    self.rename_fresh = true;
                }
                if renamed {
                    label.on_hover_text("Double-click to rename");
                }
                Some(inside)
            };
            // A small switch at the right of a row: an icon that is one of two, and whether
            // it was clicked.
            let muted = theme.text_muted;
            let mut flips: Vec<(String, &'static str, Rect)> = Vec::new();
            let mut switch = |ui: &mut egui::Ui, row: &str, what: &'static str, on: bool, icons: [&str; 2], hint: &str| -> bool {
                let tint = if on { theme.accent } else { muted };
                let button = ui.add(egui::Button::image(crate::icons::image(icons[usize::from(on)], 12.0, tint)).frame(false)).on_hover_text(hint);
                flips.push((row.to_string(), what, button.rect));
                button.clicked()
            };
            if let Some(mut row) = named(ui, NameRow::Shots, CLIP_HEIGHT, "Shots", false, 12.0) {
                row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let hint = if self.shots_locked { "The shots are locked: not moved or trimmed. Click to unlock" } else { "Lock the shots where they are" };
                    if switch(ui, "Shots", "lock", self.shots_locked, ["lock-open", "lock"], hint) {
                        self.shots_locked = !self.shots_locked;
                    }
                });
            }
            for listed_row in &listed {
                let Some((shot, _)) = open else { break };
                let Some(layer) = shot.layer(listed_row.layer()) else { continue };
                // A peg's or bones' track: named, in smaller letters, with no switches.
                match *listed_row {
                    BoardRow::Peg(peg, layer) => {
                        let name = shot.peg(peg).map_or("Peg", |peg| peg.name.as_str()).to_string();
                        named(ui, NameRow::Peg(layer), ROW_HEIGHT, &name, false, 10.0);
                        continue;
                    }
                    BoardRow::Bones(bones, layer) => {
                        let name = shot.deformer(bones).map_or("Bones", |bones| bones.name.as_str()).to_string();
                        named(ui, NameRow::Bones(layer), ROW_HEIGHT, &name, false, 10.0);
                        continue;
                    }
                    BoardRow::Layer(_) => {}
                }
                if let Some(mut row) = named(ui, NameRow::Layer(layer.id), ROW_HEIGHT, &layer.name, at.layer == Some(layer.id) && !at.camera, 11.0) {
                    row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let locked = self.locked.contains(&layer.id);
                        let hint = if locked { "Locked: not drawn on. Click to unlock" } else { "Lock the layer, so that it is not drawn on" };
                        if switch(ui, &layer.name, "lock", locked, ["lock-open", "lock"], hint) && !self.locked.remove(&layer.id) {
                            self.locked.insert(layer.id);
                        }
                        let hidden = self.hidden.contains(&layer.id);
                        let hint = if hidden { "Hidden in the viewer. Click to show it" } else { "Hide the layer in the viewer. It is still in the film" };
                        if switch(ui, &layer.name, "hide", hidden, ["eye", "eye-off"], hint) && !self.hidden.remove(&layer.id) {
                            self.hidden.insert(layer.id);
                        }
                    });
                }
            }
            // The camera: keyed on the playhead's frame from here, once it has the peg that moves it.
            if let Some((shot, clip)) = open
                && let Some(mut row) = named(ui, NameRow::Camera, ROW_HEIGHT, "Camera", at.camera, 11.0)
            {
                let frame = (clip.in_point + at.frame.saturating_sub(clip.start)).min(shot.duration.saturating_sub(1));
                row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if shot.camera.parent.is_none() {
                        let hint = "A peg above the camera, to move it with the Camera tool and to key where it is";
                        if ui.add(egui::Button::new(egui::RichText::new("Add Peg").size(10.0)).small()).on_hover_text(hint).clicked() {
                            switched.push(Action::Run("peg.new", json!({ "shot": shot.id.0, "aboveCamera": true })));
                        }
                        return;
                    }
                    let keyed = shot.camera_keys().contains(&frame);
                    let hint = if keyed {
                        "The camera has a keyframe on this frame. Click to remove it"
                    } else {
                        "Key where the camera is on this frame, and its zoom. Once keyed, moving the camera on another frame keys it there"
                    };
                    if ui.add(egui::Button::selectable(keyed, egui::RichText::new("Key").size(10.0))).on_hover_text(hint).clicked() {
                        switched.push(Action::Run("camera.setKey", json!({ "shot": shot.id.0, "frame": frame, "withPeg": true, "remove": keyed })));
                    }
                });
            }
            for (index, track) in board.sound.iter().enumerate() {
                if let Some(mut row) = named(ui, NameRow::Sound(index), SOUND_HEIGHT, &track.name, false, 11.0) {
                    row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let hint = if track.mute { "Muted: not heard. Click to hear it again" } else { "Mute the track" };
                        if ui.add(egui::Button::selectable(track.mute, egui::RichText::new("M").size(10.0))).on_hover_text(hint).clicked() {
                            switched.push(Action::Run("sound.setTrack", json!({ "storyboard": board.id.0, "track": index, "mute": !track.mute })));
                        }
                        let locked = self.sound_locked.contains(&index);
                        let hint = if locked { "Locked: its clips are not moved or trimmed. Click to unlock" } else { "Lock the track's clips where they are" };
                        if switch(ui, &track.name, "lock", locked, ["lock-open", "lock"], hint) && !self.sound_locked.remove(&index) {
                            self.sound_locked.insert(index);
                        }
                    });
                }
            }
            actions.extend(switched);
            #[cfg(test)]
            {
                self.switches = flips;
            }
            #[cfg(not(test))]
            let _ = flips;
        });
        column.response.rect
    }

    fn track(&mut self, ui: &mut egui::Ui, project: &Project, board: &Storyboard, at: Playhead, waves: Waves, actions: &mut Vec<Action>) {
        let theme = Theme::of(ui.ctx());
        // How many points a frame is wide: the zoom.
        let frame_width = self.frame_width.unwrap_or(FRAME_WIDTH);
        let snapping = self.bar_switches.snap;
        // What a dragged clip's ends are caught by: the ends of the others, and the playhead.
        let catches = |own: ShotId| -> Vec<i64> {
            let ends = board.clips().iter().filter(|other| other.shot != own).flat_map(|other| [i64::from(other.start), i64::from(other.end())]);
            ends.chain([0, i64::from(at.frame)]).collect()
        };
        let open = at.shot.and_then(|id| Some((project.shot(id)?, *board.clip(id)?)));
        let listed = open.map(|(shot, _)| board_rows(shot)).unwrap_or_default();
        // The open shot's layers, and under them its camera.
        let rows = listed.len() + usize::from(open.is_some());
        // Room after the last shot, or the last sound, to drag one into.
        let frames = board.end().max(board.sound_end()) + 240;
        let size = vec2(frame_width * frames as f32, RULER_HEIGHT + CLIP_HEIGHT + ROW_HEIGHT * rows as f32 + SOUND_HEIGHT * board.sound.len() as f32 + 4.0);
        let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let origin = response.rect.min;
        let column = |frame: i64| origin.x + frame_width * frame as f32;
        let frame_at = |x: f32| ((x - origin.x) / frame_width).floor().max(0.0) as u32;
        let rows_top = origin.y + RULER_HEIGHT + CLIP_HEIGHT;
        self.sound_drop = Some(crate::sound::SoundDrop {
            storyboard: board.id,
            // Down to the bottom of the panel, where a new track is.
            area: response.rect.with_max_y(response.rect.max.y.max(ui.clip_rect().max.y)).intersect(ui.clip_rect()),
            zero: origin.x,
            frame_width,
            first: 0,
            last: None,
            sound_top: rows_top + ROW_HEIGHT * rows as f32,
            row_height: SOUND_HEIGHT,
            tracks: (0..board.sound.len()).collect(),
            count: board.sound.len(),
        });
        #[cfg(test)]
        {
            self.origin = origin;
            self.clips.clear();
            self.clip_styles.clear();
            self.panel_marks.clear();
            self.length_field = None;
            self.transitions.clear();
        }

        // A drag that starts on the shot track, on the camera's row or on the sound tracks,
        // where there is nothing, pulls out a box that picks what it touches. One that starts
        // on the ruler or on a layer's row moves the playhead, as a click does anywhere.
        let camera_top = rows_top + ROW_HEIGHT * listed.len() as f32;
        let press = ui.input(|i| i.pointer.press_origin());
        let boxing = response.dragged() && press.is_some_and(|press| (press.y >= origin.y + RULER_HEIGHT && press.y < rows_top) || press.y >= camera_top);
        if let (true, Some(from), Some(to)) = (boxing, press, response.interact_pointer_pos()) {
            self.marquee = Some((from, to));
        }
        if response.clicked() {
            (self.picked_shots, self.picked_sounds, self.picked_keys) = (Vec::new(), Vec::new(), Vec::new());
        }
        // What is touched by the box, once it is let go.
        let pulled = self.marquee.filter(|_| !boxing).map(|(from, to)| Rect::from_two_pos(from, to));
        if pulled.is_some() {
            self.marquee = None;
            if !ui.input(|i| i.modifiers.shift) {
                (self.picked_shots, self.picked_sounds, self.picked_keys) = (Vec::new(), Vec::new(), Vec::new());
            }
        }
        let shift = ui.input(|i| i.modifiers.shift);
        // How far all that is picked is being dragged: by one of several picked, by its body.
        let several = self.picked_shots.len() + self.picked_sounds.len() > 1;
        let by_shot = self.drag.filter(|drag| several && drag.grip == Grip::Body && self.picked_shots.contains(&drag.shot)).map(|drag| drag.frames);
        let by_sound = self.sound_drag.filter(|drag| several && drag.grip == Grip::Body && self.picked_sounds.contains(&drag.clip)).map(|drag| drag.frames);
        let along = by_shot.or(by_sound);
        if let Some(pos) = response.interact_pointer_pos().filter(|_| response.clicked() || (response.dragged() && !boxing)) {
            // With snapping, a dragged playhead that comes within a few points of a shot's end,
            // a drawing, a keyframe of the camera or a marker goes to it. A click goes where
            // it is clicked.
            let under = frame_at(pos.x);
            let reach = (SNAP_REACH / frame_width) as u32;
            let to = if snapping && response.dragged() { crate::timeline::caught(under, playhead_catches(project, board, under, reach), reach) } else { under };
            actions.push(Action::Seek(to));
            let pressed = if response.clicked() { Some(pos) } else { ui.input(|i| i.pointer.press_origin()).filter(|_| response.drag_started()) };
            let row = pressed.map(|pos| ((pos.y - rows_top) / ROW_HEIGHT).floor()).filter(|row| *row >= 0.0);
            if let Some(listed_row) = row.and_then(|row| listed.get(row as usize)) {
                actions.push(Action::SelectLayer(listed_row.layer()));
            }
        }

        // The ruler: a tick and a number every second.
        let second = (project.settings.frame_rate.as_f64().round() as u32).max(1);
        // Zoomed far out, only every so many seconds have one, so that the numbers can be read.
        let every = (34.0 / (frame_width * second as f32)).ceil().max(1.0) as usize;
        for (count, frame) in (0..frames).step_by(second as usize).enumerate().filter(|(count, _)| count % every == 0) {
            painter.vline(column(i64::from(frame)), origin.y..=origin.y + RULER_HEIGHT, Stroke::new(1.0, theme.border));
            painter.text(pos2(column(i64::from(frame)) + 3.0, origin.y + 2.0), Align2::LEFT_TOP, format!("{count}s"), theme::mono(10.0), theme.text_muted);
        }
        painter.hline(response.rect.x_range(), origin.y + RULER_HEIGHT, Stroke::new(1.0, theme.border));
        // A thin line along the foot of the ruler over the frames of the shot in hand that are
        // kept ready on the card, where its clip has them, as the Animation timeline has it.
        #[cfg(test)]
        self.cache_marks.clear();
        let (cached_shot, cached) = &self.cached;
        for clip in board.clips().iter().filter(|clip| Some(clip.shot) == *cached_shot) {
            let mut kept = cached.iter().copied().filter(|frame| (clip.in_point..clip.out_point).contains(frame)).peekable();
            while let Some(first) = kept.next() {
                let mut after = first + 1;
                while kept.next_if_eq(&after).is_some() {
                    after += 1;
                }
                let (from, to) = (clip.start + first - clip.in_point, clip.start + after - clip.in_point);
                painter.hline(
                    column(i64::from(from))..=column(i64::from(to)),
                    origin.y + RULER_HEIGHT - 1.5,
                    Stroke::new(2.0, theme.colour(crate::theme::Role::Cached)),
                );
                #[cfg(test)]
                self.cache_marks.push((from, to));
            }
        }
        // The stretch that plays, if one is set: a bar along the top of the ruler with an end
        // at its in point and one at its out point, and what is outside it dimmed.
        #[cfg(test)]
        {
            self.range_ends = None;
            self.markers.clear();
        }
        if board.range.is_some() {
            let (first, after) = board.played();
            let (from, to) = (column(i64::from(first)), column(i64::from(after)));
            let whole = response.rect;
            for outside in [Rect::from_min_max(whole.min, pos2(from, whole.max.y)), Rect::from_min_max(pos2(to, whole.min.y), whole.max)] {
                painter.rect_filled(outside, 0.0, Color32::from_black_alpha(70));
            }
            painter.rect_filled(Rect::from_min_max(pos2(from, origin.y), pos2(to, origin.y + 3.0)), 0.0, theme.accent);
            for (x, inwards) in [(from, 5.0), (to, -5.0)] {
                painter.add(egui::Shape::convex_polygon(
                    vec![pos2(x, origin.y), pos2(x + inwards, origin.y), pos2(x, origin.y + 9.0)],
                    theme.accent,
                    Stroke::NONE,
                ));
                painter.vline(x, origin.y..=origin.y + RULER_HEIGHT, Stroke::new(1.0, theme.accent));
            }
            #[cfg(test)]
            {
                self.range_ends = Some((from, to));
            }
        }
        // The markers: each a flag of its colour on the ruler, with its name. A click goes to
        // it, a drag moves it, its menu names and colours it and takes it away.
        for marker in &board.markers {
            let dragged = self.marker_drag.filter(|drag| drag.0 == marker.frame).map_or(0, |drag| drag.1);
            let x = column((i64::from(marker.frame) + dragged).max(0)) + frame_width / 2.0;
            let flag = Rect::from_min_max(pos2(x - 4.0, origin.y), pos2(x + 5.0, origin.y + RULER_HEIGHT));
            #[cfg(test)]
            self.markers.push((marker.frame, flag));
            let [r, g, b] = marker.colour;
            let colour = Color32::from_rgb(r, g, b);
            painter.vline(x, origin.y..=rows_top, Stroke::new(1.0, colour));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    pos2(x - 4.0, origin.y + 1.0),
                    pos2(x + 5.0, origin.y + 1.0),
                    pos2(x + 5.0, origin.y + 8.0),
                    pos2(x, origin.y + 12.0),
                    pos2(x - 4.0, origin.y + 8.0),
                ],
                colour,
                Stroke::NONE,
            ));
            if !marker.name.is_empty() {
                painter.text(pos2(x + 8.0, origin.y + RULER_HEIGHT - 1.0), Align2::LEFT_BOTTOM, &marker.name, FontId::proportional(10.0), colour);
            }
            let held = ui.interact(flag, response.id.with(("marker", marker.frame)), Sense::click_and_drag());
            let said = if marker.name.is_empty() { "A marker".to_string() } else { marker.name.clone() };
            let held = held.on_hover_text(format!("{said}\nDrag to move it; right-click to name or colour it, or to remove it"));
            if held.clicked() {
                actions.push(Action::Seek(marker.frame));
            }
            if held.drag_started() {
                self.marker_drag = Some((marker.frame, 0));
            }
            if let Some(drag) = self.marker_drag.as_mut().filter(|drag| drag.0 == marker.frame) {
                if let (Some(now), Some(from)) = (held.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin())) {
                    drag.1 = ((now.x - from.x) / frame_width).round() as i64;
                }
                if held.drag_stopped() {
                    let to = (i64::from(marker.frame) + drag.1).max(0);
                    self.marker_drag = None;
                    if to != i64::from(marker.frame) {
                        actions.push(Action::Run("storyboard.setMarker", json!({ "storyboard": board.id.0, "frame": marker.frame, "to": to })));
                    }
                }
            }
            held.context_menu(|ui| {
                // Its name, written when the field is left.
                if self.marker_name.as_ref().is_none_or(|(frame, _)| *frame != marker.frame) {
                    self.marker_name = Some((marker.frame, marker.name.clone()));
                }
                if let Some((_, name)) = self.marker_name.as_mut() {
                    let field = ui.add(egui::TextEdit::singleline(name).hint_text("Name").desired_width(120.0));
                    if field.lost_focus() && *name != marker.name {
                        actions.push(Action::Run("storyboard.setMarker", json!({ "storyboard": board.id.0, "frame": marker.frame, "name": name })));
                    }
                }
                ui.separator();
                for (name, colour) in iskra_model::MARKER_COLOURS {
                    ui.horizontal(|ui| {
                        let (swatch, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                        ui.painter().rect_filled(swatch, 2.0, Color32::from_rgb(colour[0], colour[1], colour[2]));
                        if ui.radio(marker.colour == colour, name).clicked() {
                            actions.push(Action::Run("storyboard.setMarker", json!({ "storyboard": board.id.0, "frame": marker.frame, "colour": colour })));
                            ui.close();
                        }
                    });
                }
                ui.separator();
                if ui.button("Remove Marker").clicked() {
                    actions.push(Action::Run("storyboard.removeMarker", json!({ "storyboard": board.id.0, "frame": marker.frame })));
                    ui.close();
                }
            });
        }

        // The shots' retake notes, as far as they are in the cut: each a band at the ruler's foot
        // over its frames of the film; a click goes to it.
        #[cfg(test)]
        self.note_marks.clear();
        for clip in board.clips() {
            let Some(shot) = project.shot(clip.shot) else { continue };
            for note in &shot.notes {
                let (first, after) = (note.frame.max(clip.in_point), note.end().min(clip.out_point));
                if first >= after {
                    continue;
                }
                let film = |frame: u32| i64::from(clip.start + frame - clip.in_point);
                let band = Rect::from_min_max(
                    pos2(column(film(first)) + 1.0, origin.y + RULER_HEIGHT - 4.0),
                    pos2(column(film(after)) - 1.0, origin.y + RULER_HEIGHT - 1.0),
                );
                #[cfg(test)]
                self.note_marks.push((shot.id, note.id, band));
                if crate::notes::mark(ui, &painter, band, response.id.with(("note", shot.id, note.id)), note).clicked() {
                    actions.push(Action::Seek(clip.start + first - clip.in_point));
                }
            }
        }

        // Every second shot of the film is a little lighter, so that one shot is told from the
        // next at a glance: the second, the fourth and so on, as they lie in time.
        let mut in_time: Vec<(u32, ShotId)> = board.clips().iter().map(|clip| (clip.start, clip.shot)).collect();
        in_time.sort_unstable();
        let even: std::collections::HashSet<ShotId> = in_time.iter().skip(1).step_by(2).map(|(_, shot)| *shot).collect();
        #[cfg(test)]
        {
            self.lighter = in_time.iter().map(|(_, shot)| *shot).filter(|shot| even.contains(shot)).collect();
        }
        #[cfg(test)]
        self.clip_fills.clear();
        for clip in board.clips() {
            // While this clip is dragged it shows where letting go would put it.
            let is_picked = self.picked_shots.contains(&clip.shot);
            let by = self.drag.filter(|d| d.shot == clip.shot).map_or((Grip::Body, along.filter(|_| is_picked).unwrap_or(0)), |d| (d.grip, d.frames));
            let (mut start, mut end) = (i64::from(clip.start), i64::from(clip.end()));
            match by {
                (Grip::Body, frames) => (start, end) = ((start + frames).max(0), (start + frames).max(0) + end - start),
                (Grip::In, frames) => start = (start + frames).min(end - 1),
                (Grip::Out, frames) => end = (end + frames).max(start + 1),
            }
            let block = Rect::from_min_max(pos2(column(start), origin.y + RULER_HEIGHT), pos2(column(end), rows_top)).shrink(1.0);
            #[cfg(test)]
            self.clips.push((clip.shot, block));
            if pulled.is_some_and(|pulled| pulled.intersects(block)) && !is_picked {
                self.picked_shots.push(clip.shot);
            }
            let selected = at.shot == Some(clip.shot) || is_picked;
            let mut fill = theme.colour(if even.contains(&clip.shot) { theme::Role::ShotEven } else { theme::Role::ShotOdd });
            // Coloured by its scene or sequence, the clip is that colour half over its own,
            // still a little lighter every second shot.
            if let Some(colour) = project.shot(clip.shot).and_then(|shot| self.clip_colours.of_shot(shot)) {
                fill = theme::mix(fill, colour, 0.5);
                #[cfg(test)]
                self.clip_fills.push((clip.shot, colour));
            }
            painter.rect(block, 4.0, fill, Stroke::new(1.0, if selected { theme.accent } else { theme.border }), egui::StrokeKind::Inside);
            let shot = project.shot(clip.shot);
            let text = painter.with_clip_rect(block);
            let named =
                text.text(block.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, shot.map_or("?", |s| s.name.as_str()), FontId::proportional(12.0), theme.text);
            // The palette styles the shot is shown in, after its name.
            let styles: Vec<&str> =
                shot.into_iter().flat_map(|s| &s.styles).filter_map(|id| project.palette.style(*id)).map(|style| style.name.as_str()).collect();
            if !styles.is_empty() {
                let said = styles.join(", ");
                text.text(named.right_top() + vec2(6.0, 1.0), Align2::LEFT_TOP, &said, FontId::proportional(11.0), theme.text_muted);
                #[cfg(test)]
                self.clip_styles.push((clip.shot, said));
            }
            // The frames in the cut, and the handles outside it if there are any.
            let handles = shot.map_or(0, |s| s.duration.saturating_sub(clip.length()));
            let length = if handles > 0 { format!("{} + {handles}", clip.length()) } else { clip.length().to_string() };
            // A shot that came back shorter than its out point: the cut needs looking at.
            let short = shot.is_some_and(|s| clip.out_point > s.duration);
            let (length, color) = if short { (format!("{length}  ends early"), theme.danger) } else { (length, theme.text_muted) };
            text.text(block.left_bottom() + vec2(6.0, -4.0), Align2::LEFT_BOTTOM, length, theme::mono(10.0), color);
            // The shot's status, as a dot of its colour.
            if let Some(shot) = shot {
                text.circle_filled(block.right_top() + vec2(-9.0, 9.0), 4.0, status_color(ui.ctx(), shot.status));
            }

            let edge = EDGE_WIDTH.min(block.width() / 3.0);
            let grips = [
                (Grip::Body, block.shrink2(vec2(edge, 0.0)), CursorIcon::Grab),
                (Grip::In, Rect::from_min_max(block.min, pos2(block.min.x + edge, block.max.y)), CursorIcon::ResizeHorizontal),
                (Grip::Out, Rect::from_min_max(pos2(block.max.x - edge, block.min.y), block.max), CursorIcon::ResizeHorizontal),
            ];
            for (grip, rect, cursor) in grips {
                // Locked, the shots are clicked and opened, not dragged; with the razor, cut.
                let (sense, cursor) = match (self.shots_locked, self.bar_switches.razor) {
                    (true, _) => (Sense::click(), CursorIcon::Default),
                    (false, true) => (Sense::click(), CursorIcon::Crosshair),
                    (false, false) => (Sense::click_and_drag(), cursor),
                };
                let held = ui.interact(rect, response.id.with(("clip", clip.shot.0, grip as u8)), sense).on_hover_cursor(cursor);
                // A click on a clip puts the playhead where it was clicked, and picks the clip
                // alone; with Shift held it picks it beside the others, or lets it go. With the
                // razor, the click splits the shot there.
                if let (true, Some(pos)) = (self.bar_switches.razor && !self.shots_locked, held.interact_pointer_pos().filter(|_| held.clicked())) {
                    let frame = frame_at(pos.x).clamp(clip.start, clip.end() - 1);
                    actions.push(Action::Split(clip.shot, clip.in_point + frame - clip.start));
                } else if held.clicked() && shift {
                    match self.picked_shots.iter().position(|shot| *shot == clip.shot) {
                        Some(at) => drop(self.picked_shots.remove(at)),
                        None => self.picked_shots.push(clip.shot),
                    }
                } else if let Some(pos) = held.interact_pointer_pos().filter(|_| held.clicked()) {
                    (self.picked_shots, self.picked_sounds, self.picked_keys) = (vec![clip.shot], Vec::new(), Vec::new());
                    actions.push(Action::Seek(frame_at(pos.x).clamp(clip.start, clip.end() - 1)));
                }
                if held.double_clicked() {
                    actions.push(Action::OpenShot(clip.shot));
                }
                // Its menu (the user's of 2026-10-26): a new shot after it, a copy, a split at
                // the playhead, deleting it; and a transition on either end (plan §7.2b), across
                // the cut where another shot meets it, a fade where none does.
                let split_at =
                    (clip.start..clip.end()).contains(&at.frame).then(|| clip.in_point + at.frame - clip.start).filter(|frame| *frame > clip.in_point);
                held.context_menu(|ui| {
                    let hint = "A new shot right after this one, as long as it, with a layer to draw on. With Ripple the later shots make room";
                    if ui.button("Insert Shot").on_hover_text(hint).clicked() {
                        actions.push(Action::Run("shot.new", json!({ "after": clip.shot.0, "boards": true, "ripple": self.bar_switches.ripple })));
                        ui.close();
                    }
                    if ui.button("Duplicate").on_hover_text("A copy of the shot after it. With Ripple the later shots make room").clicked() {
                        actions.push(Action::Run("shot.duplicate", json!({ "shot": clip.shot.0, "ripple": self.bar_switches.ripple })));
                        ui.close();
                    }
                    let hint = "Split the shot in two at the playhead, which is to be over it";
                    if ui.add_enabled(split_at.is_some(), egui::Button::new("Split at the Playhead")).on_hover_text(hint).clicked()
                        && let Some(frame) = split_at
                    {
                        actions.push(Action::Split(clip.shot, frame));
                        ui.close();
                    }
                    if ui.button("Delete").on_hover_text("Delete the shot with its drawings").clicked() {
                        actions.push(Action::Run("shot.delete", json!({ "shot": clip.shot.0 })));
                        ui.close();
                    }
                    let several = self.picked_shots.len() > 1 && is_picked;
                    let hint = "Rename the shots picked together, as a text and a number in their order";
                    if several && ui.button("Rename Picked Shots…").on_hover_text(hint).clicked() {
                        actions.push(Action::RenameShots(self.picked_shots.clone()));
                        ui.close();
                    }
                    // The shots of its scene and of its sequence, picked on this storyboard.
                    let groups = shot.map(|shot| [(false, "Scene", shot.scene.clone()), (true, "Sequence", shot.sequence.clone())]);
                    let groups: Vec<(bool, &str, String)> = groups.into_iter().flatten().filter(|(_, _, name)| !name.is_empty()).collect();
                    if !groups.is_empty() {
                        ui.separator();
                    }
                    for (sequence, title, name) in groups {
                        let shots = crate::scenes::shots_of(project, board, sequence, &name);
                        if ui.button(format!("Pick the Shots of {title} {name}")).clicked() {
                            (self.picked_shots, self.picked_sounds, self.picked_keys) = (shots.clone(), Vec::new(), Vec::new());
                            ui.close();
                        }
                        let hint = "Each as a project file of its own, for someone to work on, as a shot is sent out";
                        if ui.button(format!("Send Out the Shots of {title} {name}…")).on_hover_text(hint).clicked() {
                            actions.push(Action::SendOut(shots));
                            ui.close();
                        }
                    }
                    ui.separator();
                    let half_second = (project.settings.frame_rate.as_f64() / 2.0).round().max(1.0) as u32;
                    for (name, title) in [("start", "Transition In"), ("end", "Transition Out")] {
                        ui.menu_button(title, |ui| {
                            for (kind, label) in TRANSITIONS {
                                if ui.button(label).clicked() {
                                    let params = json!({ "shot": clip.shot.0, "edge": name, "kind": kind, "length": half_second });
                                    actions.push(Action::Run("storyboard.setTransition", params));
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                if held.drag_started() {
                    self.drag = Some(ClipDrag { shot: clip.shot, grip, frames: 0 });
                    // One that is not picked is dragged alone.
                    if !is_picked {
                        (self.picked_shots, self.picked_sounds, self.picked_keys) = (vec![clip.shot], Vec::new(), Vec::new());
                    }
                }
                let Some(drag) = self.drag.as_mut().filter(|d| d.shot == clip.shot && d.grip == grip) else { continue };
                if let (Some(now), Some(from)) = (held.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin())) {
                    drag.frames = ((now.x - from.x) / frame_width).round() as i64;
                    // With snapping, an end that comes within a few points of another clip's
                    // end, or of the playhead, goes to it.
                    if snapping {
                        let (first, last) = (i64::from(clip.start) + drag.frames, i64::from(clip.end()) + drag.frames);
                        let moving: &[i64] = match grip {
                            Grip::Body => &[first, last],
                            Grip::In => &[first],
                            Grip::Out => &[last],
                        };
                        let nearest = catches(clip.shot).into_iter().flat_map(|to| moving.iter().map(move |end| to - end)).min_by_key(|by| by.abs());
                        if let Some(by) = nearest.filter(|by| (by.abs() as f32) * frame_width <= SNAP_REACH) {
                            drag.frames += by;
                        }
                    }
                }
                if held.drag_stopped() {
                    let frames = drag.frames;
                    self.drag = None;
                    let moved = |from: u32| (i64::from(from) + frames).max(0);
                    let shot = clip.shot.0;
                    let command = match grip {
                        _ if frames == 0 => continue,
                        Grip::Body if self.picked_shots.len() + self.picked_sounds.len() > 1 => ("edit.batch", self.move_picked(board, frames, None)),
                        // Let go a little way inside the shot beside it, the two are joined
                        // by a dissolve over the frames where they overlap.
                        Grip::Body if dissolve_into(board, clip, moved(clip.start)).is_some() => {
                            ("edit.batch", dissolve_into(board, clip, moved(clip.start)).unwrap_or_default())
                        }
                        Grip::Body => ("storyboard.moveClip", json!({ "shot": shot, "start": moved(clip.start), "ripple": self.bar_switches.ripple })),
                        Grip::In => ("storyboard.trimClip", json!({ "shot": shot, "in": moved(clip.in_point), "ripple": self.bar_switches.ripple })),
                        Grip::Out => ("storyboard.trimClip", json!({ "shot": shot, "out": moved(clip.out_point), "ripple": self.bar_switches.ripple })),
                    };
                    actions.push(Action::Run(command.0, command.1));
                }
            }
            // The panels after the shot's first, each a mark on the clip where it starts in
            // the film: dragged, the panel starts earlier or later; its menu removes it.
            let Some(shot) = shot else { continue };
            for panel in shot.panels().into_iter().filter(|panel| panel.frame > clip.in_point && panel.frame < clip.out_point) {
                let dragged = self.panel_drag.filter(|drag| drag.shot == shot.id && drag.frame == panel.frame).map_or(0, |drag| drag.frames);
                let x = column(start + i64::from(panel.frame - clip.in_point) + dragged);
                let mark = Rect::from_min_max(pos2(x - 3.0, block.top()), pos2(x + 3.0, block.bottom()));
                #[cfg(test)]
                self.panel_marks.push((shot.id, panel.frame, mark));
                let held = ui
                    .interact(mark, response.id.with(("panel", shot.id.0, panel.frame)), Sense::click_and_drag())
                    .on_hover_cursor(CursorIcon::ResizeHorizontal);
                let color = if held.hovered() || dragged != 0 { theme.accent } else { theme.text_muted };
                painter.vline(x, block.y_range(), Stroke::new(1.0, color));
                painter.add(egui::Shape::convex_polygon(
                    vec![pos2(x, block.top()), pos2(x + 6.0, block.top()), pos2(x, block.top() + 6.0)],
                    color,
                    Stroke::NONE,
                ));
                let said = if panel.action.is_empty() { "A panel starts here".to_string() } else { panel.action.clone() };
                let held = held.on_hover_text(format!("{said}\nDrag to make it start earlier or later; right-click to remove it"));
                if held.clicked() {
                    actions.push(Action::Seek(clip.start + panel.frame - clip.in_point));
                }
                if held.drag_started() {
                    self.panel_drag = Some(PanelDrag { shot: shot.id, frame: panel.frame, frames: 0 });
                }
                if let Some(drag) = self.panel_drag.as_mut().filter(|drag| drag.shot == shot.id && drag.frame == panel.frame) {
                    if let (Some(now), Some(from)) = (held.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin())) {
                        // It stays between the panels beside it.
                        let (before, after) = (shot.panel_at(panel.frame - 1).frame, shot.panel_end(panel.frame));
                        let reach = (i64::from(before) + 1 - i64::from(panel.frame), i64::from(after) - 1 - i64::from(panel.frame));
                        drag.frames = (((now.x - from.x) / frame_width).round() as i64).clamp(reach.0, reach.1);
                    }
                    if held.drag_stopped() {
                        let to = i64::from(panel.frame) + drag.frames;
                        self.panel_drag = None;
                        if to != i64::from(panel.frame) {
                            actions.push(Action::Run("panel.move", json!({ "shot": shot.id.0, "frame": panel.frame, "to": to })));
                        }
                    }
                }
                held.context_menu(|ui| {
                    if ui.button("Remove Panel").on_hover_text("With what is written of it. Nothing drawn changes").clicked() {
                        actions.push(Action::Run("panel.remove", json!({ "shot": shot.id.0, "frame": panel.frame })));
                        ui.close();
                    }
                });
            }
        }

        // The transitions (plan §7.2b), each a band along the foot of the shot track over the
        // frames it mixes: across its cut, or inside the clip on a free edge. Dragged, it is
        // longer or shorter; its menu has its kind and takes it away.
        for lying in board.lying() {
            let (shot, edge) = (lying.transition.shot, lying.transition.edge);
            let across = lying.outgoing.is_some() && lying.incoming.is_some();
            let dragged = self.transition_drag.filter(|drag| (drag.0, drag.1) == (shot, edge)).map_or(0, |drag| drag.2);
            // One across a cut grows to both sides; a fade from the clip's edge inwards.
            let (mut from, mut to) = (i64::from(lying.start), i64::from(lying.end));
            match (across, lying.incoming.is_some()) {
                (true, _) => (from, to) = (from - dragged, to + dragged),
                (false, true) => to += dragged,
                (false, false) => from -= dragged,
            }
            let (from, to) = (from.max(0), to.max(from.max(0) + 1));
            let band = Rect::from_min_max(pos2(column(from), rows_top - 13.0), pos2(column(to), rows_top - 1.0));
            #[cfg(test)]
            self.transitions.push((shot, edge, band));
            let held =
                ui.interact(band, response.id.with(("transition", shot.0, edge as u8)), Sense::click_and_drag()).on_hover_cursor(CursorIcon::ResizeHorizontal);
            let color = if held.hovered() || dragged != 0 { theme.accent } else { theme.accent.gamma_multiply(0.6) };
            painter.rect_filled(band, 2.0, color.gamma_multiply(0.45));
            // The line of how much of the incoming shot there is: rising, or for a fade out falling.
            let (low, high) = if lying.incoming.is_some() { (band.left_bottom(), band.right_top()) } else { (band.left_top(), band.right_bottom()) };
            painter.line_segment([low, high], Stroke::new(1.5, color));
            let name = TRANSITIONS
                .iter()
                .find(|(kind, _)| serde_json::to_value(lying.transition.kind).ok().as_ref().and_then(|value| value.as_str()) == Some(kind))
                .map_or("", |(_, label)| label);
            let frames = lying.end - lying.start;
            let held = held.on_hover_text(format!("{name}, {frames} frames\nDrag to make it longer or shorter; right-click for its kind, or to remove it"));
            if held.drag_started() {
                self.transition_drag = Some((shot, edge, 0));
            }
            let edge_name = if edge == iskra_model::Edge::Start { "start" } else { "end" };
            if let Some(drag) = self.transition_drag.as_mut().filter(|drag| (drag.0, drag.1) == (shot, edge)) {
                if let (Some(now), Some(press)) = (held.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin())) {
                    // The way it grows is longer: away from the cut it lies across, or from
                    // the clip's edge inwards.
                    let grows_right = if across { press.x >= band.center().x } else { lying.incoming.is_some() };
                    let outwards = if grows_right { now.x - press.x } else { press.x - now.x };
                    drag.2 = (outwards / frame_width).round() as i64;
                }
                if held.drag_stopped() {
                    let by = if across { 2 * drag.2 } else { drag.2 };
                    let length = (i64::from(lying.transition.length) + by).max(1);
                    self.transition_drag = None;
                    if length != i64::from(lying.transition.length) {
                        let kind = serde_json::to_value(lying.transition.kind).unwrap_or_default();
                        actions.push(Action::Run("storyboard.setTransition", json!({ "shot": shot.0, "edge": edge_name, "kind": kind, "length": length })));
                    }
                }
            }
            // Closed by a click outside it, not inside: its length is typed in it.
            let menu = egui::Popup::context_menu(&held).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            menu.show(|ui| {
                for (kind, label) in TRANSITIONS {
                    let is = serde_json::to_value(lying.transition.kind).ok().as_ref().and_then(|value| value.as_str()) == Some(kind);
                    if ui.radio(is, label).clicked() {
                        let params = json!({ "shot": shot.0, "edge": edge_name, "kind": kind, "length": lying.transition.length });
                        actions.push(Action::Run("storyboard.setTransition", params));
                        ui.close();
                    }
                }
                ui.separator();
                // Its length, typed as a number of frames.
                ui.horizontal(|ui| {
                    ui.label("Length");
                    let mut length = lying.transition.length;
                    let typed = ui.add(egui::DragValue::new(&mut length).range(1..=9999).speed(0.0).suffix(" frames"));
                    #[cfg(test)]
                    {
                        self.length_field = Some(typed.rect);
                    }
                    if typed.changed() && length != lying.transition.length {
                        let kind = serde_json::to_value(lying.transition.kind).unwrap_or_default();
                        actions.push(Action::Run("storyboard.setTransition", json!({ "shot": shot.0, "edge": edge_name, "kind": kind, "length": length })));
                    }
                });
                if ui.button("Remove Transition").clicked() {
                    actions.push(Action::Run("storyboard.removeTransition", json!({ "shot": shot.0, "edge": edge_name })));
                    ui.close();
                }
            });
        }

        // The shot under the playhead opens up: a row for each of its layers, with its
        // drawings as blocks where they show in the film.
        #[cfg(test)]
        self.hold_handles.clear();
        if let Some((_, clip)) = open {
            let film = |frame: u32| i64::from(clip.start) + i64::from(frame) - i64::from(clip.in_point);
            // As on the Animation timeline: a dragged hold moves the later drawings with Ripple
            // on, and leaves them where they are with it off; Alt held does the other.
            let ripple = self.bar_switches.ripple != ui.input(|i| i.modifiers.alt);
            for (row, listed_row) in listed.iter().enumerate() {
                let (BoardRow::Layer(layer), Some((shot, _))) = (*listed_row, open) else { continue };
                let Some(layer) = shot.layer(layer) else { continue };
                let top = rows_top + ROW_HEIGHT * row as f32;
                let lane = Rect::from_min_max(pos2(column(i64::from(clip.start)), top), pos2(column(i64::from(clip.end())), top + ROW_HEIGHT));
                if at.layer == Some(layer.id) {
                    painter.rect_filled(lane, 0.0, theme.selection.gamma_multiply(0.5));
                }
                painter.hline(lane.x_range(), lane.bottom(), Stroke::new(1.0, theme.border));
                // While a hold of this row is dragged, the row shows what letting go would give.
                let mut exposure = layer.exposure.clone();
                if let Some((_, start, len)) = self.hold_drag.filter(|(of, ..)| *of == layer.id) {
                    exposure.set_hold(start, len, ripple);
                }
                // Only the part of each hold that is in the cut.
                for run in exposure.runs().iter().filter(|r| r.start < clip.out_point && r.end() > clip.in_point) {
                    let (from, to) = (film(run.start.max(clip.in_point)), film(run.end().min(clip.out_point)));
                    let block = Rect::from_min_max(pos2(column(from), lane.top()), pos2(column(to), lane.bottom())).shrink(1.5);
                    painter.rect_filled(block, 2.0, theme.surface_raised);
                    let name = layer.drawing(run.drawing).map_or("?", |d| d.name.as_str());
                    painter.with_clip_rect(block).text(block.left_center() + vec2(3.0, 0.0), Align2::LEFT_CENTER, name, FontId::proportional(10.0), theme.text);
                    // A hold that ends in the cut has a handle on its end: dragged, it holds the
                    // drawing longer or shorter.
                    let dragged = self.hold_drag.is_some_and(|(of, start, _)| of == layer.id && start == run.start);
                    if run.end() > clip.out_point && !dragged {
                        continue;
                    }
                    let handle = Rect::from_min_max(pos2(block.right() - 5.0, block.top()), block.max);
                    #[cfg(test)]
                    self.hold_handles.push((layer.id, run.start, handle));
                    let grip = ui
                        .interact(handle, response.id.with(("board hold", layer.id.0, run.start)), Sense::drag())
                        .on_hover_cursor(CursorIcon::ResizeHorizontal)
                        .on_hover_text("Drag to hold the drawing longer or shorter. With Ripple the drawings after it move along, without it they stay where they are; Alt does the other");
                    if grip.drag_started() {
                        self.hold_drag = Some((layer.id, run.start, run.len));
                    }
                    let Some(drag) = self.hold_drag.as_mut().filter(|(of, start, _)| *of == layer.id && *start == run.start) else {
                        if grip.hovered() {
                            painter.rect_filled(handle, 2.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        continue;
                    };
                    painter.rect_stroke(block, 2.0, Stroke::new(1.5, theme.accent), egui::StrokeKind::Inside);
                    if let (Some(pos), Some(press)) = (grip.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin())) {
                        // As many frames longer or shorter as the pointer has gone: frames may be
                        // narrower here than the handle, so where it was grabbed doesn't count.
                        let by = ((pos.x - press.x) / frame_width).round() as i64;
                        let held = layer.exposure.run_at(run.start).map_or(run.len, |held| held.len);
                        drag.2 = (i64::from(held) + by).max(1) as u32;
                    }
                    if grip.drag_stopped() {
                        let (start, len) = (drag.1, drag.2);
                        self.hold_drag = None;
                        if layer.exposure.run_at(start).is_some_and(|before| before.len != len) {
                            actions.push(Action::Run("exposure.setHold", json!({ "layer": layer.id.0, "frame": start, "length": len, "ripple": ripple })));
                        }
                    }
                }
            }
        }

        // The keyframes of the open shot's boards, each on its board's row, of the peg above
        // it; and under the boards its camera's. At film time: a click goes to one, a drag
        // moves it to another frame, and its menu eases or removes it.
        #[cfg(test)]
        {
            self.camera_keys.clear();
            self.repeated_marks.clear();
            self.camera_tweens.clear();
            self.breakdown_lines.clear();
            self.board_keys.clear();
            self.bone_keys.clear();
        }
        if let Some((shot, clip)) = open {
            let keys = KeyRow { shot, clip, column: &column, frame_width, pulled, shift, id: response.id };
            for (row, listed_row) in listed.iter().enumerate() {
                let owner = match *listed_row {
                    BoardRow::Peg(peg, _) => KeyOwner::Peg(peg),
                    BoardRow::Bones(bones, _) => KeyOwner::Bones(bones),
                    BoardRow::Layer(_) => continue,
                };
                let top = rows_top + ROW_HEIGHT * row as f32;
                let lane = Rect::from_min_max(pos2(column(i64::from(clip.start)), top), pos2(column(i64::from(clip.end())), top + ROW_HEIGHT));
                painter.rect_filled(lane, 0.0, theme.surface_raised.gamma_multiply(0.3));
                painter.hline(lane.x_range(), lane.bottom(), Stroke::new(1.0, theme.border));
                self.key_row(ui, &painter, &keys, owner, lane, actions);
            }
            let top = rows_top + ROW_HEIGHT * listed.len() as f32;
            let lane = Rect::from_min_max(pos2(column(i64::from(clip.start)), top), pos2(column(i64::from(clip.end())), top + ROW_HEIGHT));
            painter.rect_filled(lane, 0.0, theme.surface_raised.gamma_multiply(0.5));
            painter.hline(lane.x_range(), lane.bottom(), Stroke::new(1.0, theme.border));
            self.key_row(ui, &painter, &keys, KeyOwner::Camera, lane, actions);
        }

        // The sound tracks, under the shots and the open shot's layers: each clip a block with
        // its waveform, dragged to move it, by an end to trim it.
        let sound_top = rows_top + ROW_HEIGHT * rows as f32;
        #[cfg(test)]
        self.sound_clips.clear();
        for (index, track) in board.sound.iter().enumerate() {
            let top = sound_top + SOUND_HEIGHT * index as f32;
            let lane = Rect::from_min_max(pos2(origin.x, top), pos2(origin.x + size.x, top + SOUND_HEIGHT));
            painter.hline(lane.x_range(), lane.bottom(), Stroke::new(1.0, theme.border));
            let on_lane = ui.interact(lane, response.id.with(("sound track", index)), Sense::click());
            if let Some(pos) = on_lane.interact_pointer_pos().filter(|_| on_lane.clicked()) {
                actions.push(Action::Seek(frame_at(pos.x)));
            }
            on_lane.context_menu(|ui| {
                if ui.button(if track.mute { "Unmute the Track" } else { "Mute the Track" }).clicked() {
                    actions.push(Action::Run("sound.setTrack", json!({ "storyboard": board.id.0, "track": index, "mute": !track.mute })));
                    ui.close();
                }
                if ui.button("Remove the Track").on_hover_text("With the clips on it. The sound items stay in the project").clicked() {
                    actions.push(Action::Run("sound.setTrack", json!({ "storyboard": board.id.0, "track": index, "remove": true })));
                    ui.close();
                }
            });
            for clip in &track.clips {
                // While this clip is dragged it shows where letting go would put it.
                let dragged = self.sound_drag.filter(|drag| drag.clip == clip.id);
                let is_picked = self.picked_sounds.contains(&clip.id);
                let (mut start, mut end, mut offset) = (i64::from(clip.start), i64::from(clip.end()), i64::from(clip.offset));
                let whole = project.media(clip.media).map_or(i64::from(clip.length), |media| i64::from(media.kind.length(project.settings.frame_rate)));
                match dragged.map_or((Grip::Body, along.filter(|_| is_picked).unwrap_or(0)), |drag| (drag.grip, drag.frames)) {
                    (Grip::Body, frames) => (start, end) = ((start + frames).max(0), (start + frames).max(0) + end - start),
                    // An end is dragged no further than the sound goes.
                    (Grip::In, frames) => {
                        let by = frames.clamp(-offset.min(start), end - start - 1);
                        (start, offset) = (start + by, offset + by);
                    }
                    (Grip::Out, frames) => end = (end + frames).clamp(start + 1, start + whole - offset),
                }
                let row = dragged.filter(|drag| drag.grip == Grip::Body).map_or(index, |drag| drag.track);
                let block_top = sound_top + SOUND_HEIGHT * row as f32;
                let block = Rect::from_min_max(pos2(column(start), block_top), pos2(column(end), block_top + SOUND_HEIGHT)).shrink(1.5);
                #[cfg(test)]
                self.sound_clips.push((clip.id, block));
                if pulled.is_some_and(|pulled| pulled.intersects(block)) && !is_picked {
                    self.picked_sounds.push(clip.id);
                }
                let fill = if track.mute { theme.surface_raised } else { theme.selection };
                painter.rect(
                    block,
                    3.0,
                    fill,
                    Stroke::new(1.0, if dragged.is_some() || is_picked { theme.accent } else { theme.border }),
                    egui::StrokeKind::Inside,
                );
                // The waveform: a line a frame, from the lowest to the highest value in it.
                let inside = painter.with_clip_rect(block);
                if let Some(peaks) = waves(clip.media) {
                    let (middle, half) = (block.center().y, block.height() / 2.0 - 2.0);
                    for frame in 0..(end - start).max(0) {
                        let Some([low, high]) = peaks.get((offset + frame) as usize) else { break };
                        let x = column(start + frame) + frame_width / 2.0;
                        let (from, to) =
                            (middle - high.clamp(-1.0, 1.0) * half * clip.gain.min(1.0), middle - low.clamp(-1.0, 1.0) * half * clip.gain.min(1.0));
                        inside.vline(x, from.min(to - 1.0)..=to, Stroke::new(frame_width - 1.0, theme.text_muted));
                    }
                }
                let name = project.media(clip.media).map_or("?", |media| media.name.as_str());
                inside.text(block.left_top() + vec2(4.0, 1.0), Align2::LEFT_TOP, name, FontId::proportional(10.0), theme.text);

                let edge = EDGE_WIDTH.min(block.width() / 3.0);
                let grips = [
                    (Grip::Body, block.shrink2(vec2(edge, 0.0)), CursorIcon::Grab),
                    (Grip::In, Rect::from_min_max(block.min, pos2(block.min.x + edge, block.max.y)), CursorIcon::ResizeHorizontal),
                    (Grip::Out, Rect::from_min_max(pos2(block.max.x - edge, block.min.y), block.max), CursorIcon::ResizeHorizontal),
                ];
                for (grip, rect, cursor) in grips {
                    let held = ui.interact(rect, response.id.with(("sound", clip.id.0, grip as u8)), Sense::click_and_drag()).on_hover_cursor(cursor);
                    if held.clicked() && shift {
                        match self.picked_sounds.iter().position(|picked| *picked == clip.id) {
                            Some(at) => drop(self.picked_sounds.remove(at)),
                            None => self.picked_sounds.push(clip.id),
                        }
                    } else if let Some(pos) = held.interact_pointer_pos().filter(|_| held.clicked()) {
                        (self.picked_shots, self.picked_sounds, self.picked_keys) = (Vec::new(), vec![clip.id], Vec::new());
                        actions.push(Action::Seek(frame_at(pos.x)));
                    }
                    held.context_menu(|ui| {
                        // How loud the clip is: 100% is as it was recorded.
                        ui.horizontal(|ui| {
                            ui.weak("Volume");
                            if let Some(gain) = crate::sound::volume_slider(ui, &mut self.volume_edit, clip.id.0, clip.gain) {
                                actions.push(Action::Run("sound.setClip", json!({ "clip": clip.id.0, "gain": gain })));
                            }
                        });
                        if ui.button("Delete").on_hover_text("Takes the clip off the track. The sound item stays in the project").clicked() {
                            actions.push(Action::Run("sound.delete", json!({ "clip": clip.id.0 })));
                            ui.close();
                        }
                    });
                    if held.drag_started() && !self.sound_locked.contains(&index) {
                        self.sound_drag = Some(SoundDrag { clip: clip.id, grip, frames: 0, track: index });
                        if !is_picked {
                            (self.picked_shots, self.picked_sounds, self.picked_keys) = (Vec::new(), vec![clip.id], Vec::new());
                        }
                    }
                    let Some(drag) = self.sound_drag.as_mut().filter(|drag| drag.clip == clip.id && drag.grip == grip) else { continue };
                    if let (Some(now), Some(from)) = (held.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin())) {
                        drag.frames = ((now.x - from.x) / frame_width).round() as i64;
                        // One row past the last makes a new track.
                        drag.track = (((now.y - sound_top) / SOUND_HEIGHT).floor().max(0.0) as usize).min(board.sound.len());
                    }
                    if held.drag_stopped() {
                        let moved = (start != i64::from(clip.start), end != i64::from(clip.end()), row != index);
                        let frames = self.sound_drag.map_or(0, |drag| drag.frames);
                        self.sound_drag = None;
                        // One of several that are picked, dragged by its body, takes them all.
                        if grip == Grip::Body && self.picked_shots.len() + self.picked_sounds.len() > 1 {
                            if frames != 0 || row != index {
                                actions.push(Action::Run("edit.batch", self.move_picked(board, frames, Some((clip.id, row)))));
                            }
                            continue;
                        }
                        let params = match (grip, moved) {
                            (_, (false, false, false)) => continue,
                            (Grip::Body, _) => json!({ "clip": clip.id.0, "start": start, "track": row }),
                            (Grip::In, _) => json!({ "clip": clip.id.0, "start": start, "offset": offset, "length": end - start }),
                            (Grip::Out, _) => json!({ "clip": clip.id.0, "length": end - start }),
                        };
                        actions.push(Action::Run("sound.setClip", params));
                    }
                }
            }
        }

        // The box that picks, while it is pulled out.
        if let Some((from, to)) = self.marquee {
            painter.rect(Rect::from_two_pos(from, to), 2.0, theme.accent.gamma_multiply(0.12), Stroke::new(1.0, theme.accent), egui::StrokeKind::Inside);
        }

        // The playhead, over everything.
        let x = column(i64::from(at.frame));
        painter.vline(x, origin.y..=origin.y + size.y, Stroke::new(1.5, theme.accent));
        painter.add(egui::Shape::convex_polygon(vec![pos2(x - 5.0, origin.y), pos2(x + 5.0, origin.y), pos2(x, origin.y + 8.0)], theme.accent, Stroke::NONE));
    }
}

/// Whose keyframes a row of the Storyboard timeline shows: the open shot's camera, with its
/// peg's, or the peg above a board, on the board's own row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyOwner {
    Camera,
    Peg(PegId),
    Bones(iskra_model::DeformerId),
}

impl KeyOwner {
    /// The frames it has keys on.
    fn frames(self, shot: &Shot) -> Vec<u32> {
        match self {
            KeyOwner::Camera => shot.camera_keys().into_iter().collect(),
            KeyOwner::Peg(peg) => shot.peg(peg).map_or(Vec::new(), |peg| peg.animation.frames(None).into_iter().collect()),
            KeyOwner::Bones(bones) => shot.deformer(bones).map_or(Vec::new(), |bones| bones.key_frames().into_iter().collect()),
        }
    }

    fn moves_after(self, shot: &Shot, frame: u32) -> bool {
        match self {
            KeyOwner::Camera => shot.camera_moves_after(frame),
            KeyOwner::Peg(peg) => shot.peg(peg).is_some_and(|peg| peg.animation.moves_after_of(frame, None)),
            KeyOwner::Bones(bones) => shot.deformer(bones).is_some_and(|bones| bones.moves_after_of(frame, None)),
        }
    }

    fn interp(self, shot: &Shot, frame: u32) -> Option<(Interp, Interp)> {
        match self {
            KeyOwner::Camera => shot.camera_interp(frame),
            KeyOwner::Peg(peg) => shot.peg(peg)?.animation.interp_of(frame, None),
            KeyOwner::Bones(bones) => shot.deformer(bones)?.interp_of(frame, None),
        }
    }

    /// One of its key commands, `moveKey` or `setEase`, with what it takes with `more`.
    fn command(self, shot: ShotId, what: &str, mut more: Value) -> (&'static str, Value) {
        let id = match (self, what) {
            (KeyOwner::Camera, "moveKey") => "camera.moveKey",
            (KeyOwner::Camera, _) => "camera.setEase",
            (KeyOwner::Peg(_), "moveKey") => "peg.moveKey",
            (KeyOwner::Peg(_), _) => "peg.setEase",
            (KeyOwner::Bones(_), "moveKey") => "deformer.moveKey",
            (KeyOwner::Bones(_), _) => "deformer.setEase",
        };
        match self {
            KeyOwner::Camera => more["shot"] = json!(shot.0),
            KeyOwner::Peg(peg) => more["peg"] = json!(peg.0),
            KeyOwner::Bones(bones) => more["deformer"] = json!(bones.0),
        }
        (id, more)
    }

    /// The command that takes its key on `frame` away: the camera's with its peg's.
    pub(crate) fn removal(self, shot: ShotId, frame: u32) -> (&'static str, Value) {
        match self {
            KeyOwner::Camera => ("camera.setKey", json!({ "shot": shot.0, "frame": frame, "withPeg": true, "remove": true })),
            KeyOwner::Peg(peg) => ("peg.removeKey", json!({ "peg": peg.0, "frame": frame })),
            KeyOwner::Bones(bones) => ("deformer.removeKey", json!({ "deformer": bones.0, "frame": frame })),
        }
    }

    /// Whose keys it is, as `keys.chart` names it.
    fn chart_owner(self, shot: ShotId) -> iskra_model::KeyOwner {
        match self {
            KeyOwner::Camera => iskra_model::KeyOwner::Camera(shot),
            KeyOwner::Peg(peg) => iskra_model::KeyOwner::Peg(peg),
            KeyOwner::Bones(bones) => iskra_model::KeyOwner::Deformer(bones),
        }
    }

    /// The commands, for `edit.batch`, that time its way from its key on `from` to the next
    /// by `spacing`; empty takes the chart away. None where it has no later key.
    pub(crate) fn charted(self, shot: &Shot, from: u32, spacing: &[f64]) -> Vec<Value> {
        let Some(to) = self.frames(shot).into_iter().find(|frame| *frame > from) else { return Vec::new() };
        vec![json!({ "id": "keys.chart", "params": { "shot": shot.id.0, "owner": self.chart_owner(shot.id), "from": from, "to": to, "spacing": spacing } })]
    }

    /// Whose a key is, as `keys.copy` takes it.
    pub(crate) fn owner(self, shot: ShotId) -> Value {
        match self {
            KeyOwner::Camera => json!({ "camera": shot.0 }),
            KeyOwner::Peg(peg) => json!({ "peg": peg.0 }),
            KeyOwner::Bones(bones) => json!({ "deformer": bones.0 }),
        }
    }
}

/// What every row of keys of the open shot is drawn with.
struct KeyRow<'a> {
    shot: &'a Shot,
    clip: iskra_model::Clip,
    column: &'a dyn Fn(i64) -> f32,
    frame_width: f32,
    /// The box let go of this frame, which picks the keys it touches.
    pulled: Option<Rect>,
    shift: bool,
    id: egui::Id,
}

impl StoryboardView {
    /// A row of keyframes, of `owner`, in `lane`: a mark for each in the cut, as the Animation
    /// timeline draws them, and a line between two where it moves on.
    fn key_row(&mut self, ui: &egui::Ui, painter: &egui::Painter, row: &KeyRow, owner: KeyOwner, lane: Rect, actions: &mut Vec<Action>) {
        let KeyRow { shot, clip, column, frame_width, pulled, shift, id } = *row;
        let theme = Theme::of(ui.ctx());
        let film = |frame: u32| i64::from(clip.start) + i64::from(frame) - i64::from(clip.in_point);
        // Where each key is shown, with whether it moves on from it, and its frame.
        let mut placed: Vec<(u32, bool, u32)> = Vec::new();
        // Only the keys in the cut, as for the drawings.
        for frame in owner.frames(shot).into_iter().filter(|frame| (clip.in_point..clip.out_point).contains(frame)) {
            let dragged = self.key_drag.filter(|drag| drag.0 == shot.id && drag.1 == owner && drag.2 == frame);
            let is_picked = self.picked_keys.contains(&(owner, frame));
            let within = |frame: i64| frame.clamp(i64::from(clip.in_point), i64::from(clip.out_point) - 1) as u32;
            // While it is dragged, the key shows on the frame it would land on; and so do
            // the others that are picked with the one that is dragged.
            let with = self
                .key_drag
                .filter(|drag| is_picked && drag.0 == shot.id && self.picked_keys.contains(&(drag.1, drag.2)))
                .map(|drag| i64::from(drag.3) - i64::from(drag.2));
            let shown = dragged.map_or(with.map_or(frame, |by| within(i64::from(frame) + by)), |drag| drag.3);
            placed.push((shown, owner.moves_after(shot, frame), frame));
            let center = pos2(column(film(shown)) + frame_width / 2.0, lane.center().y);
            let mark = Rect::from_center_size(pos2(column(film(frame)) + frame_width / 2.0, lane.center().y), vec2(11.0, ROW_HEIGHT));
            #[cfg(test)]
            match owner {
                KeyOwner::Camera => self.camera_keys.push((frame, mark)),
                KeyOwner::Peg(peg) => self.board_keys.push((peg, frame, mark)),
                KeyOwner::Bones(bones) => self.bone_keys.push((bones, frame, mark)),
            }
            if pulled.is_some_and(|pulled| pulled.intersects(mark)) && !is_picked {
                self.picked_keys.push((owner, frame));
            }
            let grip = ui.interact(mark, id.with(("key", format!("{owner:?}"), shot.id.0, frame)), Sense::click_and_drag()).on_hover_cursor(CursorIcon::Grab);
            if grip.clicked() && shift {
                match self.picked_keys.iter().position(|key| *key == (owner, frame)) {
                    Some(at) => drop(self.picked_keys.remove(at)),
                    None => self.picked_keys.push((owner, frame)),
                }
            } else if grip.clicked() {
                (self.picked_shots, self.picked_sounds, self.picked_keys) = (Vec::new(), Vec::new(), vec![(owner, frame)]);
                actions.push(Action::Seek(film(frame).max(0) as u32));
            }
            if grip.drag_started() {
                self.key_drag = Some((shot.id, owner, frame, frame));
                if !is_picked {
                    (self.picked_shots, self.picked_sounds, self.picked_keys) = (Vec::new(), Vec::new(), vec![(owner, frame)]);
                }
            }
            if let (Some(drag), Some(now), Some(from)) =
                (self.key_drag.as_mut().filter(|_| dragged.is_some()), grip.interact_pointer_pos(), ui.input(|i| i.pointer.press_origin()))
            {
                let by = ((now.x - from.x) / frame_width).round() as i64;
                drag.3 = within(i64::from(frame) + by);
            }
            if let (Some(drag), true) = (self.key_drag.filter(|_| dragged.is_some()), grip.drag_stopped()) {
                self.key_drag = None;
                let by = i64::from(drag.3) - i64::from(frame);
                if by != 0 && self.picked_keys.len() > 1 {
                    // All that are picked go by as many frames, as one step: those ahead
                    // first, so that none lands on one that has yet to move.
                    let mut keys = self.picked_keys.clone();
                    keys.sort_unstable_by_key(|(_, frame)| *frame);
                    if by > 0 {
                        keys.reverse();
                    }
                    let moves: Vec<Value> = (keys.iter())
                        .map(|(of, key)| of.command(shot.id, "moveKey", json!({ "from": key, "to": within(i64::from(*key) + by) })))
                        .map(|(id, params)| json!({ "id": id, "params": params }))
                        .collect();
                    actions.push(Action::Run("edit.batch", json!({ "commands": moves })));
                    self.picked_keys = keys.into_iter().map(|(of, key)| (of, within(i64::from(key) + by))).collect();
                } else if by != 0 {
                    let (id, params) = owner.command(shot.id, "moveKey", json!({ "from": frame, "to": drag.3 }));
                    actions.push(Action::Run(id, params));
                    self.picked_keys = vec![(owner, drag.3)];
                }
            }
            let (into, out) = owner.interp(shot, frame).unwrap_or_default();
            grip.context_menu(|ui| {
                let eases = [
                    ("Linear", Some(Interp::Linear), Interp::Linear),
                    ("Slow In", Some(Interp::Ease), Interp::Linear),
                    ("Slow Out", Some(Interp::Linear), Interp::Ease),
                    ("Slow In and Out", Some(Interp::Ease), Interp::Ease),
                    ("Hold", None, Interp::Hold),
                ];
                for (name, to_into, to_out) in eases {
                    let is = to_out == out && to_into.is_none_or(|to_into| to_into == into);
                    if ui.radio(is, name).clicked() {
                        let mut params = json!({ "frame": frame, "out": to_out });
                        if let Some(to_into) = to_into {
                            params["into"] = json!(to_into);
                        }
                        let (id, params) = owner.command(shot.id, "setEase", params);
                        actions.push(Action::Run(id, params));
                        ui.close();
                    }
                }
                ui.separator();
                if ui.button("Remove Keyframe").clicked() {
                    let (id, params) = owner.removal(shot.id, frame);
                    actions.push(Action::Run(id, params));
                    ui.close();
                }
            });
            // The marks the Animation page's timeline has: a square for a held key, a
            // circle for an eased one, a diamond otherwise.
            let (color, r) = (if dragged.is_some() || grip.hovered() || is_picked { theme.accent } else { theme.text }, 4.5);
            match (into, out) {
                (_, Interp::Hold) => painter.rect_filled(Rect::from_center_size(center, vec2(2.0 * r - 1.0, 2.0 * r - 1.0)), 0.0, color),
                (Interp::Ease | Interp::Curve(_), _) | (_, Interp::Ease | Interp::Curve(_)) => painter.circle_filled(center, r - 0.5, color),
                _ => {
                    let corners = vec![center - vec2(0.0, r), center + vec2(r, 0.0), center + vec2(0.0, r), center - vec2(r, 0.0)];
                    painter.add(egui::Shape::convex_polygon(corners, color, Stroke::NONE))
                }
            };
        }
        // The keys a cycle repeats, in the cut: darker, and not picked nor dragged.
        let chart_owner = owner.chart_owner(shot.id);
        let mut repeated = shot.repeated_keys_of(chart_owner, None, clip.out_point);
        repeated.retain(|frame| *frame >= clip.in_point && !placed.iter().any(|(.., key)| key == frame));
        let shade = theme.repeated_key();
        for frame in &repeated {
            let (center, r) = (pos2(column(film(*frame)) + frame_width / 2.0, lane.center().y), 4.0);
            let corners = vec![center - vec2(0.0, r), center + vec2(r, 0.0), center + vec2(0.0, r), center - vec2(r, 0.0)];
            painter.add(egui::Shape::convex_polygon(corners, shade, Stroke::NONE));
        }
        #[cfg(test)]
        self.repeated_marks.extend(repeated.iter().map(|frame| (owner, *frame)));
        // A line from a key to the next where it moves between them, as on the Animation
        // page's timeline, following a key that is dragged; from the last key of a cycle, a
        // darker one as far as it repeats.
        placed.sort_by_key(|(shown, ..)| *shown);
        let ends_cycle = |key: u32| shot.cycle_of(chart_owner, None, key + 1).is_some_and(|(_, to, _)| to == key);
        for (at, &(from, _, key)) in placed.iter().enumerate() {
            if self.key_drag.is_some() || !ends_cycle(key) {
                continue;
            }
            let next = placed.get(at + 1).map_or(clip.out_point, |(to, ..)| *to);
            if let Some(last) = repeated.range(from + 1..next).next_back() {
                let (start, end) = (column(film(from)) + frame_width / 2.0 + 5.0, column(film(*last)) + frame_width / 2.0 - 5.0);
                painter.hline(start..=end, lane.center().y, Stroke::new(1.5, shade));
            }
        }
        for pair in placed.windows(2) {
            let ((from, moving, key), (to, _, next)) = (pair[0], pair[1]);
            let (start, end) = (column(film(from)) + frame_width / 2.0 + 5.0, column(film(to)) + frame_width / 2.0 - 5.0);
            if moving && end > start && !(self.key_drag.is_none() && ends_cycle(key)) {
                painter.hline(start..=end, lane.center().y, Stroke::new(1.5, theme.text_muted));
                // The timing chart's lines, while no key is on its way elsewhere.
                if self.key_drag.is_none() {
                    let spacings = shot.spacings_of(owner.chart_owner(shot.id), None, key, next);
                    let (middles, row) = ((start - 5.0, end + 5.0), id.with(("chart", format!("{owner:?}"), shot.id.0)));
                    let made =
                        crate::breakdown::way(ui, painter, self.breakdowns, &mut self.chart_drag, row, key, middles, lane.center().y, ROW_HEIGHT, &spacings);
                    #[cfg(test)]
                    self.breakdown_lines.extend(made.lines.iter().map(|line| (owner, key, *line)));
                    if let Some(spacing) = made.let_go {
                        actions.push(Action::Run("edit.batch", json!({ "commands": owner.charted(shot, key, &spacing) })));
                    }
                }
            }
            #[cfg(test)]
            if moving && to > from && owner == KeyOwner::Camera {
                self.camera_tweens.push((from, to));
            }
        }
    }
}

impl StoryboardView {
    /// The commands that move all the shots and sound clips that are picked by `frames`, for
    /// `edit.batch` to run as one step: those ahead first, so that none runs into one that
    /// has yet to move. `sound` is the sound clip that was dragged, with the track it was
    /// let go on, which it alone changes to.
    fn move_picked(&self, board: &Storyboard, frames: i64, sound: Option<(SoundClipId, usize)>) -> serde_json::Value {
        let mut shots: Vec<&iskra_model::Clip> = board.clips().iter().filter(|clip| self.picked_shots.contains(&clip.shot)).collect();
        let mut sounds: Vec<(usize, &iskra_model::SoundClip)> = Vec::new();
        for (index, track) in board.sound.iter().enumerate() {
            sounds.extend(track.clips.iter().filter(|clip| self.picked_sounds.contains(&clip.id)).map(|clip| (index, clip)));
        }
        shots.sort_by_key(|clip| if frames > 0 { std::cmp::Reverse(i64::from(clip.start)) } else { std::cmp::Reverse(-i64::from(clip.start)) });
        sounds.sort_by_key(|(_, clip)| if frames > 0 { std::cmp::Reverse(i64::from(clip.start)) } else { std::cmp::Reverse(-i64::from(clip.start)) });
        let to = |start: u32| (i64::from(start) + frames).max(0);
        let moved = shots
            .iter()
            .map(|clip| json!({ "id": "storyboard.moveClip", "params": { "shot": clip.shot.0, "start": to(clip.start), "ripple": self.bar_switches.ripple } }));
        let heard = sounds.iter().map(|(index, clip)| {
            let track = sound.filter(|(dragged, _)| *dragged == clip.id).map_or(*index, |(_, track)| track);
            json!({ "id": "sound.setClip", "params": { "clip": clip.id.0, "start": to(clip.start), "track": track } })
        });
        json!({ "commands": moved.chain(heard).collect::<Vec<_>>() })
    }
}

/// What joins a clip to the shot beside it by a transition, when it is let go starting on
/// `start` of the film a little way inside that shot's clip, not as far as half of either:
/// the commands, to run as one step. The frames where the two overlap become the transition.
/// For that each shot is shortened in the cut by half of them, the outgoing one at its out
/// point and the incoming one at its in point, so that what was cut off is the handles the
/// transition shows; the cut is in the middle of the overlap, and the shots' far ends stay
/// where they were let go.
fn dissolve_into(board: &Storyboard, clip: &iskra_model::Clip, start: i64) -> Option<serde_json::Value> {
    let (length, end) = (i64::from(clip.length()), start + i64::from(clip.length()));
    board.clips().iter().filter(|other| other.shot != clip.shot).find_map(|other| {
        let (from, to) = (i64::from(other.start), i64::from(other.end()));
        let most = length.min(to - from) / 2;
        // Into the other's end, this clip comes in after it; into its start, it leads into it.
        let (outgoing, incoming, by) = match (to - start, end - from) {
            (by, _) if start >= 0 && by > 0 && by <= most => (other, clip, by),
            (_, by) if start >= 0 && by > 0 && by <= most => (clip, other, by),
            _ => return None,
        };
        // An even number of frames, half of them from each shot.
        let half = (by / 2).max(1) as u32;
        let trim = |shot: ShotId, key: &str, to: u32| json!({ "id": "storyboard.trimClip", "params": { "shot": shot.0, key: to } });
        let place = |shot: ShotId, at: i64| json!({ "id": "storyboard.moveClip", "params": { "shot": shot.0, "start": at } });
        // Where the cut is: where the shot that stays ends, or starts, once it is shorter.
        let cut = if outgoing.shot == clip.shot { from + i64::from(half) } else { to - i64::from(half) };
        let shorter = [trim(outgoing.shot, "out", outgoing.out_point - half), trim(incoming.shot, "in", incoming.in_point + half)];
        // The one that was dragged goes to the cut once it is shorter; the other is there already.
        let moved = if outgoing.shot == clip.shot { place(clip.shot, cut - (length - i64::from(half))) } else { place(clip.shot, cut) };
        let dissolve = json!({ "shot": outgoing.shot.0, "edge": "end", "kind": "crossDissolve", "length": 2 * half });
        let [out, into] = shorter;
        // The shot that stays is shortened first, to make the room the other is moved into.
        let commands = if outgoing.shot == clip.shot { vec![into, out, moved] } else { vec![out, into, moved] };
        let all: Vec<serde_json::Value> = commands.into_iter().chain([json!({ "id": "storyboard.setTransition", "params": dissolve })]).collect();
        Some(json!({ "commands": all, "title": "Add Transition" }))
    })
}

/// What a dragged playhead is caught by near `frame` of the film, with snapping: the ends of
/// the shots, the markers and the ends of the stretch that plays; and of the shots within
/// `reach` frames, where the boards' drawings start, and the keyframes of the boards' pegs and
/// of the camera, in the cut.
fn playhead_catches(project: &Project, board: &Storyboard, frame: u32, reach: u32) -> Vec<u32> {
    let mut catches: Vec<u32> = board.clips().iter().flat_map(|clip| [clip.start, clip.end()]).collect();
    catches.extend(board.markers.iter().map(|marker| marker.frame));
    catches.extend(board.range.into_iter().flat_map(|(first, after)| [first, after]));
    for clip in board.clips().iter().filter(|clip| clip.start <= frame + reach && clip.end() + reach >= frame) {
        let Some(shot) = project.shot(clip.shot) else { continue };
        let starts = boards(shot).into_iter().flat_map(|layer| layer.exposure.runs().iter().map(|run| run.start.max(clip.in_point)).collect::<Vec<_>>());
        let keyed = boards(shot).into_iter().filter_map(|layer| layer.parent).flat_map(|peg| KeyOwner::Peg(peg).frames(shot));
        let within = starts.chain(keyed).chain(shot.camera_keys()).filter(|at| (clip.in_point..clip.out_point).contains(at));
        catches.extend(within.map(|at| clip.start + at - clip.in_point));
    }
    catches
}

/// The layers of a shot that the Storyboard page shows (plan §7.2c): its boards, as the shot
/// lists them. What was animated is the Animation page's.
pub fn boards(shot: &Shot) -> Vec<&iskra_model::Layer> {
    shot.listed().into_iter().filter(|layer| shot.is_board(layer.id)).collect()
}

/// A row of the open shot on the Storyboard timeline: a board, or over it the track of the
/// peg that carries it, or under it the track of its bones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoardRow {
    Peg(PegId, LayerId),
    Layer(LayerId),
    Bones(iskra_model::DeformerId, LayerId),
}

impl BoardRow {
    /// The board it is of.
    pub(crate) fn layer(self) -> LayerId {
        match self {
            BoardRow::Peg(_, layer) | BoardRow::Layer(layer) | BoardRow::Bones(_, layer) => layer,
        }
    }
}

/// The rows of the open shot's boards, top first (the user's of 2026-10-26): each board, with
/// the track of the peg over it once that peg has a keyframe or the board has bones, and the
/// track of its bones under it once it has some. A peg that does neither has no track.
pub(crate) fn board_rows(shot: &Shot) -> Vec<BoardRow> {
    let mut rows = Vec::new();
    for layer in boards(shot) {
        let bones = layer.deformer.filter(|bones| shot.deformer(*bones).is_some());
        let peg = layer.parent.filter(|peg| shot.peg(*peg).is_some_and(|peg| bones.is_some() || !peg.animation.frames(None).is_empty()));
        rows.extend(peg.map(|peg| BoardRow::Peg(peg, layer.id)));
        rows.push(BoardRow::Layer(layer.id));
        rows.extend(bones.map(|bones| BoardRow::Bones(bones, layer.id)));
    }
    rows
}

/// The layers panel: the boards of the shot being worked on, top first. Layers and drawings
/// are added, ordered and removed here.
#[derive(Default)]
pub struct LayersView {
    /// The layer whose name is being edited, with the text so far.
    renaming: Option<(LayerId, String)>,
}

impl LayersView {
    /// `frame` is the frame of the shot that is showing; `board` says new layers are boards.
    pub fn show(&mut self, ui: &mut egui::Ui, shot: &Shot, current: Option<LayerId>, frame: u32) -> Vec<Action> {
        let mut actions = Vec::new();
        let (layers, seen) = (boards(shot), shot.stack());
        let selected = current.and_then(|id| layers.iter().position(|e| e.id == id));
        // What acts on the selected layer sits under the list.
        egui::Panel::bottom(ui.id().with("layer-buttons")).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                // A layer holds one kind of art: pixels, vector art, or texts.
                ui.menu_button("+ Layer", |ui| {
                    for (title, kind) in [("Bitmap Layer", "bitmap"), ("Vector Layer", "vector"), ("Text Layer", "text")] {
                        if ui.button(title).clicked() {
                            actions.push(Action::Run("layer.new", json!({ "shot": shot.id.0, "kind": kind })));
                        }
                    }
                })
                .response
                .on_hover_text("A new layer on top of the others, of pixels, of vector art or of texts");
                let Some(index) = selected else { return };
                let layer = layers[index];
                // Over or under the others its composite stacks.
                let (index, count) = shot.place(layer.id).unwrap_or((0, 0));
                let cell = json!({ "layer": layer.id.0, "frame": frame });
                if ui.button("+ Drawing").on_hover_text("A new, empty drawing on this layer at the playhead").clicked() {
                    actions.push(Action::Run("drawing.newDrawing", cell.clone()));
                }
                if ui
                    .add_enabled(layer.drawing_at(frame).is_some(), egui::Button::new("Duplicate"))
                    .on_hover_text("Copy the drawing at the playhead so it can change on its own")
                    .clicked()
                {
                    actions.push(Action::Run("drawing.duplicate", cell));
                }
                for (label, hint, to) in
                    [("↑", "Move the layer up", index.checked_sub(1)), ("↓", "Move the layer down", Some(index + 1).filter(|&i| i < count))]
                {
                    if ui.add_enabled(to.is_some(), egui::Button::new(label)).on_hover_text(hint).clicked() {
                        actions.push(Action::Run("layer.move", json!({ "layer": layer.id.0, "index": to })));
                    }
                }
                if ui.button("Delete").on_hover_text("Delete the layer with its drawings").clicked() {
                    actions.push(Action::Run("layer.delete", json!({ "layer": layer.id.0 })));
                }
            });
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if layers.is_empty() {
                ui.weak("This shot has no boards yet. Add a layer to draw them on.");
            }
            for layer in &layers {
                ui.horizontal(|ui| {
                    // What it holds: vector art or pixels.
                    let (icon, hint) = match layer {
                        layer if layer.text => ("text", "A text layer"),
                        layer if layer.is_vector() => ("layer-vector", "A vector layer"),
                        _ => ("layer-bitmap", "A bitmap layer"),
                    };
                    ui.add(crate::icons::image(icon, 12.0, ui.visuals().weak_text_color())).on_hover_text(hint);
                    if let Some((_, name)) = self.renaming.as_mut().filter(|(id, _)| *id == layer.id) {
                        let edit = ui.add(egui::TextEdit::singleline(name).desired_width(120.0));
                        if edit.lost_focus() {
                            // Enter keeps the new name; Escape, or the same name, changes nothing.
                            let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape)) && *name != layer.name;
                            if let Some((_, name)) = self.renaming.take().filter(|_| keep) {
                                actions.push(Action::Run("layer.rename", json!({ "layer": layer.id.0, "name": name })));
                            }
                        } else {
                            edit.request_focus();
                        }
                    } else {
                        let row = ui.selectable_label(current == Some(layer.id), &layer.name).on_hover_text("Click to draw on it, double-click to rename");
                        if row.clicked() {
                            actions.push(Action::SelectLayer(layer.id));
                        }
                        if row.double_clicked() {
                            self.renaming = Some((layer.id, layer.name.clone()));
                        }
                        // The same menu as the layer's name on the storyboard timeline.
                        let (index, count) = shot.place(layer.id).unwrap_or((0, 0));
                        row.context_menu(|ui| {
                            if ui.button("Add New Layer").clicked() {
                                actions.push(Action::Command("layer.new"));
                                ui.close();
                            }
                            if ui.button("Add New Text Layer").clicked() {
                                actions.push(Action::Command("layer.newText"));
                                ui.close();
                            }
                            if ui.button("Add New Drawing").clicked() {
                                actions.push(Action::SelectLayer(layer.id));
                                actions.push(Action::Command("drawing.newDrawing"));
                                ui.close();
                            }
                            if ui.add_enabled(index + 1 < count, egui::Button::new("Move Down")).clicked() {
                                actions.push(Action::Run("layer.move", json!({ "layer": layer.id.0, "index": index + 1 })));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Delete").on_hover_text("Delete the layer with its drawings").clicked() {
                                actions.push(Action::Run("layer.delete", json!({ "layer": layer.id.0 })));
                                ui.close();
                            }
                        });
                    }
                    let showing = layer.drawing_at(frame).map_or("no drawing here".to_string(), |d| format!("drawing {}", d.name));
                    // A layer that nothing the display shows is connected to is not in the picture.
                    let unseen = if seen.contains(&layer.id) { "" } else { " · not connected, so not seen" };
                    ui.weak(format!("{showing}{unseen}"));
                });
            }
        });
        actions
    }
}
