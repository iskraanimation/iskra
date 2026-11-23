//! A layer's exposure track: which drawing shows on each frame.

use serde::{Deserialize, Serialize};

use crate::DrawingId;

/// One drawing held for `len` frames from `start`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub start: u32,
    pub len: u32,
    pub drawing: DrawingId,
}

impl Run {
    /// The frame after the run's last.
    pub fn end(&self) -> u32 {
        self.start + self.len
    }
}

/// Frame → drawing, stored as runs: a hold of 100 frames is one entry. Frames no run covers are
/// empty cells. The runs are kept sorted, non-overlapping and non-empty, and neighbours that
/// show the same drawing are joined.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Exposure {
    runs: Vec<Run>,
}

impl Exposure {
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// The run that covers `frame`.
    pub fn run_at(&self, frame: u32) -> Option<Run> {
        let after = self.runs.partition_point(|r| r.start <= frame);
        self.runs[..after].last().copied().filter(|r| frame < r.end())
    }

    /// The drawing exposed at `frame`, or `None` for an empty cell.
    pub fn at(&self, frame: u32) -> Option<DrawingId> {
        self.run_at(frame).map(|r| r.drawing)
    }

    /// Expose `drawing` on the `len` frames from `start`, or empty those cells with `None`.
    pub fn set(&mut self, start: u32, len: u32, drawing: Option<DrawingId>) {
        let end = start.saturating_add(len);
        if end == start {
            return;
        }
        // What stays of each run: the part before `start` and the part from `end`.
        let mut runs = Vec::with_capacity(self.runs.len() + 2);
        for r in &self.runs {
            if r.start < start {
                runs.push(Run { len: r.end().min(start) - r.start, ..*r });
            }
            if r.end() > end {
                let from = r.start.max(end);
                runs.push(Run { start: from, len: r.end() - from, drawing: r.drawing });
            }
        }
        if let Some(drawing) = drawing {
            runs.push(Run { start, len: end - start, drawing });
        }
        runs.sort_by_key(|r| r.start);
        self.runs.clear();
        for r in runs {
            match self.runs.last_mut() {
                Some(last) if last.drawing == r.drawing && last.end() == r.start => last.len += r.len,
                _ => self.runs.push(r),
            }
        }
    }

    /// Make room for `len` frames after `frame`, moving everything later along. A drawing held
    /// on `frame` is held for `len` frames longer; in an empty cell this adds empty cells.
    pub fn insert(&mut self, frame: u32, len: u32) {
        for r in &mut self.runs {
            if r.start > frame {
                r.start = r.start.saturating_add(len);
            } else if r.end() > frame {
                r.len = r.len.saturating_add(len);
            }
        }
        // Whatever saturated past the last frame is dropped.
        self.runs.retain_mut(|r| {
            r.len = r.len.min(u32::MAX - r.start);
            r.len > 0
        });
    }

    /// Take out the `len` frames from `frame`, moving everything later back.
    pub fn remove(&mut self, frame: u32, len: u32) {
        let end = frame.saturating_add(len);
        self.set(frame, end - frame, None);
        for r in &mut self.runs {
            if r.start >= end {
                r.start -= end - frame;
            }
        }
        // The runs on either side of the gap may now touch and show the same drawing.
        let runs = std::mem::take(&mut self.runs);
        for r in runs {
            self.set(r.start, r.len, Some(r.drawing));
        }
    }

    /// The frame the first drawing after `frame` starts on: where a hold that begins on `frame`
    /// has to end if it is not to cover a later drawing.
    pub fn next_start(&self, frame: u32) -> Option<u32> {
        self.runs.get(self.runs.partition_point(|r| r.start <= frame)).map(|r| r.start)
    }

    /// Hold the last drawing before `frame` on to `frame`, over the empty cells between, when
    /// `frame` is an empty cell with a drawing before it. Nothing later moves. Returns whether
    /// a hold was made longer.
    pub fn extend_to(&mut self, frame: u32) -> bool {
        let Some(run) = self.held_before(frame) else { return false };
        self.set(run.end(), frame + 1 - run.end(), Some(run.drawing));
        true
    }

    /// The hold [`Exposure::extend_to`] would make longer to reach `frame`.
    pub fn held_before(&self, frame: u32) -> Option<Run> {
        self.runs[..self.runs.partition_point(|r| r.start <= frame)].last().copied().filter(|r| r.end() <= frame)
    }

    /// Hold the drawing that shows on `frame` for `len` frames in all. With `ripple` the later
    /// cells move along or back by the difference. Without it they stay: a longer hold covers
    /// them and a shorter one leaves empty cells. Returns whether a drawing shows on `frame`.
    pub fn set_hold(&mut self, frame: u32, len: u32, ripple: bool) -> bool {
        let Some(run) = self.run_at(frame).filter(|_| len > 0) else { return false };
        let len = len.min(u32::MAX - run.start);
        match (len > run.len, ripple) {
            (true, true) => self.insert(run.end() - 1, len - run.len),
            (true, false) => self.set(run.start, len, Some(run.drawing)),
            (false, true) => self.remove(run.start + len, run.len - len),
            (false, false) => self.set(run.start + len, run.len - len, None),
        }
        true
    }

