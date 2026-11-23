//! Retake notes (plan §9.6): notes pinned to a shot's frames, for review, each open until it is
//! ticked done. They are the shot's, in its file, and go out and come back with it.

use serde::{Deserialize, Serialize};

use crate::Shot;

/// A note on a stretch of a shot's frames.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    /// Its id among the shot's notes.
    pub id: u64,
    /// The first frame it is on, and how many.
    pub frame: u32,
    #[serde(default = "one")]
    pub length: u32,
    pub text: String,
    /// Who wrote it: empty where that is not known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// When it was written, in seconds since 1970.
    #[serde(default)]
    pub written: u64,
    #[serde(default)]
    pub done: bool,
}

fn one() -> u32 {
    1
}

impl Note {
    /// Whether `other` is this note as written, whatever was done to it since: the same text,
    /// by the same, at the same time.
    pub fn same_as(&self, other: &Note) -> bool {
        (self.written, &self.author, &self.text) == (other.written, &other.author, &other.text)
    }

    /// The frame after its last.
    pub fn end(&self) -> u32 {
        self.frame + self.length.max(1)
    }
}

impl Shot {
    pub fn note(&self, id: u64) -> Option<&Note> {
        self.notes.iter().find(|note| note.id == id)
    }

    /// The id a new note of the shot's is given.
    pub fn new_note_id(&self) -> u64 {
        self.notes.iter().map(|note| note.id).max().map_or(1, |id| id + 1)
    }

    /// Put a note among the shot's, in place of the one of its id, in the order of their frames
    /// and then of when they were written.
    pub fn put_note(&mut self, note: Note) {
        self.notes.retain(|other| other.id != note.id);
        let key = |note: &Note| (note.frame, note.written, note.id);
        let at = self.notes.partition_point(|other| key(other) < key(&note));
        self.notes.insert(at, note);
    }

    /// The notes of `ours` that the shot does not have, given ids of its own: what was written
    /// in the studio while the shot was out, kept when it comes back.
    pub fn keep_notes(&mut self, ours: &[Note]) {
        let missing: Vec<&Note> = ours.iter().filter(|note| !self.notes.iter().any(|theirs| theirs.same_as(note))).collect();
        for note in missing {
            let id = self.new_note_id();
            self.put_note(Note { id, ..note.clone() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    fn note(id: u64, frame: u32, text: &str, written: u64) -> Note {
        Note { id, frame, length: 1, text: text.to_string(), author: "Ana".to_string(), written, done: false }
    }

    #[test]
    fn notes_are_in_frame_order_and_kept_from_both_sides_when_a_shot_comes_back() {
        let mut project = Project::default();
        let shot = std::sync::Arc::make_mut(&mut project.shots[0]);
        shot.put_note(note(1, 42, "arm pops", 10));
        shot.put_note(note(2, 3, "too slow", 20));
        shot.put_note(note(3, 42, "eyes", 5));
        assert_eq!(shot.notes.iter().map(|n| n.id).collect::<Vec<_>>(), [2, 3, 1]);
        assert_eq!(shot.new_note_id(), 4);
        // Back from the animator: one ticked done, one of theirs; ours has one written meanwhile.
        let mut back = shot.clone();
        back.notes[0].done = true;
        back.put_note(note(4, 7, "theirs", 30));
        let mut ours = shot.notes.clone();
        ours.push(note(4, 9, "written meanwhile", 40));
        back.keep_notes(&ours);
        let said: Vec<(u64, &str, bool)> = back.notes.iter().map(|n| (n.id, n.text.as_str(), n.done)).collect();
        assert_eq!(said, [(2, "too slow", true), (4, "theirs", false), (5, "written meanwhile", false), (3, "eyes", false), (1, "arm pops", false)]);
    }
}
