//! Groups (plan §4.3): some of a shot's nodes handled as one. A group is folded into a single
//! box in the node view, and apart from that into a single row on the timeline; it is saved to the library as a rig
//! template, and can be marked as the storyboard's, which makes all that is in it boards.
//!
//! A group changes nothing of the picture: the graph is as it is with or without it.
//!
//! There are no empty groups: one that holds nothing any more, but for its own ports, is gone
//! with them ([`Shot::drop_empty_groups`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{CompositeId, EffectId, LayerId, PORT_IN, PORT_OUT, Role, Shot, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    /// What is in it, by the ids of the shot's nodes: layers, pegs, composites, deformers and
    /// other groups. A node is in one group at most. The id of a node that was deleted may
    /// stay here, and names nothing.
    pub members: BTreeSet<u64>,
    /// Shown as one box in the node view, with what is in it put away.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub folded: bool,
    /// Shown as one row on the timeline, with the rows of what is in it put away. Apart from
    /// `folded`: a group opened in one panel stays as it was in the other.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rows_folded: bool,
    /// The storyboard's (plan §7.2c): all that is in it is boards, whatever a layer says of
    /// itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub board: bool,
    /// The library template the group is a copy from, if it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

/// Which of a shot's layers are meant (plan §7.2c): one shot holds two layers of work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Work {
    /// What the Storyboard page shows and plays, and an animatic is made of.
    Boards,
    /// What a render is made of.
    Animation,
    /// Both, as the Animation page shows the shot.
    #[default]
    Whole,
}

impl Shot {
    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().map(|group| &**group).find(|group| group.id == id)
    }

    /// Whether a node, by its id, is a port of a group (plan §8.6): a way in or out of it.
    pub fn is_port(&self, node: u64) -> bool {
        self.effect(EffectId(node)).is_some_and(|effect| effect.kind == PORT_IN || effect.kind == PORT_OUT)
    }

    /// Whether a group holds anything: a node of the shot that is not a port of its own.
    fn holds(&self, group: &Group) -> bool {
        group.members.iter().any(|member| !self.is_port(*member) && self.node_ids().any(|id| id == *member))
    }

    /// Whether the shot has a group that holds nothing, which [`Shot::drop_empty_groups`]
    /// would take away.
    pub fn has_empty_group(&self) -> bool {
        self.groups.iter().any(|group| !self.holds(group))
    }

    /// Take a group's ports away: each comes off its wire, which is joined again behind it,
    /// what was parented to it being carried by what came into it, and is no more.
    pub fn remove_ports(&mut self, group: GroupId) {
        let ports: Vec<u64> = self.group(group).into_iter().flat_map(|group| &group.members).copied().filter(|member| self.is_port(*member)).collect();
        for port in &ports {
            self.take_off(EffectId(*port));
            self.effects.retain(|effect| effect.id.0 != *port);
            self.places.remove(port);
            self.colours.remove(port);
        }
        if let (false, Some(group)) = (ports.is_empty(), self.group_mut(group)) {
            group.members.retain(|member| !ports.contains(member));
        }
    }

    /// Take away every group that holds nothing, with its ports: there are no empty groups.
    /// A group that held only such a group goes too.
    pub fn drop_empty_groups(&mut self) {
        while let Some(empty) = self.groups.iter().find(|group| !self.holds(group)).map(|group| group.id) {
            self.remove_ports(empty);
            self.groups.retain(|group| group.id != empty);
            self.places.remove(&empty.0);
            self.hidden.remove(&empty.0);
            for group in self.groups.iter_mut().filter(|group| group.members.contains(&empty.0)) {
                Arc::make_mut(group).members.remove(&empty.0);
            }
        }
    }

    pub fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        self.groups.iter_mut().find(|group| group.id == id).map(Arc::make_mut)
    }

    /// The group a node is in, by the node's id.
    /// Put a node, by its id, into a group, if there is one.
    pub fn add_to_group(&mut self, group: Option<GroupId>, node: u64) {
        if let Some(group) = self.groups.iter_mut().find(|own| Some(own.id) == group) {
            Arc::make_mut(group).members.insert(node);
        }
    }

    pub fn group_of(&self, node: u64) -> Option<&Group> {
        self.groups.iter().map(|group| &**group).find(|group| group.members.contains(&node))
    }

    /// The groups a node is in, the nearest first: its own, the one that is in, and so on.
    pub fn groups_above(&self, node: u64) -> impl Iterator<Item = &Group> {
        let mut seen = BTreeSet::new();
        std::iter::successors(self.group_of(node), move |group| self.group_of(group.id.0).filter(|above| seen.insert(above.id)))
    }

    /// The folded group a node is put away in, the outermost of them, if it is in one.
    pub fn folded_into(&self, node: u64) -> Option<GroupId> {
        self.groups_above(node).filter(|group| group.folded).last().map(|group| group.id)
    }

    /// Every node that is in a group, however deep, by its id; the groups in it too.
    pub fn group_nodes(&self, id: GroupId) -> BTreeSet<u64> {
        let (mut all, mut open) = (BTreeSet::new(), vec![id]);
        while let Some(group) = open.pop().and_then(|id| self.group(id)) {
            for member in &group.members {
                if all.insert(*member) && self.group(GroupId(*member)).is_some() {
                    open.push(GroupId(*member));
                }
            }
        }
        all
    }

    /// Whether a layer is a board (plan §7.2c): by its own role, or by a group above it that
    /// is the storyboard's.
    pub fn is_board(&self, layer: LayerId) -> bool {
        self.layer(layer).is_some_and(|layer| layer.role == Role::Board) || self.groups_above(layer.0).any(|group| group.board)
    }

    /// Whether a peg is a board's: in a group that is the storyboard's, as the peg each board
    /// is given is, or carrying boards alone.
    pub fn is_board_peg(&self, peg: crate::PegId) -> bool {
        let carried: Vec<LayerId> = self.layers.iter().filter(|layer| layer.parent == Some(peg)).map(|layer| layer.id).collect();
        let alone = !carried.is_empty() && carried.iter().all(|layer| self.is_board(*layer)) && !self.pegs.iter().any(|other| other.parent == Some(peg));
        self.groups_above(peg.0).any(|group| group.board) || alone
    }

    /// The shot with only one of its two layers of work to be seen. Everything else is as it is, the pegs and
    /// the camera too, so what is left is seen where and how it is with the rest there; but of
    /// the boards, the storyboard's group is seen though it is hidden.
    pub fn only(&self, work: Work) -> Shot {
        let mut of = self.clone();
        // The storyboard's group hidden on the Animation page is reference put away there: the
        // boards are what the storyboard is, and show whatever its eye says.
        if work == Work::Boards {
            of.hidden.retain(|node| !self.group(GroupId(*node)).is_some_and(|group| group.board));
        }
        for layer in self.stack() {
            let keep = match work {
                Work::Boards => self.is_board(layer),
                Work::Animation => !self.is_board(layer),
                Work::Whole => true,
            };
            if !keep {
                of.disconnect(Source::Layer(layer));
            }
        }
        // Rain, snow and the like are the animation's, not the boards'.
        if work == Work::Boards {
            for effect in self.effects.iter().filter(|effect| effect.kind == crate::PARTICLES) {
                of.disconnect(Source::Effect(effect.id));
            }
        }
        of
    }
}

