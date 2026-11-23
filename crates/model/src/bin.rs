//! Bins (plan §7.11): the project's items in folders of the user's making, as an editor's media
//! pool has them. Every item is in one bin. The bin everything starts in, *Master*, is not
//! stored: an item, or a bin, that names no bin is in it.

use serde::{Deserialize, Serialize};

use crate::{BinId, MediaId, Project, ShotId, StoryboardId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bin {
    pub id: BinId,
    /// Unique among the bins of the bin it is in.
    pub name: String,
    /// The bin it is in. `None` is Master.
    pub parent: Option<BinId>,
}

/// An item of the project that is kept in a bin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Item {
    Shot(ShotId),
    Storyboard(StoryboardId),
    Media(MediaId),
}

impl Project {
    pub fn bin(&self, id: BinId) -> Option<&Bin> {
        self.bins.iter().find(|bin| bin.id == id)
    }

    /// `bin` as a bin of the project: itself if the project has it, else Master.
    pub fn bin_or_master(&self, bin: Option<BinId>) -> Option<BinId> {
        bin.filter(|id| self.bin(*id).is_some())
    }

    /// The bin an item is in; Master if it names none, or one that is gone.
    pub fn bin_of(&self, item: Item) -> Option<BinId> {
        let named = match item {
            Item::Shot(id) => self.shot(id)?.bin,
            Item::Storyboard(id) => self.storyboard(id)?.bin,
            Item::Media(id) => self.media(id)?.bin,
        };
        self.bin_or_master(named)
    }

    /// The bins that are in `parent`, in the order of their names.
    pub fn bins_in(&self, parent: Option<BinId>) -> Vec<&Bin> {
        let mut inside: Vec<&Bin> = self.bins.iter().filter(|bin| self.bin_or_master(bin.parent) == parent).collect();
        inside.sort_by_key(|bin| bin.name.to_lowercase());
        inside
    }

    /// Whether `inside` is `bin` or in it, however deep.
    pub fn bin_holds(&self, bin: BinId, inside: Option<BinId>) -> bool {
        // A tree is never deeper than it has bins; the limit only keeps bad data from looping.
        std::iter::successors(inside, |at| self.bin(*at)?.parent).take(self.bins.len() + 1).any(|at| at == bin)
    }

    /// The items in `bin`: the storyboards, then the shots, then the media, each as the
    /// project lists them.
    pub fn items_in(&self, bin: Option<BinId>) -> Vec<Item> {
        self.items_everywhere().into_iter().filter(|item| self.bin_of(*item) == bin).collect()
    }

    /// Put an item into a bin. `None` if the project has no such item.
    pub fn set_bin(&mut self, item: Item, bin: Option<BinId>) -> Option<()> {
        match item {
            Item::Shot(id) => self.shot_mut(id)?.bin = bin,
            Item::Storyboard(id) => self.storyboard_mut(id)?.bin = bin,
            Item::Media(id) => self.media_mut(id)?.bin = bin,
        }
        Some(())
    }

    /// Take a bin away. What was in it, items and bins, is then in the bin it was in.
    pub fn remove_bin(&mut self, id: BinId) -> Option<()> {
        let parent = self.bin(id)?.parent;
        self.bins.retain(|bin| bin.id != id);
        for bin in self.bins.iter_mut().filter(|bin| bin.parent == Some(id)) {
            bin.parent = parent;
        }
        let inside: Vec<Item> = self.items_everywhere().into_iter().filter(|item| self.item_names(*item) == Some(id)).collect();
        for item in inside {
            self.set_bin(item, parent);
        }
        Some(())
    }

    fn items_everywhere(&self) -> Vec<Item> {
        let boards = self.storyboards.iter().map(|board| Item::Storyboard(board.id));
        boards.chain(self.shots.iter().map(|shot| Item::Shot(shot.id))).chain(self.media.iter().map(|media| Item::Media(media.id))).collect()
    }

    /// The bin an item names, whether or not the project has it.
    fn item_names(&self, item: Item) -> Option<BinId> {
        match item {
            Item::Shot(id) => self.shot(id)?.bin,
            Item::Storyboard(id) => self.storyboard(id)?.bin,
            Item::Media(id) => self.media(id)?.bin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_is_in_one_bin_and_a_bin_that_goes_leaves_what_it_held_a_bin_up() {
        let mut project = Project::default();
        let (shot, board) = (Item::Shot(project.shots[0].id), Item::Storyboard(project.storyboards[0].id));
        assert_eq!((project.items_in(None), project.bin_of(shot)), (vec![board, shot], None), "all starts in Master");
        let [scenes, one, sound] = [BinId(project.new_id()), BinId(project.new_id()), BinId(project.new_id())];
        let bin = |id, name: &str, parent| Bin { id, name: name.to_string(), parent };
        project.bins = vec![bin(scenes, "Scenes", None), bin(one, "act one", Some(scenes)), bin(sound, "Audio", None)];
        project.set_bin(shot, Some(one)).unwrap();
        assert_eq!((project.items_in(None), project.items_in(Some(one)), project.bin_of(shot)), (vec![board], vec![shot], Some(one)));
        assert_eq!(project.bins_in(None).iter().map(|bin| bin.name.as_str()).collect::<Vec<_>>(), ["Audio", "Scenes"], "by name");
        assert!(
            project.bin_holds(scenes, Some(one))
                && project.bin_holds(one, Some(one))
                && !project.bin_holds(one, Some(scenes))
                && !project.bin_holds(scenes, None)
        );

        // The bin in the middle goes: its shot is in the bin it was in.
        project.remove_bin(one).unwrap();
        assert_eq!((project.bin_of(shot), project.bins.len()), (Some(scenes), 2));
        // And so are bins: with Scenes gone, what it held is in Master.
        project.bins.push(bin(one, "act one", Some(scenes)));
        project.remove_bin(scenes).unwrap();
        assert_eq!((project.bin_of(shot), project.bin(one).unwrap().parent), (None, None));
        // An item that names a bin the project lacks is in Master, not lost.
        project.set_bin(board, Some(BinId(999))).unwrap();
        assert_eq!((project.bin_of(board), project.items_in(None).len()), (None, 2));
        assert!(project.remove_bin(BinId(999)).is_none() && project.set_bin(Item::Shot(ShotId(999)), None).is_none());
    }
}