    /// What the `len` frames from `start` show, as holds one after the other: how long each
    /// is and its drawing, or `None` for empty cells. Together they are `len` frames long.
    pub fn cells(&self, start: u32, len: u32) -> Vec<(u32, Option<DrawingId>)> {
        let end = start.saturating_add(len);
        let mut cells: Vec<(u32, Option<DrawingId>)> = Vec::new();
        let mut at = start;
        while at < end {
            // As far as the hold on this frame goes, or to the next drawing over empty cells.
            let (until, drawing) = match self.run_at(at) {
                Some(run) => (run.end(), Some(run.drawing)),
                None => (self.next_start(at).unwrap_or(end), None),
            };
            let until = until.min(end);
            cells.push((until - at, drawing));
            at = until;
        }
        cells
    }

    /// Lay `cells`, as [`Exposure::cells`] gives them, from `start` on, over what is there:
    /// empty cells empty what they cover.
    pub fn lay(&mut self, start: u32, cells: &[(u32, Option<DrawingId>)]) {
        let mut at = start;
        for &(len, drawing) in cells {
            self.set(at, len, drawing);
            at = at.saturating_add(len);
        }
    }

    /// Move the `len` frames from `start` so that they begin on `to`: where they were is
    /// emptied, and where they land they cover what was there.
    pub fn shift(&mut self, start: u32, len: u32, to: u32) {
        let cells = self.cells(start, len);
        self.set(start, len, None);
        self.lay(to, &cells);
    }

    /// Show `to` wherever `from` shows.
    pub fn replace_drawing(&mut self, from: DrawingId, to: DrawingId) {
        let runs = std::mem::take(&mut self.runs);
        for r in runs {
            self.set(r.start, r.len, Some(if r.drawing == from { to } else { r.drawing }));
        }
    }