impl crate::Project {
    /// Give a shot the layers its storyboard is drawn on: *Background*, and *Character* over
    /// it, in its group for storyboard drawings, which they are wired in through. With
    /// `under`, under what the shot shows already, as reference under the animation; else on
    /// top. Returns them, the top first.
    pub fn add_storyboard_layers(&mut self, shot: crate::ShotId, under: bool) -> Vec<LayerId> {
        let mut made = Vec::new();
        for name in ["Background", "Character"] {
            let layer = self.new_layer(name, Role::Board);
            let id = layer.id;
            let Some(within) = self.shot_mut(shot) else { return made };
            let name = crate::unique_name(&layer.name, within.node_names());
            within.layers.push(Arc::new(crate::Layer { name, ..layer }));
            // The first is wired where the boards go; the next goes on top of it in the group.
            if made.is_empty() {
                let main = within.main_composite();
                if let Some(composite) = within.composites.iter_mut().find(|composite| Some(composite.id) == main) {
                    let composite = Arc::make_mut(composite);
                    if under { composite.inputs.push(Source::Layer(id)) } else { composite.inputs.insert(0, Source::Layer(id)) }
                }
            }
            self.into_board_group(shot, id.0);
            self.peg_the_board(shot, id);
            made.insert(0, id);
        }
        made
    }

