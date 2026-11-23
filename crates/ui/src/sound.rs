//! The sounds the app has read (R7). A sound media item refers to a file; to be heard, the
//! file is read whole, once, and kept while the project has the item.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iskra_audio::Sound;
use iskra_model::{Media, MediaId, MediaKind};

/// Where a timeline would lay sound dragged from the media pool (plan §7.2): what is seen of
/// it, and how a point in it is a frame and a sound track. Each timeline says so as it is
/// drawn, for the frame it is drawn on.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundDrop {
    /// The storyboard whose sound tracks they are.
    pub storyboard: iskra_model::StoryboardId,
    /// What is seen of the timeline.
    pub area: egui::Rect,
    /// Where frame `first` is, and how wide a frame is.
    pub zero: f32,
    pub frame_width: f32,
    /// The frame of the film at `zero`, and the last frame a point can be.
    pub first: i64,
    pub last: Option<u32>,
    /// The top of the sound rows, how high each is, and the storyboard's track each is.
    pub sound_top: f32,
    pub row_height: f32,
    pub tracks: Vec<usize>,
    /// How many sound tracks the storyboard has: below the last row is a new one.
    pub count: usize,
}

impl SoundDrop {
    /// The frame of the film and the track a sound let go of at `pos` is laid on: the track
    /// of the row under it; below the rows, a new one; above them, the first that has room.
    pub fn at(&self, pos: egui::Pos2) -> Option<(u32, Option<usize>)> {
        if !self.area.contains(pos) {
            return None;
        }
        let mut frame = ((pos.x - self.zero) / self.frame_width).floor().max(0.0) as u32;
        if let Some(last) = self.last {
            frame = frame.min(last);
        }
        let film = u32::try_from(self.first + i64::from(frame)).ok()?;
        let row = (pos.y >= self.sound_top).then(|| ((pos.y - self.sound_top) / self.row_height) as usize);
        Some((film, row.map(|row| self.tracks.get(row).copied().unwrap_or(self.count))))
    }
}

#[derive(Default)]
pub struct Sounds {
    /// What was read of each item, from which file. `None` for a file that could not be read,
    /// so that it is not tried again on every frame.
    read: HashMap<MediaId, (PathBuf, Option<Arc<Sound>>)>,
    /// A waveform's lows and highs for each item, at the fineness they were asked for.
    peaks: HashMap<(MediaId, u64), Arc<Vec<[f32; 2]>>>,
}

impl Sounds {
    /// The samples of a sound item of a project whose file is in `folder`. The first time it
    /// is asked for, its file is read.
    pub fn get(&mut self, media: &Media, folder: Option<&Path>) -> Option<Arc<Sound>> {
        if !matches!(media.kind, MediaKind::Sound { .. }) {
            return None;
        }
        let file = media.file(folder);
        if self.read.get(&media.id).is_none_or(|(from, _)| *from != file) {
            let sound = std::fs::read(&file).ok().and_then(|bytes| iskra_audio::read_wav(&bytes).ok()).map(Arc::new);
            self.read.insert(media.id, (file, sound));
            self.peaks.retain(|(of, _), _| *of != media.id);
        }
        self.read.get(&media.id)?.1.clone()
    }

    /// A sound item's waveform: the lowest and highest value of each stretch of `seconds`.
    pub fn peaks(&mut self, media: &Media, folder: Option<&Path>, seconds: f64) -> Option<Arc<Vec<[f32; 2]>>> {
        let sound = self.get(media, folder)?;
        let key = (media.id, seconds.to_bits());
        Some(self.peaks.entry(key).or_insert_with(|| Arc::new(sound.peaks(seconds))).clone())
    }

    /// Let go of the sounds of items the project no longer has.
    pub fn keep(&mut self, has: impl Fn(MediaId) -> bool) {
        self.peaks.retain(|(id, _), _| has(*id));
        self.read.retain(|id, _| has(*id));
    }
}

/// A slider for how loud something is, from silent to twice as loud, shown in percent. While
/// it is dragged the value is kept in `edit`, under `key`, which says what is being set, and
/// nothing else changes; let go, the volume to set is given back, if it is another than `now`.
/// So a drag is one step of the history.
pub(crate) fn volume_slider(ui: &mut egui::Ui, edit: &mut Option<(u64, f32)>, key: u64, now: f32) -> Option<f32> {
    let mut value = edit.filter(|(of, _)| *of == key).map_or(now, |(_, value)| value);
    let percent = egui::Slider::new(&mut value, 0.0..=2.0)
        .custom_formatter(|value, _| format!("{:.0}%", value * 100.0))
        .custom_parser(|typed| typed.trim().trim_end_matches('%').trim().parse::<f64>().ok().map(|percent| percent / 100.0));
    let slider = ui.add(percent);
    if slider.changed() {
        *edit = Some((key, value));
    }
    if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
        return edit.take().filter(|(of, _)| *of == key).map(|(_, value)| value).filter(|value| *value != now);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::SoundDrop;
    use egui::{Pos2, Rect};

    #[test]
    fn a_sound_let_go_of_below_the_rows_goes_on_a_new_track() {
        // The shot's timeline shows two of the storyboard's four tracks, from y 100 down.
        let drop = SoundDrop {
            storyboard: iskra_model::StoryboardId(1),
            area: Rect::from_min_max(Pos2::ZERO, Pos2::new(400.0, 300.0)),
            zero: 0.0,
            frame_width: 10.0,
            first: 50,
            last: Some(20),
            sound_top: 100.0,
            row_height: 20.0,
            tracks: vec![1, 3],
            count: 4,
        };
        assert_eq!(drop.at(Pos2::new(35.0, 50.0)), Some((53, None)), "above the rows, the first with room");
        assert_eq!(drop.at(Pos2::new(35.0, 125.0)), Some((53, Some(3))), "on a row, its track");
        assert_eq!(drop.at(Pos2::new(395.0, 150.0)), Some((70, Some(4))), "below them, a new one, at the shot's last frame at most");
        assert_eq!(drop.at(Pos2::new(35.0, 350.0)), None);
    }
}