    /// Empty every cell that shows `drawing`.
    pub fn remove_drawing(&mut self, drawing: DrawingId) {
        self.runs.retain(|r| r.drawing != drawing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: DrawingId = DrawingId(1);
    const B: DrawingId = DrawingId(2);

    fn runs(e: &Exposure) -> Vec<(u32, u32, DrawingId)> {
        e.runs().iter().map(|r| (r.start, r.len, r.drawing)).collect()
    }

    #[test]
    fn a_stretch_of_cells_is_read_laid_elsewhere_and_moved() {
        let mut e = Exposure::default();
        e.set(0, 4, Some(A));
        e.set(6, 4, Some(B));
        // Read from inside one hold, over the empty cells, into the next.
        let cells = e.cells(2, 6);
        assert_eq!(cells, [(2, Some(A)), (2, None), (2, Some(B))]);
        assert_eq!(e.cells(12, 3), [(3, None)]);
        assert!(e.cells(5, 0).is_empty());
        // Laid elsewhere it covers what is there, with its empty cells too.
        let mut laid = e.clone();
        laid.lay(5, &cells);
        assert_eq!(runs(&laid), [(0, 4, A), (5, 2, A), (9, 2, B)]);
        // Moved, the stretch leaves empty cells behind; onto itself by a little, nothing is lost.
        e.shift(6, 4, 12);
        assert_eq!(runs(&e), [(0, 4, A), (12, 4, B)]);
        e.shift(12, 4, 13);
        assert_eq!(runs(&e), [(0, 4, A), (13, 4, B)]);
        e.shift(0, 4, 11);
        assert_eq!(runs(&e), [(11, 4, A), (15, 2, B)]);
    }

    #[test]
    fn a_hold_is_extended_over_empty_cells_to_a_frame() {
        let mut e = Exposure::default();
        e.set(2, 2, Some(A));
        e.set(10, 2, Some(B));
        // From an empty cell, the hold before it comes on to it; the later drawing stays.
        assert!(e.extend_to(7));
        assert_eq!(runs(&e), [(2, 6, A), (10, 2, B)]);
        // Not on a drawing, nor where none is before.
        assert!(!e.extend_to(5) && !e.extend_to(11) && !e.extend_to(0));
        assert!(e.extend_to(9));
        assert_eq!(runs(&e), [(2, 8, A), (10, 2, B)]);
    }

    #[test]
    fn a_hold_is_one_run() {
        let mut e = Exposure::default();
        e.set(10, 100, Some(A));
        assert_eq!(runs(&e), [(10, 100, A)]);
        assert_eq!((e.at(9), e.at(10), e.at(109), e.at(110)), (None, Some(A), Some(A), None));
    }

    #[test]
    fn setting_inside_a_hold_splits_it() {
        let mut e = Exposure::default();
        e.set(0, 10, Some(A));
        e.set(4, 2, Some(B));
        assert_eq!(runs(&e), [(0, 4, A), (4, 2, B), (6, 4, A)]);
        e.set(4, 2, None);
        assert_eq!(runs(&e), [(0, 4, A), (6, 4, A)]);
        assert_eq!(e.at(5), None);
    }

    #[test]
    fn neighbours_showing_the_same_drawing_join() {
        let mut e = Exposure::default();
        e.set(0, 4, Some(A));
        e.set(6, 4, Some(A));
        e.set(4, 2, Some(A));
        assert_eq!(runs(&e), [(0, 10, A)]);
    }

    #[test]
    fn setting_over_several_runs_replaces_what_it_covers() {
        let mut e = Exposure::default();
        e.set(0, 3, Some(A));
        e.set(3, 3, Some(B));
        e.set(6, 3, Some(A));
        e.set(2, 5, Some(B));
        assert_eq!(runs(&e), [(0, 2, A), (2, 5, B), (7, 2, A)]);
        e.set(0, 0, Some(A));
        assert_eq!(runs(&e).len(), 3, "a zero-length set changes nothing");
        e.set(u32::MAX - 1, 5, Some(A));
        assert_eq!(e.runs().last().map(Run::end), Some(u32::MAX), "the end is clamped, not wrapped");
    }

    #[test]
    fn inserting_frames_lengthens_a_hold_and_moves_later_cells() {
        let mut e = Exposure::default();
        e.set(0, 2, Some(A));
        e.set(2, 2, Some(B));
        e.insert(1, 3);
        assert_eq!(runs(&e), [(0, 5, A), (5, 2, B)], "the hold under the frame grows");
        e.insert(4, 1);
        assert_eq!(runs(&e), [(0, 6, A), (6, 2, B)], "also on the hold's last frame");
        e.insert(20, 2);
        assert_eq!(runs(&e), [(0, 6, A), (6, 2, B)], "nothing later to move");
        e.set(0, 1, None);
        e.insert(0, 2);
        assert_eq!(runs(&e), [(3, 5, A), (8, 2, B)], "in an empty cell it adds empty cells");
        e.insert(3, u32::MAX);
        assert_eq!(runs(&e), [(3, u32::MAX - 3, A)], "what would pass the last frame is dropped");
    }

    #[test]
    fn removing_frames_moves_later_cells_back_and_joins_what_meets() {
        let mut e = Exposure::default();
        e.set(0, 4, Some(A));
        e.set(4, 2, Some(B));
        e.set(6, 4, Some(A));
        e.remove(1, 2);
        assert_eq!(runs(&e), [(0, 2, A), (2, 2, B), (4, 4, A)]);
        e.remove(2, 2);
        assert_eq!(runs(&e), [(0, 6, A)], "the two holds of A meet and join");
        e.remove(4, 100);
        assert_eq!(runs(&e), [(0, 4, A)]);
    }

    #[test]
    fn a_hold_is_set_with_or_without_moving_the_later_cells() {
        let board = || {
            let mut e = Exposure::default();
            e.set(0, 4, Some(A));
            e.set(4, 4, Some(B));
            e.set(10, 2, Some(A));
            e
        };
        let held = |frame, len, ripple| {
            let mut e = board();
            assert!(e.set_hold(frame, len, ripple));
            runs(&e)
        };
        // Any frame of the hold names it.
        assert_eq!(held(2, 6, true), [(0, 6, A), (6, 4, B), (12, 2, A)], "longer: the later cells move along");
        assert_eq!(held(0, 6, false), [(0, 6, A), (6, 2, B), (10, 2, A)], "longer: it covers what follows");
        assert_eq!(held(3, 1, true), [(0, 1, A), (1, 4, B), (7, 2, A)], "shorter: the later cells move back");
        assert_eq!(held(3, 1, false), [(0, 1, A), (4, 4, B), (10, 2, A)], "shorter: empty cells are left");
        assert_eq!(held(5, 7, false), [(0, 4, A), (4, 7, B), (11, 1, A)]);
        assert_eq!(held(5, 4, true), runs(&board()), "the same length changes nothing");

        let mut e = board();
        assert!(!e.set_hold(8, 3, true) && !e.set_hold(0, 0, true), "an empty cell has no hold, and a hold is at least a frame");
        assert_eq!(runs(&e), runs(&board()));
        assert_eq!((e.next_start(0), e.next_start(3), e.next_start(4), e.next_start(8), e.next_start(10)), (Some(4), Some(4), Some(10), Some(10), None));
    }

    #[test]
    fn replacing_a_drawing_keeps_the_timing() {
        let mut e = Exposure::default();
        e.set(0, 3, Some(A));
        e.set(3, 3, Some(B));
        e.set(8, 2, Some(A));
        e.replace_drawing(A, B);
        assert_eq!(runs(&e), [(0, 6, B), (8, 2, B)], "holds that now show the same drawing join");
    }

    #[test]
    fn removing_a_drawing_empties_its_cells() {
        let mut e = Exposure::default();
        e.set(0, 3, Some(A));
        e.set(3, 3, Some(B));
        e.remove_drawing(A);
        assert_eq!(runs(&e), [(3, 3, B)]);
    }
}