    /// Put a peg above a board layer of a shot, between it and its parent, named after it as
    /// `Character-P`: what its drawings are animated by, a few keys of where they are, whose
    /// keys the Storyboard page shows on the board's own row. It goes into the group for the
    /// storyboard drawings with the board. Returns the peg.
    pub fn peg_the_board(&mut self, shot: crate::ShotId, layer: LayerId) -> Option<crate::PegId> {
        let id = crate::PegId(self.new_id());
        let within = self.shot_mut(shot)?;
        let board = within.layer(layer)?;
        let (name, parent) = (crate::unique_name(&format!("{}-P", board.name), within.node_names()), board.parent);
        within.pegs.push(Arc::new(crate::Peg {
            id,
            name,
            parent,
            transform: Default::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        Arc::make_mut(within.layers.iter_mut().find(|own| own.id == layer)?).parent = Some(id);
        self.into_board_group(shot, id.0);
        Some(id)
    }

    /// Put a node of a shot into the shot's group for its storyboard drawings (plan §7.2c),
    /// which is made if the shot has none: the first of its groups that is the storyboard's,
    /// or a new one named *Storyboard*. Returns the group.
    ///
    /// A layer that goes in is wired into the group's own composite, on top of what that
    /// stacks: the first composite that is in the group, or a new one, which is wired in
    /// where the layer was. So one wire leaves the group, whatever number of boards it has.
    pub fn into_board_group(&mut self, shot: crate::ShotId, node: u64) -> Option<GroupId> {
        let found = self.shot(shot)?.groups.iter().find(|group| group.board).map(|group| group.id);
        let id = match found {
            Some(id) => id,
            None => {
                let id = GroupId(self.new_id());
                let within = self.shot_mut(shot)?;
                let name = crate::unique_name("Storyboard", within.node_names());
                within.groups.push(Arc::new(Group { id, name, members: BTreeSet::new(), folded: false, rows_folded: false, board: true, template: None }));
                id
            }
        };
        let within = self.shot_mut(shot)?;
        for group in &mut within.groups {
            if group.members.contains(&node) && group.id != id {
                Arc::make_mut(group).members.remove(&node);
            }
        }
        within.group_mut(id)?.members.insert(node);
        if within.layer(LayerId(node)).is_some() {
            let own = within.group(id)?.members.iter().map(|member| CompositeId(*member)).find(|member| within.composite(*member).is_some());
            let fresh = CompositeId(self.new_id());
            self.shot_mut(shot)?.gather_board(id, LayerId(node), own.unwrap_or(fresh));
        }
        Some(id)
    }
}

impl Shot {
    /// Wire a board into `own`, the composite of the storyboard's group `group`, on top of
    /// what that stacks. A composite the shot has not yet is made, in the group, and takes
    /// the board's place where the board was wired.
    fn gather_board(&mut self, group: GroupId, layer: LayerId, own: CompositeId) {
        let tip = self.tip(Source::Layer(layer));
        if self.composite(own).is_none() {
            let name = crate::unique_name("Storyboard Composite", self.node_names());
            self.composites.push(Arc::new(crate::Composite::new(own, &name, Vec::new())));
            self.rewire(tip, Some(Source::Composite(own)));
            if let Some(group) = self.group_mut(group) {
                group.members.insert(own.0);
            }
        } else {
            self.disconnect(tip);
        }
        if let Some(composite) = self.composites.iter_mut().find(|composite| composite.id == own) {
            Arc::make_mut(composite).inputs.insert(0, tip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    #[test]
    fn the_boards_alone_have_no_particles_and_the_animation_has_them() {
        let mut project = Project::default();
        let rain = crate::EffectId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        shot.effects.push(Arc::new(crate::Effect::new(rain, "Rain", crate::effect_kind(crate::PARTICLES).unwrap())));
        Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(rain));
        let wired = |part| shot.only(part).composites[0].inputs.contains(&Source::Effect(rain));
        assert_eq!((wired(Work::Boards), wired(Work::Animation), wired(Work::Whole)), (false, true, true));
    }

    #[test]
    fn a_group_holds_nodes_and_groups_and_makes_what_is_in_it_boards() {
        let mut project = Project::default();
        let extra = project.new_layer("Sky", Role::Animation);
        let (rig, arm) = (GroupId(project.new_id()), GroupId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (layer, sky) = (shot.layers[0].id, extra.id);
        Arc::make_mut(&mut shot.layers[0]).role = Role::Animation;
        shot.layers.push(Arc::new(extra));
        let group = |id, name: &str, members: &[u64]| {
            Arc::new(Group {
                id,
                name: name.to_string(),
                members: members.iter().copied().collect(),
                folded: false,
                rows_folded: false,
                board: false,
                template: None,
            })
        };
        shot.groups = vec![group(rig, "Rig", &[arm.0, 999]), group(arm, "Arm", &[layer.0])];

        assert_eq!((shot.group_of(layer.0).map(|group| group.id), shot.group_of(sky.0).map(|group| group.id)), (Some(arm), None));
        assert_eq!(shot.groups_above(layer.0).map(|group| group.id).collect::<Vec<_>>(), [arm, rig]);
        assert_eq!(shot.group_nodes(rig), [arm.0, layer.0, 999].into_iter().collect());
        // Folded, the outer group is what its nodes are put away in.
        assert_eq!(shot.folded_into(layer.0), None);
        shot.group_mut(arm).unwrap().folded = true;
        shot.group_mut(rig).unwrap().folded = true;
        assert_eq!((shot.folded_into(layer.0), shot.folded_into(arm.0), shot.folded_into(sky.0)), (Some(rig), Some(rig), None));
        // The storyboard's group makes boards of what is in it, however deep.
        assert!(!shot.is_board(layer) && !shot.is_board(sky));
        shot.group_mut(rig).unwrap().board = true;
        assert!(shot.is_board(layer) && !shot.is_board(sky));
        // Each part of the shot is what is seen of it alone, stacked as it was.
        shot.connect_on_top(sky);
        assert_eq!(shot.stack(), [sky, layer]);
        let seen = |part| shot.only(part).stack();
        assert_eq!((seen(Work::Boards), seen(Work::Animation), seen(Work::Whole)), (vec![layer], vec![sky], vec![sky, layer]));
        // A shot given new ids keeps its groups, with what is in them under its new ids.
        shot.reidentify(crate::ShotId(5000), {
            let mut next = 6000;
            move || {
                next += 1;
                next
            }
        });
        let (rig, layer) = (shot.groups[0].id, shot.layers[0].id);
        assert!(rig.0 > 6000 && shot.groups_above(layer.0).map(|group| group.id).last() == Some(rig) && shot.group_nodes(rig).len() == 2);
    }
}
