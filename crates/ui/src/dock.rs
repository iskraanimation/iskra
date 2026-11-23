//! Docking (plan §7.4): a page's panels sit in a tree of splits and tab groups. A group has a
//! tab strip; dragging a tab onto another group makes it a tab there or splits that group, and
//! the gaps between groups drag to resize them. A layout is the tree, which serializes.
//!
//! The tree, its operations, the drop zones and the layout pass are ported from EffectCraft's
//! `ui-egui/src/dock.rs` (MIT OR Apache-2.0; see `ATTRIBUTION.md`), without its stacked and
//! floating groups. The tab strip is our own.

use egui::{Align2, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};

use crate::theme::Theme;

/// The app's pages (plan §7.2): the main sections of the work, switched in the bar at the bottom
/// of the window. Each is a workspace with its own layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Page {
    Media,
    Storyboard,
    Animation,
    Export,
}

impl Page {
    pub const ALL: [Page; 4] = [Page::Media, Page::Storyboard, Page::Animation, Page::Export];

    pub const fn title(self) -> &'static str {
        match self {
            Page::Media => "Media",
            Page::Storyboard => "Storyboard",
            Page::Animation => "Animation",
            Page::Export => "Export",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Media => "folder-open",
            Page::Storyboard => "storyboard",
            Page::Animation => "sparkles",
            Page::Export => "export",
        }
    }

    /// What the page is for, as its tooltip in the page bar says.
    pub fn purpose(self) -> &'static str {
        match self {
            Page::Media => "Media: everything the project holds",
            Page::Storyboard => "Storyboard: the film as a sequence of shots",
            Page::Animation => "Animation: one shot, drawn and timed",
            Page::Export => "Export: rendering, shot by shot",
        }
    }

    /// Whether the page has the drawing tools.
    pub fn draws(self) -> bool {
        matches!(self, Page::Storyboard | Page::Animation)
    }
}

/// The panels. Any of them can be opened on any page; the pages differ in what they start with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Panel {
    Viewport,
    Timeline,
    Palette,
    /// The brush presets, and the brush in hand.
    Brushes,
    OnionSkin,
    History,
    /// The project bin: the storyboards and shots.
    Bin,
    /// The storyboard timeline: a storyboard's shots as clips.
    Storyboard,
    /// The layers of the shot being worked on, as a list.
    // Layers were called elements until 2026-10-09; layouts saved before then say so.
    #[serde(alias = "elements")]
    Layers,
    /// What is selected in the timeline, in numbers: a peg's transform.
    Properties,
    /// The settings of the tool in hand, as the tool options bar has them, a group a row.
    ToolProperties,
    /// The function curve editor: a peg's values over time, as curves.
    Curves,
    /// The drawing substitution picker: the selected layer's drawings as thumbnails.
    Substitution,
    /// The node view: the shot's graph as boxes and wires.
    NodeView,
    /// The scene from above: x across, depth up the screen, with the camera.
    TopView,
    /// The scene from the side: depth across, y down.
    SideView,
    /// The library: folders of templates to put into shots.
    Library,
    /// The media pool: the items of the bin that is open, as a table.
    Pool,
    /// Media storage: the folders and files of the disk.
    Storage,
    /// The item picked in the pool, seen: a picture, a sequence's frames, a sound's waveform.
    Preview,
    /// What is written of the panel under the playhead: action, dialogue, notes. Only read
    /// from layouts of before 2026-10-22: it is the foot of the Panels panel now, which takes
    /// its place when the layout is read.
    ShotNotes,
    /// The storyboard as cards, a card a panel.
    Panels,
    /// What the app did and what went wrong, to copy into a report.
    Log,
    /// The fonts, and the format of the text being typed in (plan §4.19).
    Text,
    /// A script typed in and run, and what came of it (plan §9).
    ScriptConsole,
    /// The retake notes of every shot, to be gone through and ticked done (plan §9.6).
    Notes,
}

impl Panel {
    pub const ALL: [Panel; 25] = [
        Panel::Viewport,
        Panel::Timeline,
        Panel::Palette,
        Panel::Brushes,
        Panel::OnionSkin,
        Panel::History,
        Panel::Bin,
        Panel::Storyboard,
        Panel::Layers,
        Panel::Properties,
        Panel::ToolProperties,
        Panel::Curves,
        Panel::Substitution,
        Panel::NodeView,
        Panel::TopView,
        Panel::SideView,
        Panel::Library,
        Panel::Pool,
        Panel::Storage,
        Panel::Preview,
        Panel::Panels,
        Panel::Log,
        Panel::Text,
        Panel::ScriptConsole,
        Panel::Notes,
    ];

