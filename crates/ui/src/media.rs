//! The Media page's panels (plan §7.11), after an editor's: the bins, a tree of folders of the
//! user's making that order the project's items; and the media pool, a table of the items of
//! the bin that is open.

use egui::{Align, Layout, Rect, Sense};
use egui_extras::{Column, TableBuilder};
use iskra_model::{BinId, Item, Project, ShotId, Status, StoryboardId};
use serde_json::json;

use crate::items::{Action, status_color, status_text};
use crate::theme::Theme;

/// A column of the media pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    Name,
    Status,
    Scene,
    Sequence,
    Start,
    End,
    Duration,
    Frames,
    Type,
    Resolution,
    Fps,
    Path,
}

impl Field {
    pub const ALL: [Field; 12] = [
        Field::Name,
        Field::Status,
        Field::Scene,
        Field::Sequence,
        Field::Start,
        Field::End,
        Field::Duration,
        Field::Frames,
        Field::Type,
        Field::Resolution,
        Field::Fps,
        Field::Path,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Field::Name => "Name",
            Field::Status => "Status",
            Field::Scene => "Scene",
            Field::Sequence => "Sequence",
            Field::Start => "Start TC",
            Field::End => "End TC",
            Field::Duration => "Duration",
            Field::Frames => "Frames",
            Field::Type => "Type",
            Field::Resolution => "Resolution",
            Field::Fps => "FPS",
            Field::Path => "File Path",
        }
    }
}

/// One item of the pool, as its row says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub item: Item,
    pub name: String,
    /// A shot's status, with whom it is while it is out.
    pub status: Option<(Status, String)>,
    /// A shot's scene and sequence, empty for none.
    pub scene: String,
    pub sequence: String,
    /// Where it starts and ends in its film, in frames from the film's start timecode. A shot
    /// on no storyboard has neither.
    pub span: Option<(i64, i64)>,
    /// How long it is in the cut, in frames; a shot on no storyboard, how long it is.
    pub duration: i64,
    /// Its whole length: for a shot, handles included.
    pub frames: i64,
    pub kind: &'static str,
    /// The file it is of. Shots and storyboards are of none: they are in the project.
    pub path: String,
    /// A media item's own size in pixels, if it is of pictures; a shot or a storyboard has
    /// the project's.
    pub size: Option<[u32; 2]>,
    /// The item is played at the project's frame rate: all but a still picture and a sound.
    pub timed: bool,
}

/// The rows of the items in `bin`, the storyboards first.
pub fn rows(project: &Project, bin: Option<BinId>) -> Vec<Row> {
    let row = |item: Item| match item {
        Item::Storyboard(id) => {
            let board = project.storyboard(id)?;
            let (start, length) = (i64::from(board.start), i64::from(board.end()));
            Some(Row {
                item,
                name: board.name.clone(),
                status: None,
                scene: String::new(),
                sequence: String::new(),
                span: Some((start, start + length)),
                duration: length,
                frames: length,
                kind: "Storyboard",
                path: String::new(),
                size: Some([project.settings.width, project.settings.height]),
                timed: true,
            })
        }
        Item::Shot(id) => {
            let shot = project.shot(id)?;
            let home = project.storyboard_of(id).and_then(|board| Some((board, *board.clip(id)?)));
            let span = home.map(|(board, clip)| (i64::from(board.start) + i64::from(clip.start), i64::from(board.start) + i64::from(clip.end())));
            let duration = home.map_or(i64::from(shot.duration), |(_, clip)| i64::from(clip.length()));
            let status = Some((shot.status, status_text(shot)));
            let size = Some([project.settings.width, project.settings.height]);
            Some(Row {
                item,
                name: shot.name.clone(),
                status,
                scene: shot.scene.clone(),
                sequence: shot.sequence.clone(),
                span,
                duration,
                frames: i64::from(shot.duration),
                kind: "Shot",
                path: String::new(),
                size,
                timed: true,
            })
        }
        Item::Media(id) => {
            let media = project.media(id)?;
            let length = i64::from(media.kind.length(project.settings.frame_rate));
            Some(Row {
                item,
                name: media.name.clone(),
                status: None,
                scene: String::new(),
                sequence: String::new(),
                span: None,
                duration: length,
                frames: length,
                kind: media.kind.name(),
                path: media.path.display().to_string(),
                size: media.kind.size(),
                timed: matches!(media.kind, iskra_model::MediaKind::ImageSequence { .. }),
            })
        }
    };
    project.items_in(bin).into_iter().filter_map(row).collect()
}

