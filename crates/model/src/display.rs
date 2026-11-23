//! A shot's displays (plan §8.6): each shows one picture of the shot's graph, and the viewer
//! is switched between them. One is the shot's main display, which every shot has from when it
//! is made: what the shot is, wherever it is not looked at in the viewer. The Export page
//! renders it, the Storyboard page and its thumbnails show it, the playback cache keeps it, and
//! a template that is placed stands it for the shot. The others are for looking at another
//! picture of the shot, a matte or a pass, in the viewer alone.
//!
//! The main display is [`Shot::display`], with its name in [`Shot::display_name`], and is a
//! node under the shot's own id; the others are in [`Shot::displays`], each under an id of its
//! own. Making another one the main one swaps the two over.

use serde::{Deserialize, Serialize};

use crate::{Shot, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DisplayId(pub u64);

/// A display that is not the shot's main one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Display {
    pub id: DisplayId,
    pub name: String,
    /// What it shows. Nothing is seen through it without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shows: Option<Source>,
    /// It is a *Pass* (plan §7.14), a node of its own to the user: what it shows is rendered
    /// apart on the Export page, under its name. It is never the main display.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pass: bool,
}

/// The name of a shot's main display, until it is given another.
pub const MAIN_DISPLAY: &str = "Display";

pub(crate) fn main_display_name() -> String {
    MAIN_DISPLAY.to_string()
}

pub(crate) fn is_main_display_name(name: &str) -> bool {
    name == MAIN_DISPLAY
}

impl Shot {
    /// The main display's id: the shot's own.
    pub fn main_display(&self) -> DisplayId {
        DisplayId(self.id.0)
    }

    /// Whether the shot has a display of that id, the main one or another.
    pub fn has_display(&self, id: DisplayId) -> bool {
        id == self.main_display() || self.displays.iter().any(|display| display.id == id)
    }

    /// What a display shows, by its id, if the shot has it.
    pub fn shown_by(&self, id: DisplayId) -> Option<Option<Source>> {
        if id == self.main_display() {
            return Some(self.display);
        }
        self.displays.iter().find(|display| display.id == id).map(|display| display.shows)
    }

    /// A name for a new display: free among the shot's nodes and its main display.
    pub fn free_display_name(&self, base: &str) -> String {
        crate::unique_name(base, self.node_names().chain([self.display_name.as_str()]))
    }

    /// A display's name, by its id.
    pub fn display_name_of(&self, id: DisplayId) -> Option<&str> {
        if id == self.main_display() {
            return Some(&self.display_name);
        }
        self.displays.iter().find(|display| display.id == id).map(|display| display.name.as_str())
    }

    /// Make a display show `shows`. Says whether the shot has it.
    pub fn show_on(&mut self, id: DisplayId, shows: Option<Source>) -> bool {
        if id == self.main_display() {
            self.display = shows;
            return true;
        }
        match self.displays.iter_mut().find(|display| display.id == id) {
            Some(display) => {
                display.shows = shows;
                true
            }
            None => false,
        }
    }

    /// Make another display the main one. The two change places: what each shows, its name,
    /// where it is in the node view and its colour, so that to the eye the main one's mark
    /// moves to the other box. Says whether the shot has it.
    pub fn make_main(&mut self, id: DisplayId) -> bool {
        let main = self.main_display();
        let Some(other) = self.displays.iter_mut().find(|display| display.id == id && !display.pass) else { return id == main };
        std::mem::swap(&mut self.display, &mut other.shows);
        std::mem::swap(&mut self.display_name, &mut other.name);
        swap_keys(&mut self.places, main.0, id.0);
        swap_keys(&mut self.colours, main.0, id.0);
        true
    }

    /// The shot as it is seen through one of its displays: with that one as its main one, so
    /// that what draws the shot draws what it shows. The shot itself if it is the main one, or
    /// the shot has no such display.
    pub fn seen_through(&self, id: DisplayId) -> Option<Shot> {
        let shows = self.displays.iter().find(|display| display.id == id)?.shows;
        Some(Shot { display: shows, ..self.clone() })
    }
}

/// What a map has under `a` put under `b`, and the other way.
fn swap_keys<V>(map: &mut std::collections::BTreeMap<u64, V>, a: u64, b: u64) {
    let (at_a, at_b) = (map.remove(&a), map.remove(&b));
    map.extend(at_a.map(|value| (b, value)).into_iter().chain(at_b.map(|value| (a, value))));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    #[test]
    fn a_shot_has_a_main_display_and_another_is_made_the_main_one_by_trading_places() {
        let mut project = Project::default();
        let mut shot = (*project.shots[0]).clone();
        let main = shot.main_display();
        let composite = shot.display;
        assert!(composite.is_some() && shot.display_name == MAIN_DISPLAY && shot.has_display(main));
        // A matte's display, showing the first layer alone.
        let matte = DisplayId(project.new_id());
        let layer = Source::Layer(shot.layers[0].id);
        shot.displays.push(Display { id: matte, name: "Matte".to_string(), shows: Some(layer), pass: false });
        shot.places.insert(matte.0, [5.0, 6.0]);
        assert_eq!((shot.shown_by(matte), shot.seen_through(matte).unwrap().display), (Some(Some(layer)), Some(layer)));
        assert!(shot.seen_through(main).is_none(), "the main one is the shot as it is");
        // Made the main one, the matte is what the shot shows, and the box with its name and
        // its place is the main one's.
        assert!(shot.make_main(matte));
        assert_eq!((shot.display, shot.display_name.as_str(), shot.places.get(&main.0)), (Some(layer), "Matte", Some(&[5.0, 6.0])));
        assert_eq!((shot.shown_by(matte), shot.display_name_of(matte)), (Some(composite), Some(MAIN_DISPLAY)));
        assert!(!shot.places.contains_key(&matte.0));
        assert!(!shot.make_main(DisplayId(987_654)) && shot.make_main(main));
    }
}