    pub fn title(self) -> &'static str {
        self.title_const()
    }

    /// The parts of the bar over the panel (see `bar::of`): none for a panel without one.
    /// The dock draws it above what the panel shows; the timelines draw their own bars.
    pub fn bar_parts(self) -> Vec<crate::bar::Part> {
        crate::bar::of(self)
    }

    /// [`Panel::title`], for constants.
    pub const fn title_const(self) -> &'static str {
        match self {
            Panel::Viewport => "Viewport",
            Panel::Timeline => "Timeline",
            Panel::Palette => "Palette",
            Panel::Brushes => "Brushes",
            Panel::OnionSkin => "Onion Skin",
            Panel::History => "History",
            Panel::Bin => "Bins",
            Panel::Storyboard => "Storyboard",
            Panel::Layers => "Layers",
            Panel::Properties => "Properties",
            Panel::ToolProperties => "Tool Properties",
            Panel::Curves => "Function Curves",
            Panel::Substitution => "Drawing Substitution",
            Panel::NodeView => "Node View",
            Panel::TopView => "Top View",
            Panel::SideView => "Side View",
            Panel::Library => "Library",
            Panel::Pool => "Media Pool",
            Panel::Storage => "Media Storage",
            Panel::Preview => "Preview",
            Panel::ShotNotes => "Shot Notes",
            Panel::Panels => "Panels",
            Panel::Log => "Log",
            Panel::Text => "Text",
            Panel::ScriptConsole => "Script Console",
            Panel::Notes => "Retake Notes",
        }
    }
}