impl Row {
    /// What the row says in a column.
    pub fn text(&self, field: Field, project: &Project) -> String {
        let settings = &project.settings;
        let timecode = |frame: i64| iskra_model::timecode(frame, settings.frame_rate);
        match field {
            Field::Name => self.name.clone(),
            Field::Status => self.status.as_ref().map_or(String::new(), |(_, said)| said.clone()),
            Field::Scene => self.scene.clone(),
            Field::Sequence => self.sequence.clone(),
            Field::Start => self.span.map_or("—".to_string(), |(start, _)| timecode(start)),
            Field::End => self.span.map_or("—".to_string(), |(_, end)| timecode(end)),
            Field::Duration => timecode(self.duration),
            Field::Frames => self.frames.to_string(),
            Field::Type => self.kind.to_string(),
            Field::Resolution => self.size.map_or(String::new(), |[width, height]| format!("{width} × {height}")),
            Field::Fps => {
                if self.timed {
                    settings.frame_rate.label()
                } else {
                    String::new()
                }
            }
            Field::Path => self.path.clone(),
        }
    }
}

/// Whether a media item is of pictures, which can be placed in a shot.
fn pictures(project: &Project, media: iskra_model::MediaId) -> bool {
    project.media(media).is_some_and(|media| media.kind.size().is_some())
}

/// Put rows in the order of a column, upwards or downwards: names and words by their letters,
/// times and counts by their numbers. Rows that are alike there keep their order.
pub fn sort(rows: &mut [Row], by: Field, upwards: bool) {
    let number = |row: &Row| match by {
        Field::Start => row.span.map(|(start, _)| start),
        Field::End => row.span.map(|(_, end)| end),
        Field::Duration => Some(row.duration),
        Field::Frames => Some(row.frames),
        _ => None,
    };
    let word = |row: &Row| match by {
        Field::Status => row.status.as_ref().map_or(String::new(), |(_, said)| said.to_lowercase()),
        Field::Scene => row.scene.to_lowercase(),
        Field::Sequence => row.sequence.to_lowercase(),
        Field::Type => row.kind.to_string(),
        Field::Path => row.path.to_lowercase(),
        _ => row.name.to_lowercase(),
    };
    match by {
        Field::Start | Field::End | Field::Duration | Field::Frames => rows.sort_by_key(number),
        // Every row has the project's.
        Field::Resolution | Field::Fps => {}
        _ => rows.sort_by_key(word),
    }
    if !upwards {
        rows.reverse();
    }
}

/// What the Media page's panels remember between frames.
#[derive(Default)]
pub struct Media {
    /// The bin that is open: the one the pool lists. `None` is Master.
    pub bin: Option<BinId>,
    /// The bin whose name is being edited, with the text so far.
    renaming: Option<(BinId, String)>,
    /// The storyboard whose name is being edited in the pool, with the text so far.
    pub renaming_board: Option<(StoryboardId, String)>,
    /// The bins whose own bins are not shown in the tree. Master is `None`.
    pub closed: Vec<Option<BinId>>,
    /// The one status whose shots the pool lists, if it is narrowed to one.
    pub only: Option<Status>,
    /// The column the pool is in the order of, and whether upwards.
    pub order: Option<(Field, bool)>,
    /// The columns that are not shown.
    pub hidden: Vec<Field>,
    /// Where each bin's row was drawn in the Bins panel, to know which a drop is on.
    pub bin_rows: Vec<(Option<BinId>, Rect)>,
    /// The pool's inspector is open on its right: what the item clicked last is, and what is
    /// done to it, as buttons.
    pub inspector: bool,
    /// The item clicked last in the pool, which the inspector and the Preview panel show.
    pub picked: Option<Item>,
    /// A scene's or a sequence's name being typed in a shot's menu.
    new_group: String,

    /// Where each item's row was drawn in the pool.
    #[cfg(test)]
    pub rows: Vec<(Item, Rect)>,
}

/// How many shots a storyboard has, and how many of each status that some have, in the
/// statuses' order: what the inspector says of it.
pub fn counts(project: &Project, storyboard: StoryboardId) -> (usize, Vec<(Status, usize)>) {
    let Some(board) = project.storyboard(storyboard) else { return (0, Vec::new()) };
    let shots: Vec<&iskra_model::Shot> = board.clips().iter().filter_map(|clip| project.shot(clip.shot)).collect();
    let by_status = Status::ALL.into_iter().map(|status| (status, shots.iter().filter(|shot| shot.status == status).count())).filter(|(_, count)| *count > 0);
    (shots.len(), by_status.collect())
}

/// The bins from Master down to `bin`, as a path of their names.
fn path_of(project: &Project, bin: Option<BinId>) -> String {
    let mut names: Vec<&str> = std::iter::successors(bin.and_then(|id| project.bin(id)), |bin| bin.parent.and_then(|id| project.bin(id)))
        .take(project.bins.len() + 1)
        .map(|bin| bin.name.as_str())
        .collect();
    names.push("Master");
    names.reverse();
    names.join(" › ")
}

impl Media {
    /// The bin a drop at `pos` of the window is on, if it is on a row of the Bins panel.
    pub fn bin_at(&self, pos: egui::Pos2) -> Option<Option<BinId>> {
        self.bin_rows.iter().find(|(_, row)| row.contains(pos)).map(|(bin, _)| *bin)
    }

    /// The Bins panel: Master and the bins in it, as a tree. A click opens a bin in the pool.
    pub fn bins(&mut self, ui: &mut egui::Ui, project: &Project, new_bin: bool) -> Vec<Action> {
        let mut actions = Vec::new();
        self.bin = project.bin_or_master(self.bin);
        self.bin_rows.clear();
        if new_bin {
            actions.extend(self.new_bin(ui, project));
            ui.separator();
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| self.bin_row(ui, project, None, 0, &mut actions));
        actions
    }

    /// *New Bin*: a bin in the bin that is open. In the Bins panel's bar, and over the bins
    /// beside the pool where the page has no Bins panel.
    pub fn new_bin(&mut self, ui: &mut egui::Ui, project: &Project) -> Option<Action> {
        self.bin = project.bin_or_master(self.bin);
        let clicked = ui.button("New Bin").on_hover_text("A bin in the bin that is open").clicked();
        clicked.then(|| Action::Run("bin.new", json!({ "parent": self.bin.map(|bin| bin.0) })))
    }