/// How a split shares its length between its two children.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SplitSize {
    /// The first child's share, 0 to 1.
    Ratio(f32),
    /// The first child has a fixed size in points.
    FixedA(f32),
    /// The second child has a fixed size in points.
    FixedB(f32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DockNode {
    /// `vertical`: the children are one above the other; else side by side.
    Split {
        vertical: bool,
        size: SplitSize,
        a: Box<DockNode>,
        b: Box<DockNode>,
    },
    Tabs {
        panels: Vec<Panel>,
        active: usize,
    },
}

fn tabs(panels: &[Panel]) -> DockNode {
    DockNode::Tabs { panels: panels.to_vec(), active: 0 }
}

fn split(vertical: bool, size: SplitSize, a: DockNode, b: DockNode) -> DockNode {
    DockNode::Split { vertical, size, a: Box::new(a), b: Box::new(b) }
}

impl Default for DockNode {
    /// The Animation page as it starts (plan §7.3): the viewport in the centre, the timeline
    /// under it, and on the right what is about the drawing in hand and about what is selected.
    fn default() -> Self {
        let centre = split(
            true,
            SplitSize::FixedB(190.0),
            tabs(&[Panel::Viewport, Panel::NodeView, Panel::TopView, Panel::SideView]),
            tabs(&[Panel::Timeline, Panel::Curves]),
        );
        let about = tabs(&[Panel::Properties, Panel::ToolProperties, Panel::Substitution, Panel::Library, Panel::History]);
        let sidebar = split(true, SplitSize::Ratio(0.5), tabs(&[Panel::Palette, Panel::Brushes, Panel::OnionSkin]), about);
        // Wide enough for the three tabs of its upper stack.
        split(false, SplitSize::FixedB(264.0), centre, sidebar)
    }
}

impl DockNode {
    /// The layout a page starts with, and returns to when it is reset.
    pub fn for_page(page: Page) -> Self {
        match page {
            Page::Animation | Page::Export => Self::default(),
            // The disk over the project: media storage, and under it the bins beside the pool,
            // which lists what the bin that is open holds.
            Page::Media => {
                let project = split(false, SplitSize::FixedA(220.0), tabs(&[Panel::Bin]), tabs(&[Panel::Pool]));
                // The preview at the top right, beside the disk (the user's of 2026-10-26).
                let top = split(false, SplitSize::Ratio(0.5), tabs(&[Panel::Storage]), tabs(&[Panel::Preview]));
                split(true, SplitSize::Ratio(0.42), top, project)
            }
            // The viewer over the storyboard timeline, which is the page's one timeline; on the
            // right the layers of the shot under the playhead, and under them the storyboard
            // as cards, with what is written of the panel under the playhead.
            Page::Storyboard => {
                let centre = split(true, SplitSize::FixedB(220.0), tabs(&[Panel::Viewport]), tabs(&[Panel::Storyboard]));
                let tools = tabs(&[Panel::Panels, Panel::ToolProperties]);
                let sidebar = split(true, SplitSize::Ratio(0.45), tabs(&[Panel::Layers, Panel::Palette, Panel::Brushes, Panel::OnionSkin]), tools);
                split(false, SplitSize::FixedB(250.0), centre, sidebar)
            }
        }
    }

    /// Every panel in the tree.
    pub fn panels(&self) -> Vec<Panel> {
        match self {
            DockNode::Split { a, b, .. } => [a.panels(), b.panels()].concat(),
            DockNode::Tabs { panels, .. } => panels.clone(),
        }
    }

    pub fn contains(&self, panel: Panel) -> bool {
        self.panels().contains(&panel)
    }

    /// Make `panel` the tab its group shows. False if it isn't in the tree.
    pub fn activate(&mut self, panel: Panel) -> bool {
        match self {
            DockNode::Split { a, b, .. } => a.activate(panel) || b.activate(panel),
            DockNode::Tabs { panels, active } => match panels.iter().position(|p| *p == panel) {
                Some(index) => {
                    *active = index;
                    true
                }
                None => false,
            },
        }
    }

    /// Whether `panel` is the tab its group shows.
    pub fn shows(&self, panel: Panel) -> bool {
        match self {
            DockNode::Split { a, b, .. } => a.shows(panel) || b.shows(panel),
            DockNode::Tabs { panels, active } => panels.get(*active) == Some(&panel),
        }
    }

    /// Close a panel: its tab goes, and a group left empty gives its place to its neighbour.
    pub fn close(&mut self, panel: Panel) {
        match self {
            DockNode::Split { a, b, .. } => {
                a.close(panel);
                b.close(panel);
                let empty = |node: &DockNode| matches!(node, DockNode::Tabs { panels, .. } if panels.is_empty());
                if empty(a) {
                    *self = (**b).clone();
                } else if empty(b) {
                    *self = (**a).clone();
                }
            }
            DockNode::Tabs { panels, active } => {
                // The group goes on showing the panel it showed, if that one stays.
                if let Some(index) = panels.iter().position(|p| *p == panel) {
                    panels.remove(index);
                    *active = (*active - usize::from(index < *active)).min(panels.len().saturating_sub(1));
                }
            }
        }
    }

    /// Open a panel as a tab beside `near`, or in the last group if `near` isn't open. A panel
    /// that is open already is brought to the front.
    pub fn open_near(&mut self, panel: Panel, near: Panel) {
        if self.activate(panel) {
            return;
        }
        fn add(node: &mut DockNode, panel: Panel, near: Option<Panel>) -> bool {
            match node {
                DockNode::Split { a, b, .. } => add(b, panel, near) || add(a, panel, near),
                DockNode::Tabs { panels, active } if near.is_none_or(|near| panels.contains(&near)) => {
                    panels.push(panel);
                    *active = panels.len() - 1;
                    true
                }
                DockNode::Tabs { .. } => false,
            }
        }
        if !add(self, panel, Some(near)) {
            add(self, panel, None);
        }
    }

    /// Put `panel` relative to the group that holds `anchor`: as a tab after it (centre), or in
    /// a new group split off to one side. It is taken from where it was first. False, with the
    /// tree unchanged, if `anchor` isn't in the tree or is `panel` itself.
    pub fn dock_panel(&mut self, panel: Panel, anchor: Panel, zone: Zone) -> bool {
        if panel == anchor || !self.contains(anchor) {
            return false;
        }
        self.close(panel);
        fn place(node: &mut DockNode, panel: Panel, anchor: Panel, zone: Zone) -> bool {
            match (zone, &mut *node) {
                (_, DockNode::Split { a, b, .. }) => place(a, panel, anchor, zone) || place(b, panel, anchor, zone),
                (_, DockNode::Tabs { panels, .. }) if !panels.contains(&anchor) => false,
                (Zone::Center, DockNode::Tabs { panels, active }) => {
                    let index = panels.iter().position(|p| *p == anchor).map_or(panels.len(), |i| i + 1);
                    panels.insert(index, panel);
                    *active = index;
                    true
                }
                (zone, node) => {
                    let (new, old) = (tabs(&[panel]), std::mem::replace(node, tabs(&[])));
                    let (a, b) = if matches!(zone, Zone::Left | Zone::Top) { (new, old) } else { (old, new) };
                    *node = split(matches!(zone, Zone::Top | Zone::Bottom), SplitSize::Ratio(0.5), a, b);
                    true
                }
            }
        }
        place(self, panel, anchor, zone)
    }

    /// Move `panel` into the group that holds `anchor`, as the tab before `before` or the last,
    /// and show it: where a tab dropped on a tab strip lands.
    pub fn insert_tab(&mut self, panel: Panel, anchor: Panel, before: Option<Panel>) -> bool {
        if panel == anchor || before == Some(panel) || !self.contains(anchor) {
            return false;
        }
        self.close(panel);
        fn place(node: &mut DockNode, panel: Panel, anchor: Panel, before: Option<Panel>) -> bool {
            match node {
                DockNode::Split { a, b, .. } => place(a, panel, anchor, before) || place(b, panel, anchor, before),
                DockNode::Tabs { panels, active } if panels.contains(&anchor) => {
                    let index = before.and_then(|b| panels.iter().position(|p| *p == b)).unwrap_or(panels.len());
                    panels.insert(index, panel);
                    *active = index;
                    true
                }
                DockNode::Tabs { .. } => false,
            }
        }
        place(self, panel, anchor, before)
    }
}

/// Where a dragged tab lands on a group: as a tab (centre) or split off to one side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

/// The zone for a pointer at `pos` over a group: its tab strip and its middle are the centre;
/// within a quarter of its size from an edge, that edge.
pub fn drop_zone(rect: Rect, pos: Pos2) -> Zone {
    if pos.y < rect.min.y + TAB_HEIGHT {
        return Zone::Center;
    }
    let fx = ((pos.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
    let fy = ((pos.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    let edges = [(fx, Zone::Left), (1.0 - fx, Zone::Right), (fy, Zone::Top), (1.0 - fy, Zone::Bottom)];
    let (distance, zone) = edges.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap_or((1.0, Zone::Center));
    if distance > 0.25 { Zone::Center } else { zone }
}

/// The part of a group that a drop into `zone` would take: what is highlighted during a drag.
pub fn zone_rect(rect: Rect, zone: Zone) -> Rect {
    let c = rect.center();
    match zone {
        Zone::Center => rect,
        Zone::Left => Rect::from_min_max(rect.min, pos2(c.x, rect.max.y)),
        Zone::Right => Rect::from_min_max(pos2(c.x, rect.min.y), rect.max),
        Zone::Top => Rect::from_min_max(rect.min, pos2(rect.max.x, c.y)),
        Zone::Bottom => Rect::from_min_max(pos2(rect.min.x, c.y), rect.max),
    }
}

/// The height of a group's tab strip.
pub const TAB_HEIGHT: f32 = 24.0;

/// The gap between groups, which drags to resize them.
const GAP: f32 = 3.0;

/// One tab group, laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// The whole group, tab strip included.
    pub rect: Rect,
    /// Where the showing panel draws.
    pub content: Rect,
    pub panels: Vec<Panel>,
    pub active: usize,
}

impl Group {
    pub fn showing(&self) -> Option<Panel> {
        self.panels.get(self.active).copied()
    }
}

/// Lay the tree out in `rect` and let the gaps between groups be dragged. `path` tells the gaps
/// of the tree apart.
pub fn layout(ui: &mut egui::Ui, node: &mut DockNode, rect: Rect, path: &str, out: &mut Vec<Group>) {
    match node {
        DockNode::Tabs { panels, active } => {
            let content = Rect::from_min_max(pos2(rect.min.x, (rect.min.y + TAB_HEIGHT).min(rect.max.y)), rect.max);
            out.push(Group { rect, content, panels: panels.clone(), active: *active });
        }
        DockNode::Split { vertical, size, a, b } => {
            let total = if *vertical { rect.height() } else { rect.width() };
            let available = (total - GAP).max(0.0);
            let wanted = match *size {
                SplitSize::Ratio(ratio) => available * ratio,
                SplitSize::FixedA(points) => points,
                SplitSize::FixedB(points) => available - points,
            };
            let first = split_first(wanted, available);
            let cut = |from: f32, to: f32| match *vertical {
                true => Rect::from_min_max(pos2(rect.min.x, rect.min.y + from), pos2(rect.max.x, rect.min.y + to)),
                false => Rect::from_min_max(pos2(rect.min.x + from, rect.min.y), pos2(rect.min.x + to, rect.max.y)),
            };
            if let Some(by) = gutter(ui, cut(first, first + GAP), *vertical, path) {
                let first = split_first(first + by, available);
                *size = match *size {
                    SplitSize::Ratio(_) => SplitSize::Ratio(first / available.max(1.0)),
                    SplitSize::FixedA(_) => SplitSize::FixedA(first),
                    SplitSize::FixedB(_) => SplitSize::FixedB(available - first),
                };
            }
            layout(ui, a, cut(0.0, first), &format!("{path}a"), out);
            layout(ui, b, cut(first + GAP, total), &format!("{path}b"), out);
        }
    }
}

/// Where a split of `available` points puts its gap: each side keeps 40 points when there is room.
/// When there is not, the first side gets what there is, and a size that is no number gives 40.
/// `f32::clamp` panics when its bounds cross, which a split under 80 points made them do.
fn split_first(first: f32, available: f32) -> f32 {
    let available = if available.is_finite() { available.max(0.0) } else { 0.0 };
    let low = 40f32.min(available);
    let high = (available - 40.0).max(low);
    if first.is_finite() { first.clamp(low, high) } else { low }
}

/// A gap between groups, a little easier to hit than it is wide. Returns how far it was dragged
/// this frame.
fn gutter(ui: &mut egui::Ui, gap: Rect, vertical: bool, path: &str) -> Option<f32> {
    let hit = gap.expand2(if vertical { vec2(0.0, 2.0) } else { vec2(2.0, 0.0) });
    let response = ui.interact(hit, egui::Id::new(("dock-gutter", path)), Sense::drag());
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(if vertical { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
        ui.painter().rect_filled(gap, 0.0, Theme::of(ui.ctx()).accent.gamma_multiply(if response.dragged() { 0.9 } else { 0.4 }));
    }
    response.dragged().then(|| if vertical { response.drag_delta().y } else { response.drag_delta().x })
}

/// What a tab is wider than its panel's name: room on the left, and for the cross that closes it.
const TAB_PADDING: f32 = 34.0;
/// The width of the button that lists a stack's panels when not all have a tab.
const MORE_WIDTH: f32 = 24.0;

/// Which of a stack's panels get a tab, by index, when the tabs are `widths` wide and the strip
/// has `room`: all, if they fit. Else the showing one always, and the others in their order
/// for as long as each fits whole beside the button that lists them all.
pub fn fitting(widths: &[f32], active: usize, room: f32) -> Vec<usize> {
    let gaps = widths.len().saturating_sub(1) as f32;
    if widths.iter().sum::<f32>() + gaps <= room {
        return (0..widths.len()).collect();
    }
    let mut left = room - MORE_WIDTH - widths.get(active).copied().unwrap_or(0.0);
    let mut shown = Vec::new();
    for (index, width) in widths.iter().enumerate() {
        if index == active {
            shown.push(index);
        } else if width + 1.0 <= left {
            left -= width + 1.0;
            shown.push(index);
        }
    }
    shown
}

/// What the user did on a group's tab strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabAction {
    Activate(Panel),
    Close(Panel),
    /// A tab began to be dragged, to dock it elsewhere.
    Drag(Panel),
    /// Give the panel a window of its own.
    Detach(Panel),
    /// Put a part that may go in any bar, by its id, in the panel's bar.
    AddPart(Panel, &'static str),
}

/// Draw a group's tab strip. Returns what was done on it, and where each tab is.
pub fn tab_strip(ui: &mut egui::Ui, group: &Group) -> (Vec<TabAction>, Vec<(Panel, Rect)>) {
    let theme = Theme::of(ui.ctx());
    let strip = Rect::from_min_max(group.rect.min, pos2(group.rect.max.x, group.content.min.y));
    let painter = ui.painter_at(strip);
    painter.rect_filled(strip, 0.0, crate::theme::mix(theme.surface, theme.text, 0.04));
    let (mut actions, mut placed) = (Vec::new(), Vec::new());
    let font = egui::FontId::proportional(12.0);
    // Each panel's name, laid out, and so how wide its tab is.
    let name = |(index, panel): (usize, &Panel)| {
        painter.layout_no_wrap(panel.title().to_string(), font.clone(), if index == group.active { theme.text } else { theme.text_muted })
    };
    let names: Vec<_> = group.panels.iter().enumerate().map(name).collect();
    let widths: Vec<f32> = names.iter().map(|name| name.size().x + TAB_PADDING).collect();
    let with_tabs = fitting(&widths, group.active, strip.width());
    let mut x = strip.min.x;
    for (index, (panel, text)) in group.panels.iter().copied().zip(names).enumerate().filter(|(index, _)| with_tabs.contains(index)) {
        let active = index == group.active;
        let tab = Rect::from_min_size(pos2(x, strip.min.y), vec2(widths[index], strip.height()));
        x = tab.max.x + 1.0;
        placed.push((panel, tab));
        let response = ui.interact(tab, egui::Id::new(("dock-tab", panel)), Sense::click_and_drag());
        if active {
            painter.rect_filled(tab, 0.0, theme.surface);
            painter.hline(tab.x_range(), tab.min.y + 0.5, Stroke::new(1.0, theme.accent));
        }
        painter.galley(pos2(tab.min.x + 10.0, tab.center().y - text.size().y / 2.0), text, theme.text);
        // The cross to close the panel shows while the pointer is on its tab.
        let cross = Rect::from_center_size(pos2(tab.max.x - 12.0, tab.center().y), vec2(14.0, 14.0));
        let on_cross = response.hovered() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|pos| cross.contains(pos));
        if response.hovered() {
            painter.text(cross.center(), Align2::CENTER_CENTER, "×", font.clone(), if on_cross { theme.text } else { theme.text_muted });
        }
        if response.clicked() {
            actions.push(if on_cross { TabAction::Close(panel) } else { TabAction::Activate(panel) });
        }
        if response.drag_started() {
            actions.push(TabAction::Drag(panel));
        }
        response.context_menu(|ui| {
            if ui.button("Detach into Its Own Window").clicked() {
                actions.push(TabAction::Detach(panel));
            }
            // The parts any bar may have, for a panel that has no bar of its own too; the
            // timelines have them in their bars' `…` menus.
            if !matches!(panel, Panel::Timeline | Panel::Storyboard) {
                ui.menu_button("Add to Bar", |ui| {
                    let theirs = crate::bar::of(panel);
                    let parts = crate::bar::PARTS.iter().filter(|part| crate::bar::ANYWHERE.contains(&part.id) && !theirs.iter().any(|had| had.id == part.id));
                    for part in parts {
                        if ui.button(part.title).on_hover_text("Put it at the end of this panel's bar").clicked() {
                            actions.push(TabAction::AddPart(panel, part.id));
                            ui.close();
                        }
                    }
                });
            }
            if ui.button("Close").clicked() {
                actions.push(TabAction::Close(panel));
            }
        });
    }
    // Where tabs are left out, a button at the end lists every panel of the stack.
    if with_tabs.len() < group.panels.len() {
        let more = Rect::from_min_max(pos2(strip.max.x - MORE_WIDTH, strip.min.y), strip.max);
        ui.painter_at(strip).rect_filled(more, 0.0, crate::theme::mix(theme.surface, theme.text, 0.04));
        let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(more.shrink2(vec2(1.0, 2.0))).id_salt(("dock-more", group.panels.first().copied())));
        let hidden = group.panels.len() - with_tabs.len();
        let dots = crate::icons::image("ellipsis", 14.0, theme.text_muted);
        let button = inside.menu_image_button(dots, |ui| {
            for (index, panel) in group.panels.iter().copied().enumerate() {
                ui.horizontal(|ui| {
                    // The ones the stack has no room to show a tab of stand out.
                    let name = egui::RichText::new(panel.title());
                    let name = if with_tabs.contains(&index) { name } else { name.strong() };
                    if ui.radio(index == group.active, name).clicked() {
                        actions.push(TabAction::Activate(panel));
                        ui.close();
                    }
                    if ui.small_button("×").on_hover_text("Close the panel").clicked() {
                        actions.push(TabAction::Close(panel));
                        ui.close();
                    }
                });
            }
        });
        button.response.on_hover_text(format!("{hidden} more: every panel of this stack"));
    }
    painter.hline(strip.x_range(), strip.max.y - 0.5, Stroke::new(1.0, theme.border));
    (actions, placed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Panel::*;

    /// The tree written out: `[A B*]` is a group with B showing, `|` a side-by-side split and
    /// `/` one above the other.
    fn shape(node: &DockNode) -> String {
        match node {
            DockNode::Split { vertical, a, b, .. } => format!("({} {} {})", shape(a), if *vertical { "/" } else { "|" }, shape(b)),
            DockNode::Tabs { panels, active } => {
                let names: Vec<String> = panels.iter().enumerate().map(|(i, p)| format!("{p:?}{}", if i == *active { "*" } else { "" })).collect();
                format!("[{}]", names.join(" "))
            }
        }
    }

    #[test]
    fn a_split_too_small_for_both_sides_lays_out() {
        // The bounds of a split under 80 points crossed, and the layout panicked.
        for available in [0.0, 5.0, 39.0, 40.0, 50.0, 79.0, 80.0, 200.0, -10.0, f32::NAN, f32::INFINITY] {
            for first in [0.0, 25.0, 300.0, -5.0, f32::NAN] {
                let at = split_first(first, available);
                assert!(at.is_finite() && at >= 0.0, "{first} of {available}: {at}");
                if available.is_finite() && available > 0.0 {
                    assert!(at <= available, "{first} of {available}: {at}");
                }
            }
        }
        assert_eq!(split_first(25.0, 50.0), 40.0);
        assert_eq!(split_first(100.0, 200.0), 100.0);
        let ctx = egui::Context::default();
        for page in Page::ALL {
            for (width, height) in [(30.0, 30.0), (800.0, 50.0), (800.0, 90.0), (50.0, 600.0), (1.0, 1.0), (0.0, 0.0)] {
                let mut node = DockNode::for_page(page);
                ctx.run_ui(egui::RawInput::default(), |ui| {
                    let mut out = Vec::new();
                    layout(ui, &mut node, Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(width, height)), "", &mut out);
                    assert!(!out.is_empty(), "{page:?}");
                })
                .textures_delta
                .clear();
            }
        }
    }

    #[test]
    fn the_default_layout_is_the_plans() {
        let layout = DockNode::default();
        assert_eq!(
            shape(&layout),
            "(([Viewport* NodeView TopView SideView] / [Timeline* Curves]) | ([Palette* Brushes OnionSkin] / [Properties* ToolProperties Substitution Library History]))"
        );
        assert_eq!(shape(&DockNode::for_page(Page::Media)), "(([Storage*] | [Preview*]) / ([Bin*] | [Pool*]))");
        assert_eq!(
            shape(&DockNode::for_page(Page::Storyboard)),
            "(([Viewport*] / [Storyboard*]) | ([Layers* Palette Brushes OnionSkin] / [Panels* ToolProperties]))"
        );
        let on_some_page: Vec<Panel> = Page::ALL.iter().flat_map(|p| DockNode::for_page(*p).panels()).collect();
        // The Text panel opens with the Text tool, the Script Console from the Scripts menu, the
        // Retake Notes from the Animation menu.
        let opened = [Panel::Log, Panel::Text, Panel::ScriptConsole, Panel::Notes];
        assert!(Panel::ALL.iter().all(|p| opened.contains(p) || on_some_page.contains(p)), "every panel but those opened starts out on some page");
        assert!(!DockNode::for_page(Page::Storyboard).contains(Timeline), "the storyboard timeline is that page's only timeline");
    }

    /// The Animation page with Palette, Onion Skin and History in the sidebar's upper stack.
    fn with_history_above() -> DockNode {
        let mut layout = DockNode::default();
        layout.close(Brushes);
        layout.close(History);
        assert!(layout.insert_tab(History, OnionSkin, None) && layout.activate(Palette));
        layout
    }

    #[test]
    fn closing_a_groups_last_panel_gives_its_place_away_and_opening_brings_it_back() {
        // The sidebar's tabs, with the timeline alone under the viewport.
        let mut layout = with_history_above();
        layout.close(Curves);
        layout.close(ToolProperties);
        layout.close(Substitution);
        layout.close(Library);
        layout.close(NodeView);
        layout.close(TopView);
        layout.close(SideView);
        assert!(layout.activate(History));
        assert!(layout.shows(History) && !layout.shows(Palette));
        layout.close(History);
        assert_eq!(shape(&layout), "(([Viewport*] / [Timeline*]) | ([Palette OnionSkin*] / [Properties*]))");
        layout.close(Palette);
        assert_eq!(
            shape(&layout),
            "(([Viewport*] / [Timeline*]) | ([OnionSkin*] / [Properties*]))",
            "closing a tab before the showing one keeps that one showing"
        );
        layout.open_near(Palette, OnionSkin);
        layout.dock_panel(Palette, OnionSkin, Zone::Center);
        layout.insert_tab(Palette, OnionSkin, Some(OnionSkin));
        layout.activate(OnionSkin);
        layout.close(Timeline);
        assert_eq!(shape(&layout), "([Viewport*] | ([Palette OnionSkin*] / [Properties*]))", "the viewport takes the timeline's place");
        layout.open_near(History, Palette);
        assert_eq!(shape(&layout), "([Viewport*] | ([Palette OnionSkin History*] / [Properties*]))");
        layout.open_near(Palette, Viewport);
        assert_eq!(shape(&layout), "([Viewport*] | ([Palette* OnionSkin History] / [Properties*]))", "a panel that is open comes to the front");
        layout.close(Palette);
        layout.close(OnionSkin);
        layout.close(History);
        layout.open_near(Timeline, Palette);
        assert_eq!(shape(&layout), "([Viewport*] | [Properties Timeline*])", "with its neighbour gone, into the last group");
    }

    #[test]
    fn a_dropped_tab_joins_a_group_or_splits_it() {
        let mut layout = with_history_above();
        layout.close(Curves);
        layout.close(ToolProperties);
        layout.close(Substitution);
        layout.close(Library);
        layout.close(NodeView);
        layout.close(TopView);
        layout.close(SideView);
        assert!(layout.dock_panel(History, Timeline, Zone::Right));
        assert_eq!(shape(&layout), "(([Viewport*] / ([Timeline*] | [History*])) | ([Palette* OnionSkin] / [Properties*]))");
        assert!(layout.dock_panel(OnionSkin, Viewport, Zone::Top));
        assert_eq!(shape(&layout), "((([OnionSkin*] / [Viewport*]) / ([Timeline*] | [History*])) | ([Palette*] / [Properties*]))");
        assert!(layout.dock_panel(Palette, Timeline, Zone::Center));
        assert_eq!(shape(&layout), "((([OnionSkin*] / [Viewport*]) / ([Timeline Palette*] | [History*])) | [Properties*])", "the emptied group is gone");
        assert!(layout.insert_tab(History, Palette, Some(Timeline)));
        assert_eq!(shape(&layout), "((([OnionSkin*] / [Viewport*]) / [History* Timeline Palette]) | [Properties*])");

        let before = layout.clone();
        assert!(!layout.dock_panel(Palette, Palette, Zone::Left) && !layout.insert_tab(Palette, Timeline, Some(Palette)));
        layout.close(OnionSkin);
        assert!(!layout.dock_panel(Palette, OnionSkin, Zone::Left), "nowhere to drop it");
        layout.open_near(OnionSkin, Viewport);
        assert_eq!(layout.panels().len(), before.panels().len());
    }

    #[test]
    fn the_edges_of_a_group_split_it_and_its_middle_and_tabs_join_it() {
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(400.0, 200.0));
        let zone = |x, y| drop_zone(rect, pos2(x, y));
        assert_eq!((zone(300.0, 210.0), zone(300.0, 110.0)), (Zone::Center, Zone::Center), "the middle, and the tab strip");
        assert_eq!((zone(120.0, 200.0), zone(480.0, 200.0), zone(300.0, 140.0), zone(300.0, 290.0)), (Zone::Left, Zone::Right, Zone::Top, Zone::Bottom));
        assert_eq!(zone_rect(rect, Zone::Right), Rect::from_min_max(pos2(300.0, 100.0), rect.max));
        assert_eq!(zone_rect(rect, Zone::Center), rect);
    }

    #[test]
    fn tabs_that_do_not_fit_are_left_out_but_never_the_showing_one() {
        let widths = [80.0, 100.0, 80.0];
        assert_eq!(fitting(&widths, 0, 262.0), [0, 1, 2], "all, when there is room for all with the gaps between");
        // Too narrow by a point: the list's button takes room too, and the last tab goes.
        assert_eq!(fitting(&widths, 0, 261.0), [0, 1]);
        assert_eq!(fitting(&widths, 2, 261.0), [0, 2], "the showing one stays, and what fits before it");
        assert_eq!(fitting(&widths, 1, 150.0), [1]);
        assert_eq!(fitting(&widths, 1, 40.0), [1], "even where it has no room itself");
        assert_eq!(fitting(&widths, 0, 200.0), [0, 2], "a narrower tab further on can fit where a wide one did not");
        assert_eq!(fitting(&[], 0, 100.0), Vec::<usize>::new());
    }

    #[test]
    fn a_layout_is_saved_as_data() {
        let mut layout = DockNode::default();
        layout.dock_panel(History, Timeline, Zone::Right);
        let json = serde_json::to_string(&layout).unwrap();
        assert!(json.contains("\"onionSkin\"") && json.contains("\"fixedB\""), "{json}");
        assert_eq!(serde_json::from_str::<DockNode>(&json).unwrap(), layout);
    }
}