    /// A bin's row, and under it the rows of the bins it holds.
    fn bin_row(&mut self, ui: &mut egui::Ui, project: &Project, bin: Option<BinId>, depth: usize, actions: &mut Vec<Action>) {
        let name = bin.and_then(|id| project.bin(id)).map_or("Master", |bin| bin.name.as_str());
        let held = project.items_in(bin).len();
        let (has_bins, closed) = (!project.bins_in(bin).is_empty(), self.closed.contains(&bin));
        let row = ui.horizontal(|ui| {
            ui.add_space(14.0 * depth as f32);
            // A bin that holds bins has a mark to show or hide them by; the others have its room.
            let (place, mark) = ui.allocate_exact_size(egui::vec2(14.0, 16.0), Sense::click());
            if has_bins {
                let theme = Theme::of(ui.ctx());
                let color = if mark.hovered() { theme.text } else { theme.text_muted };
                ui.painter().text(place.center(), egui::Align2::CENTER_CENTER, if closed { "›" } else { "⌄" }, egui::FontId::proportional(13.0), color);
                if mark.on_hover_text(if closed { "Show the bins in it" } else { "Hide the bins in it" }).clicked() {
                    self.closed.retain(|other| *other != bin);
                    if !closed {
                        self.closed.push(bin);
                    }
                }
            }
            if let (Some(id), Some((_, text))) = (bin, self.renaming.as_mut().filter(|(renamed, _)| Some(*renamed) == bin)) {
                let edit = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
                if edit.lost_focus() {
                    // Enter keeps the new name; Escape, or the same name, changes nothing.
                    let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape)) && text != name;
                    if let Some((_, text)) = self.renaming.take().filter(|_| keep) {
                        actions.push(Action::Run("bin.rename", json!({ "bin": id.0, "name": text })));
                    }
                } else {
                    edit.request_focus();
                }
                return None;
            }
            let label = ui.selectable_label(self.bin == bin, name);
            ui.weak(held.to_string());
            Some(label)
        });
        self.bin_rows.push((bin, row.response.rect.expand2(egui::vec2(40.0, 1.0))));
        if let Some(label) = row.inner {
            if label.clicked() {
                self.bin = bin;
            }
            if let (true, Some(id)) = (label.double_clicked(), bin) {
                self.renaming = Some((id, name.to_string()));
            }
            label.context_menu(|ui| {
                if ui.button("New Bin Here").clicked() {
                    actions.push(Action::Run("bin.new", json!({ "parent": bin.map(|bin| bin.0) })));
                    ui.close();
                }
                let Some(id) = bin else { return };
                if ui.button("Rename").clicked() {
                    self.renaming = Some((id, name.to_string()));
                    ui.close();
                }
                if ui.button("Delete").on_hover_text("What it holds moves to the bin it is in. Nothing else is deleted").clicked() {
                    actions.push(Action::Run("bin.delete", json!({ "bin": id.0 })));
                    ui.close();
                }
            });
        }
        // A tree is never deeper than it has bins.
        if !closed && depth <= project.bins.len() {
            let inside: Vec<BinId> = project.bins_in(bin).iter().map(|bin| bin.id).collect();
            for bin in inside {
                self.bin_row(ui, project, Some(bin), depth + 1, actions);
            }
        }
    }

    /// A part of the bar over the pool: new storyboards and shots in the bin and shots brought
    /// back, the status shown, the columns, where the bin is, and the inspector.
    pub fn pool_bar_part(&mut self, ui: &mut egui::Ui, part: &str, project: &Project) -> Vec<Action> {
        let mut actions = Vec::new();
        self.bin = project.bin_or_master(self.bin);
        let bin = self.bin.map(|bin| bin.0);
        match part {
            "poolnew" => {
                if ui.button("New Storyboard").clicked() {
                    actions.push(Action::Run("storyboard.new", json!({ "bin": bin })));
                }
                if ui.button("New Shot").on_hover_text("A shot in this bin, on no storyboard yet").clicked() {
                    actions.push(Action::Run("shot.new", json!({ "bin": bin })));
                }
                if ui.button("Bring Back…").on_hover_text("Pick the file of a shot that was sent out: it takes the place of the shot it came from").clicked()
                {
                    actions.push(Action::BringBack(None));
                }
            }
            "poolshow" => {
                ui.menu_button(format!("Show: {}  ⌄", self.only.map_or("every status", Status::label)), |ui| {
                    if ui.radio(self.only.is_none(), "Every status").clicked() {
                        self.only = None;
                        ui.close();
                    }
                    for status in Status::ALL {
                        let count = project.shots.iter().filter(|shot| shot.status == status).count();
                        if ui.radio(self.only == Some(status), format!("{} ({count})", status.label())).clicked() {
                            self.only = Some(status);
                            ui.close();
                        }
                    }
                });
            }
            "poolcolumns" => {
                ui.menu_button("Columns  ⌄", |ui| {
                    for field in Field::ALL.into_iter().filter(|field| *field != Field::Name) {
                        let mut shown = !self.hidden.contains(&field);
                        if ui.checkbox(&mut shown, field.title()).changed() {
                            self.hidden.retain(|hidden| *hidden != field);
                            if !shown {
                                self.hidden.push(field);
                            }
                        }
                    }
                });
            }
            "poolbin" => drop(ui.weak(path_of(project, self.bin))),
            "inspector" => {
                let hint = "The inspector: what the item clicked is, and what is done to it, beside the pool";
                if crate::icons::switch(ui, "inspector", self.inspector, hint).clicked() {
                    self.inspector = !self.inspector;
                }
            }
            _ => {}
        }
        actions
    }

    /// The media pool: the items of the bin that is open, a row each. `storyboard` and `shot`
    /// are the ones that are open, whose rows are marked.
    pub fn pool(&mut self, ui: &mut egui::Ui, project: &Project, storyboard: Option<StoryboardId>, shot: Option<ShotId>) -> Vec<Action> {
        let mut actions = Vec::new();
        self.bin = project.bin_or_master(self.bin);
        if self.inspector {
            let side = egui::Panel::right("pool inspector").resizable(true).default_size(240.0).size_range(180.0..=420.0);
            side.show(ui, |ui| egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui, project, storyboard, shot, &mut actions)));
        }
        let mut listed = rows(project, self.bin);
        // Narrowed to a status, the pool lists the shots that have it.
        listed.retain(|row| self.only.is_none_or(|only| row.status.as_ref().is_some_and(|(status, _)| *status == only)));
        if let Some((by, upwards)) = self.order {
            sort(&mut listed, by, upwards);
        }
        #[cfg(test)]
        self.rows.clear();
        let fields: Vec<Field> = Field::ALL.into_iter().filter(|field| !self.hidden.contains(field)).collect();
        let theme = Theme::of(ui.ctx());
        let mut table = TableBuilder::new(ui).striped(true).resizable(true).sense(Sense::click_and_drag()).cell_layout(Layout::left_to_right(Align::Center));
        for (index, field) in fields.iter().enumerate() {
            let least = if *field == Field::Name { 110.0 } else { 56.0 };
            table = table.column(if index + 1 == fields.len() {
                Column::remainder().at_least(least).clip(true)
            } else {
                Column::auto().at_least(least).clip(true)
            });
        }
        let mut order = self.order;
        table
            .header(20.0, |mut header| {
                for field in &fields {
                    header.col(|ui| {
                        // A click puts the pool in the column's order; another turns it round.
                        let arrow = match order {
                            Some((by, upwards)) if by == *field => {
                                if upwards {
                                    "  ↑"
                                } else {
                                    "  ↓"
                                }
                            }
                            _ => "",
                        };
                        if ui.add(egui::Button::new(egui::RichText::new(format!("{}{arrow}", field.title())).strong()).frame(false)).clicked() {
                            order = Some((*field, !matches!(order, Some((by, true)) if by == *field)));
                        }
                    });
                }
            })
            .body(|mut body| {
                for row in &listed {
                    body.row(20.0, |mut line| {
                        line.set_selected(match row.item {
                            Item::Shot(id) => shot == Some(id),
                            Item::Storyboard(id) => storyboard == Some(id),
                            Item::Media(_) => false,
                        });
                        for field in &fields {
                            line.col(|ui| {
                                if let (Field::Status, Some((status, _))) = (field, &row.status) {
                                    let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
                                    ui.painter().circle_filled(dot.center(), 4.0, status_color(ui.ctx(), *status));
                                }
                                if let (Field::Name, Item::Storyboard(id)) = (field, row.item) {
                                    self.name_edit(ui, id, &row.name, &mut actions);
                                    if self.renaming_board.as_ref().is_some_and(|(renamed, _)| *renamed == id) {
                                        return;
                                    }
                                }
                                let text = row.text(*field, project);
                                let text = if *field == Field::Name {
                                    egui::RichText::new(text).color(theme.text)
                                } else {
                                    egui::RichText::new(text).color(theme.text_muted)
                                };
                                ui.add(egui::Label::new(text).selectable(false).truncate());
                            });
                        }
                        let response = line.response();
                        #[cfg(test)]
                        self.rows.push((row.item, response.rect));
                        self.row_input(&response, project, row, storyboard, &mut actions);
                    });
                }
            });
        self.order = order;
        if listed.is_empty() {
            ui.weak("Nothing in this bin. New items are made here, and items are dragged here from other bins.");
        }
        actions
    }

    /// The field a storyboard's name is edited in, in its row, while it is being renamed.
    fn name_edit(&mut self, ui: &mut egui::Ui, id: StoryboardId, name: &str, actions: &mut Vec<Action>) {
        let Some((_, text)) = self.renaming_board.as_mut().filter(|(renamed, _)| *renamed == id) else { return };
        let edit = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
        if edit.lost_focus() {
            // Enter keeps the new name; Escape, or the same name, changes nothing.
            let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape)) && text != name && !text.trim().is_empty();
            if let Some((_, text)) = self.renaming_board.take().filter(|_| keep) {
                actions.push(Action::Run("storyboard.rename", json!({ "storyboard": id.0, "name": text })));
            }
        } else {
            edit.request_focus();
        }
    }

    /// What a click, a double click, a drag or a right-click on an item's row asks for.
    fn row_input(&mut self, response: &egui::Response, project: &Project, row: &Row, storyboard: Option<StoryboardId>, actions: &mut Vec<Action>) {
        if response.clicked() || response.secondary_clicked() {
            self.picked = Some(row.item);
        }
        match row.item {
            Item::Shot(id) if response.double_clicked() => actions.push(Action::OpenShot(id)),
            Item::Shot(id) if response.clicked() => actions.push(Action::SelectShot(id)),
            Item::Storyboard(id) if response.double_clicked() => actions.push(Action::OpenStoryboard(id)),
            Item::Media(id) if response.double_clicked() && pictures(project, id) => actions.push(Action::Place(id)),
            Item::Media(id) if response.double_clicked() => actions.push(Action::Listen(id)),
            _ => {}
        }
        if response.drag_started() {
            actions.push(Action::Drag(vec![row.item]));
        }
        response.context_menu(|ui| self.item_menu(ui, project, row.item, storyboard, actions));
    }

    /// What is done to an item: its row's right-click menu, and the inspector's buttons.
    fn item_menu(&mut self, ui: &mut egui::Ui, project: &Project, item: Item, storyboard: Option<StoryboardId>, actions: &mut Vec<Action>) {
        {
            match item {
                Item::Media(id) => {
                    if !pictures(project, id)
                        && ui.button("Listen, or Stop").on_hover_text("Plays the sound from its start. A double click does the same").clicked()
                    {
                        actions.push(Action::Listen(id));
                        ui.close();
                    }
                    let hint = "As a layer of the shot that is worked on, on top: a picture for the whole shot, a sequence from the playhead on";
                    if ui
                        .add_enabled(pictures(project, id), egui::Button::new("Place in the Shot"))
                        .on_hover_text(hint)
                        .on_disabled_hover_text("Sound goes on a sound track")
                        .clicked()
                    {
                        actions.push(Action::Place(id));
                        ui.close();
                    }
                    if ui.button("Relink…").on_hover_text("Point it at another file: one that was moved or renamed, or a newer take").clicked() {
                        actions.push(Action::Relink(id));
                        ui.close();
                    }
                    // What is spoken in a sound, for the dialogue of the panels it lies under.
                    if let Some(media) = project.media(id).filter(|media| matches!(media.kind, iskra_model::MediaKind::Sound { .. })) {
                        let has = if media.transcript.is_empty() {
                            "It has none yet".to_string()
                        } else {
                            format!("It has one now, of {} entries", media.transcript.len())
                        };
                        let hint = format!("What is spoken in it, from a subtitle file (.srt, .vtt) or a list of words (.json). {has}");
                        if ui.button("Import Transcript…").on_hover_text(hint).clicked() {
                            actions.push(Action::Transcript(id));
                            ui.close();
                        }
                    }
                    if ui.button("Delete").on_hover_text("Takes it out of the project. Its file is left as it is").clicked() {
                        actions.push(Action::Run("media.delete", json!({ "media": id.0 })));
                        ui.close();
                    }
                    return;
                }
                Item::Storyboard(id) => {
                    let Some(board) = project.storyboard(id) else { return };
                    if ui.button("Open").clicked() {
                        actions.push(Action::OpenStoryboard(id));
                        ui.close();
                    }
                    if ui.button("Rename").on_hover_text("Its name is edited in its row").clicked() {
                        self.renaming_board = Some((id, board.name.clone()));
                        ui.close();
                    }
                    if ui.add_enabled(!board.clips().is_empty(), egui::Button::new("Send Its Shots Out…")).clicked() {
                        actions.push(Action::SendOut(board.clips().iter().map(|clip| clip.shot).collect()));
                        ui.close();
                    }
                    if ui.button("Delete").on_hover_text("Its shots stay in the project").clicked() {
                        actions.push(Action::Run("storyboard.delete", json!({ "storyboard": id.0 })));
                        ui.close();
                    }
                }
                Item::Shot(id) => {
                    let Some(shot) = project.shot(id) else { return };
                    if ui.button("Open for Animation").clicked() {
                        actions.push(Action::OpenShot(id));
                        ui.close();
                    }
                    ui.menu_button("Status", |ui| {
                        for status in Status::ALL {
                            if ui.radio(shot.status == status, status.label()).clicked() {
                                actions.push(Action::Run("shot.setStatus", json!({ "shot": id.0, "status": status })));
                                ui.close();
                            }
                        }
                    });
                    let groups = [("Scene", "scene", &shot.scene, project.scenes()), ("Sequence", "sequence", &shot.sequence, project.sequences())];
                    for (title, key, now, in_use) in groups {
                        ui.menu_button(title, |ui| {
                            for name in in_use.iter().chain(std::iter::once(&String::new())) {
                                if ui.radio(now == name, if name.is_empty() { "None" } else { name }).clicked() {
                                    actions.push(Action::Run("shot.setScene", json!({ "shots": [id.0], key: name })));
                                    ui.close();
                                }
                            }
                            ui.separator();
                            // A name not in use yet, typed and kept with Enter.
                            let typed = ui.add(egui::TextEdit::singleline(&mut self.new_group).hint_text(format!("New {key}")).desired_width(140.0));
                            if typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.new_group.trim().is_empty() {
                                actions.push(Action::Run("shot.setScene", json!({ "shots": [id.0], key: std::mem::take(&mut self.new_group) })));
                                ui.close();
                            }
                        });
                    }
                    if ui.button("Send Out…").on_hover_text("Writes the shot as a project file of its own, for someone to work on").clicked() {
                        actions.push(Action::SendOut(vec![id]));
                        ui.close();
                    }
                    if ui.button("Bring Back…").on_hover_text("Replaces the shot with the one in a file, after showing what changed").clicked() {
                        actions.push(Action::BringBack(Some(id)));
                        ui.close();
                    }
                    match (project.storyboard_of(id), storyboard) {
                        (Some(_), _) => {
                            if ui.button("Take Off Its Storyboard").clicked() {
                                actions.push(Action::Run("storyboard.remove", json!({ "shot": id.0 })));
                                ui.close();
                            }
                        }
                        (None, Some(board)) => {
                            if ui.button("Put on the Storyboard").on_hover_text("After the last shot of the storyboard that is open").clicked() {
                                actions.push(Action::Run("storyboard.place", json!({ "storyboard": board.0, "shot": id.0 })));
                                ui.close();
                            }
                        }
                        (None, None) => {}
                    }
                    if ui.button("Delete").on_hover_text("Delete the shot with its drawings. Undo brings it back").clicked() {
                        actions.push(Action::Run("shot.delete", json!({ "shot": id.0 })));
                        ui.close();
                    }
                }
            }
            ui.menu_button("Move to Bin", |ui| {
                let here = project.bin_of(item);
                let bins = std::iter::once(None).chain(project.bins.iter().map(|bin| Some(bin.id)));
                let mut bins: Vec<(String, Option<BinId>)> = bins.map(|bin| (path_of(project, bin), bin)).collect();
                bins.sort();
                for (path, bin) in bins {
                    if ui.radio(here == bin, path).clicked() {
                        actions.push(Action::Run("item.setBin", json!({ "items": [item], "bin": bin.map(|bin| bin.0) })));
                        ui.close();
                    }
                }
            });
        }
    }

    /// The inspector, beside the pool (the user's of 2026-10-26): what the item clicked last
    /// is, or the shot in hand, and what its menu does, as buttons. Of a storyboard, how many
    /// shots it has, and how many of each status.
    fn inspector(&mut self, ui: &mut egui::Ui, project: &Project, storyboard: Option<StoryboardId>, shot: Option<ShotId>, actions: &mut Vec<Action>) {
        let Some(item) = self.picked.or(shot.map(Item::Shot)) else {
            ui.weak("Click an item in the pool to see it here.");
            return;
        };
        let fact = |ui: &mut egui::Ui, name: &str, value: String| {
            ui.horizontal(|ui| {
                ui.weak(name);
                ui.label(value);
            });
        };
        let rate = project.settings.frame_rate.as_f64();
        let seconds = |frames: i64| format!("{frames} frames, {:.1} s", frames as f64 / rate.max(1.0));
        match item {
            Item::Shot(id) => {
                let Some(found) = project.shot(id) else { return };
                ui.heading(&found.name);
                let status = match &found.out_to {
                    Some(who) if found.status == Status::Out => format!("Out with {who}"),
                    _ => found.status.label().to_string(),
                };
                fact(ui, "Status", status);
                let none = |name: &str| if name.is_empty() { "None".to_string() } else { name.to_string() };
                fact(ui, "Scene", none(&found.scene));
                fact(ui, "Sequence", none(&found.sequence));
                let cut = project.storyboard_of(id).and_then(|board| board.clip(id).copied());
                fact(ui, "In the cut", cut.map_or("On no storyboard".to_string(), |clip| seconds(i64::from(clip.out_point - clip.in_point))));
                fact(ui, "Whole", seconds(i64::from(found.duration)));
                fact(ui, "Layers", found.layers.len().to_string());
            }
            Item::Storyboard(id) => {
                let Some(board) = project.storyboard(id) else { return };
                ui.heading(&board.name);
                let (shots, by_status) = counts(project, id);
                fact(ui, "Shots", shots.to_string());
                for (status, count) in by_status {
                    fact(ui, &format!("  {}", status.label()), count.to_string());
                }
                fact(ui, "Length", seconds(i64::from(board.end()) - i64::from(board.start)));
                fact(ui, "Sound tracks", board.sound.len().to_string());
            }
            Item::Media(id) => {
                let Some(media) = project.media(id) else { return };
                ui.heading(&media.name);
                fact(ui, "Kind", media.kind.name().to_string());
                match media.kind {
                    iskra_model::MediaKind::Image { width, height } => fact(ui, "Size", format!("{width} × {height}")),
                    iskra_model::MediaKind::ImageSequence { width, height, frames } => {
                        fact(ui, "Size", format!("{width} × {height}"));
                        fact(ui, "Frames", frames.to_string());
                    }
                    iskra_model::MediaKind::Sound { sample_rate, channels, frames } => {
                        fact(ui, "Length", format!("{:.1} s", frames as f64 / f64::from(sample_rate.max(1))));
                        fact(ui, "Channels", channels.to_string());
                    }
                }
                ui.add(egui::Label::new(egui::RichText::new(media.path.display().to_string()).small().weak()).wrap());
            }
        }
        ui.separator();
        // What its menu does, as buttons, one under another.
        ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| self.item_menu(ui, project, item, storyboard, actions));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_model::{Bin, Clip};

    use super::*;

    #[test]
    fn a_storyboard_is_counted_by_its_shots_statuses() {
        let mut project = Project::default();
        let board = project.storyboards[0].id;
        let mut second = project.new_shot("Second", 24);
        second.status = Status::Animation;
        let second_id = second.id;
        project.shots.push(Arc::new(second));
        project.storyboard_mut(board).unwrap().put(Clip { shot: second_id, start: 48, in_point: 0, out_point: 24 }, false).unwrap();
        assert_eq!(counts(&project, board), (2, vec![(Status::Board, 1), (Status::Animation, 1)]));
    }

    #[test]
    fn a_row_says_where_an_item_is_in_its_film_and_rows_sort_by_a_column() {
        let mut project = Project::default();
        let (first, board) = (project.shots[0].id, project.storyboards[0].id);
        let mut spare = project.new_shot("Spare", 30);
        spare.status = Status::Out;
        spare.out_to = Some("Ana".to_string());
        spare.sequence = "SQ01".to_string();
        let spare_id = spare.id;
        project.shots.push(Arc::new(spare));
        // The film starts at one hour; the first shot is cut from its frame 12 to 36.
        let film = project.storyboard_mut(board).unwrap();
        film.start = 86400;
        film.put(Clip { shot: first, start: 0, in_point: 12, out_point: 36 }, false).unwrap();

        let listed = rows(&project, None);
        assert_eq!(listed.iter().map(|row| row.item).collect::<Vec<_>>(), [Item::Storyboard(board), Item::Shot(first), Item::Shot(spare_id)]);
        let says = |row: &Row| Field::ALL.map(|field| row.text(field, &project));
        assert_eq!(says(&listed[0]), ["Storyboard", "", "", "", "01:00:00:00", "01:00:01:00", "00:00:01:00", "24", "Storyboard", "1920 × 1080", "24", ""]);
        assert_eq!(
            says(&listed[1]),
            ["Shot_01", "Board", "", "", "01:00:00:00", "01:00:01:00", "00:00:01:00", "48", "Shot", "1920 × 1080", "24", ""],
            "the cut part, and the whole in frames"
        );
        assert_eq!(says(&listed[2])[..8], ["Spare", "Out with Ana", "", "SQ01", "—", "—", "00:00:01:06", "30"], "on no storyboard, it is nowhere in a film");

        let names = |rows: &[Row]| rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();
        let mut sorted = listed.clone();
        sort(&mut sorted, Field::Frames, true);
        assert_eq!(names(&sorted), ["Storyboard", "Spare", "Shot_01"]);
        sort(&mut sorted, Field::Name, false);
        assert_eq!(names(&sorted), ["Storyboard", "Spare", "Shot_01"], "downwards by name");
        sort(&mut sorted, Field::Start, true);
        assert_eq!(names(&sorted)[0], "Spare", "what has no start comes first");
        sort(&mut sorted, Field::Sequence, false);
        assert_eq!(names(&sorted)[0], "Spare", "by its sequence, the one that has one first downwards");

        // A bin lists what is in it, and says where it is.
        let scenes = BinId(project.new_id());
        project.bins.push(Bin { id: scenes, name: "Scenes".to_string(), parent: None });
        project.set_bin(Item::Shot(spare_id), Some(scenes)).unwrap();
        assert_eq!((names(&rows(&project, Some(scenes))), rows(&project, None).len()), (vec!["Spare".to_string()], 2));
        assert_eq!((path_of(&project, Some(scenes)), path_of(&project, None)), ("Master › Scenes".to_string(), "Master".to_string()));
    }
}
