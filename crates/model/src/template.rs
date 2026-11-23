//! Library templates (plan §4.12): some of a shot's nodes, taken out to be put into other
//! shots, of this project or another.
//!
//! A template is a project of its own, with one shot that holds the nodes: so a template file
//! is a project file, and whatever reads one reads the other. The shot's display shows a
//! composite that stands for the shot the template is put into: what is connected to it is
//! what gets connected there.
//!
//! Putting a template into a shot makes copies. Nothing ties them to the template afterwards
//! but a note of its name.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{Clip, CompositeId, GroupId, LayerId, Palette, PegId, Project, Shot, ShotId, Source, Storyboard, StoryboardId, unique_name};

/// One of a shot's nodes, of any kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Part {
    Layer(LayerId),
    Peg(PegId),
    Composite(CompositeId),
    Group(GroupId),
    /// An effect node taken by itself, as a Particles node, which takes no picture that
    /// would bring it along.
    Effect(crate::EffectId),
    /// A display beside the shot's main one, as a Pass is (plan §7.14).
    Display(crate::DisplayId),
}

/// What a template file says about itself, beside the nodes it holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TemplateInfo {
    /// The name of the project it was saved from.
    pub source: String,
    /// When it was saved, in seconds since the start of 1970 (UTC).
    pub saved: u64,
    pub tags: Vec<String>,
    pub notes: String,
}

/// The nodes that putting a template into a shot made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    pub layers: Vec<LayerId>,
    pub pegs: Vec<PegId>,
    pub composites: Vec<CompositeId>,
    pub groups: Vec<GroupId>,
    pub effects: Vec<crate::EffectId>,
    pub displays: Vec<crate::DisplayId>,
}

impl Shot {
    /// Whether the shot has the node.
    pub fn has(&self, part: Part) -> bool {
        match part {
            Part::Group(id) => self.group(id).is_some(),
            Part::Layer(id) => self.layer(id).is_some(),
            Part::Peg(id) => self.peg(id).is_some(),
            Part::Composite(id) => self.composite(id).is_some(),
            Part::Effect(id) => self.effect(id).is_some(),
            // The main display stays: a shot has its own.
            Part::Display(id) => self.displays.iter().any(|display| display.id == id),
        }
    }

    /// Everything that makes a picture, the top of what is seen first: what the display shows
    /// with each composite before what it stacks, then the rest.
    fn sources(&self) -> Vec<Source> {
        let (mut listed, mut seen) = (Vec::new(), HashSet::new());
        let rest = self.composites.iter().map(|c| Source::Composite(c.id)).chain(self.effects.iter().map(|e| Source::Effect(e.id)));
        let rest = rest.chain(self.layers.iter().map(|l| Source::Layer(l.id)));
        for start in self.display.into_iter().chain(rest) {
            let mut open = vec![start];
            while let Some(source) = open.pop() {
                if !seen.insert(source) {
                    continue;
                }
                listed.push(source);
                open.extend(self.inputs_of(source).into_iter().rev());
            }
        }
        listed
    }

    /// The nodes that go into a template of `parts`. A layer goes alone. A peg takes all that
    /// it carries. A composite takes all that it stacks, and the pegs over those layers, so
    /// that a character's composite is the whole character. Not `whole`, a peg and a composite
    /// go alone too, as copied nodes do: only a group takes what is in it.
    fn taken(&self, parts: &[Part], whole: bool) -> HashSet<Part> {
        let mut taken: HashSet<Part> = HashSet::new();
        let mut open: Vec<Source> = Vec::new();
        for part in parts.iter().filter(|part| self.has(**part)) {
            match part {
                Part::Composite(id) if whole => open.push(Source::Composite(*id)),
                // A group takes what is in it, however deep, and no more: it is a rig as
                // it was put together.
                Part::Group(id) => {
                    taken.insert(*part);
                    for node in self.group_nodes(*id) {
                        let kinds = [
                            Part::Layer(LayerId(node)),
                            Part::Peg(PegId(node)),
                            Part::Composite(CompositeId(node)),
                            Part::Group(GroupId(node)),
                            Part::Display(crate::DisplayId(node)),
                        ];
                        taken.extend(kinds.into_iter().filter(|part| self.has(*part)));
                    }
                }
                part => {
                    taken.insert(*part);
                }
            }
        }
        while let Some(source) = open.pop() {
            match source {
                Source::Composite(id) if taken.insert(Part::Composite(id)) => open.extend(self.composite(id).into_iter().flat_map(|c| &c.inputs)),
                Source::Composite(_) => {}
                // What goes through an effect comes along with what the effect is wired into.
                Source::Effect(id) => open.extend(self.effect(id).into_iter().flat_map(|effect| effect.inputs.values())),
                Source::Layer(id) | Source::Art(id, _) => {
                    let Some(layer) = self.layer(id) else { continue };
                    taken.insert(Part::Layer(id));
                    taken.extend(self.ancestors(layer.parent).map(|peg| Part::Peg(peg.id)));
                }
            }
        }
        // What is under a peg that was asked for, however deep.
        let asked: HashSet<PegId> = parts.iter().filter(|_| whole).filter_map(|part| if let Part::Peg(id) = part { Some(*id) } else { None }).collect();
        let under = |parent: Option<PegId>| self.ancestors(parent).any(|peg| asked.contains(&peg.id));
        taken.extend(self.pegs.iter().filter(|peg| under(peg.parent)).map(|peg| Part::Peg(peg.id)));
        taken.extend(self.layers.iter().filter(|layer| under(layer.parent)).map(|layer| Part::Layer(layer.id)));
        taken
    }
}

impl Project {
    /// A template of some of a shot's nodes (see [`Shot::taken`] for which come along), as a
    /// project named `name`: one shot with the nodes as they are, their drawings shared with
    /// this project's, and the swatches those drawings use. What was parented to a peg that
    /// stays behind has no parent there. `None` if the shot has none of the nodes.
    pub fn template(&self, shot: ShotId, parts: &[Part], name: &str) -> Option<Project> {
        self.template_of(shot, parts, name, true)
    }

    /// A copy of some of a shot's nodes, as [`Project::template`] makes a template of them,
    /// but of those nodes alone: a peg without what it carries, a composite without what it
    /// stacks, a layer without the effects on its wire. A group still comes with all in it,
    /// and a layer with what bends it. They are wired as they were among themselves only.
    pub fn copy_of(&self, shot: ShotId, parts: &[Part]) -> Option<Project> {
        self.template_of(shot, parts, "Copy", false)
    }

    fn template_of(&self, shot: ShotId, parts: &[Part], name: &str, whole: bool) -> Option<Project> {
        let from = self.shot(shot)?;
        let taken = from.taken(parts, whole);
        if taken.is_empty() {
            return None;
        }
        // Ids go on from this project's, so the nodes keep theirs.
        let mut template = Project {
            settings: self.settings.clone(),
            palette: Arc::default(),
            shots: Vec::new(),
            storyboards: Vec::new(),
            bins: Vec::new(),
            media: Vec::new(),
            renders: Vec::new(),
            look: 0,
            next_id: self.next_id,
        };
        let mut shot = template.new_shot(name, from.duration);
        // A transform that comes down through ports, Sends and Receives (plan §8.6) comes with
        // them: those between what is taken and the peg above it, and those a group that is
        // taken has in it.
        let mut carriers: Vec<Arc<crate::Effect>> = Vec::new();
        let parents = from.layers.iter().filter(|layer| taken.contains(&Part::Layer(layer.id))).map(|layer| layer.parent);
        let parents = parents.chain(from.pegs.iter().filter(|peg| taken.contains(&Part::Peg(peg.id))).map(|peg| peg.parent));
        for parent in parents.filter(|_| whole).collect::<Vec<_>>() {
            let mut at = parent;
            // No more lie between two pegs than the shot has effects.
            for _ in 0..=from.effects.len() {
                let Some(effect) = at.filter(|id| from.peg(*id).is_none()).and_then(|id| from.effect(crate::EffectId(id.0))) else { break };
                if !carriers.iter().any(|carrier| carrier.id == effect.id) {
                    carriers.push(Arc::new(effect.clone()));
                }
                at = from.handed_down(PegId(effect.id.0));
            }
        }
        for group in from.groups.iter().filter(|group| taken.contains(&Part::Group(group.id))) {
            for node in from.group_nodes(group.id) {
                if let Some(effect) = from.effect(crate::EffectId(node)).filter(|effect| !carriers.iter().any(|carrier| carrier.id == effect.id)) {
                    carriers.push(Arc::new(effect.clone()));
                }
            }
        }
        let kept = |peg: Option<PegId>| peg.filter(|id| taken.contains(&Part::Peg(*id)) || carriers.iter().any(|carrier| carrier.id.0 == id.0));
        for layer in from.layers.iter().filter(|layer| taken.contains(&Part::Layer(layer.id))) {
            let mut layer = layer.clone();
            if kept(layer.parent) != layer.parent {
                Arc::make_mut(&mut layer).parent = None;
            }
            shot.layers.push(layer);
        }
        // What bends a layer that is taken goes with it: its bones, curve or envelope.
        let bends: HashSet<_> = shot.layers.iter().filter_map(|layer| layer.deformer).collect();
        shot.deformers = from.deformers.iter().filter(|deformer| bends.contains(&deformer.id)).cloned().collect();
        // A group that is taken is in the template, with what is in it.
        shot.groups = from.groups.iter().filter(|group| taken.contains(&Part::Group(group.id))).cloned().collect();
        for peg in from.pegs.iter().filter(|peg| taken.contains(&Part::Peg(peg.id))) {
            let mut peg = peg.clone();
            if kept(peg.parent) != peg.parent {
                Arc::make_mut(&mut peg).parent = None;
            }
            shot.pegs.push(peg);
        }
        // The composite the shot was made with stands for the shot the template goes into:
        // what was connected to something that stays behind is connected to it.
        let composites: Vec<_> = from.composites.iter().filter(|composite| taken.contains(&Part::Composite(composite.id))).cloned().collect();
        // An effect goes with the picture that goes through it: with a layer the effects on
        // its wire, and so on along the wire.
        // An effect node taken by itself comes too, as a Particles node, which no picture
        // brings along; and what its picture goes through, as with any other.
        let mut effects: Vec<Arc<crate::Effect>> = from.effects.iter().filter(|effect| taken.contains(&Part::Effect(effect.id))).cloned().collect();
        // Not for nodes copied alone: only those asked for.
        loop {
            if !whole {
                break;
            }
            let has = |source: Source| match source {
                Source::Layer(id) | Source::Art(id, _) => taken.contains(&Part::Layer(id)),
                Source::Composite(id) => taken.contains(&Part::Composite(id)),
                Source::Effect(id) => effects.iter().any(|effect| effect.id == id),
            };
            let more: Vec<_> = from.effects.iter().filter(|effect| !has(Source::Effect(effect.id)) && effect.picture().is_some_and(has)).cloned().collect();
            if more.is_empty() {
                break;
            }
            effects.extend(more);
        }
        let inside: HashSet<Source> =
            composites.iter().flat_map(|composite| &composite.inputs).chain(effects.iter().flat_map(|effect| effect.inputs.values())).copied().collect();
        let is_taken = |source: &Source| match source {
            Source::Layer(id) | Source::Art(id, _) => taken.contains(&Part::Layer(*id)),
            Source::Composite(id) => taken.contains(&Part::Composite(*id)),
            Source::Effect(id) => effects.iter().any(|effect| effect.id == *id),
        };
        // A port that was wired to something that stays behind is empty in the template, and
        // one that a peg that stays behind was wired into carries nothing.
        shot.effects = effects.clone();
        shot.effects.extend(carriers.iter().filter(|carrier| !effects.iter().any(|effect| effect.id == carrier.id)).cloned());
        for effect in &mut shot.effects {
            if effect.inputs.values().any(|input| !is_taken(input)) {
                Arc::make_mut(effect).inputs.retain(|_, input| is_taken(input));
            }
            if kept(effect.parent) != effect.parent {
                Arc::make_mut(effect).parent = None;
            }
        }
        // A display taken, as a Pass, shows what it showed if that came too, else nothing.
        shot.displays = from
            .displays
            .iter()
            .filter(|display| taken.contains(&Part::Display(display.id)))
            .map(|display| crate::Display { shows: display.shows.filter(is_taken), ..display.clone() })
            .collect();
        let loose = from.sources().into_iter().filter(|source| is_taken(source) && !inside.contains(source));
        Arc::make_mut(&mut shot.composites[0]).inputs = loose.collect();
        // A composite taken alone stacks only what came with it.
        let composites = composites.into_iter().map(|mut composite| {
            if composite.inputs.iter().any(|input| !is_taken(input)) {
                Arc::make_mut(&mut composite).inputs.retain(is_taken);
            }
            composite
        });
        shot.composites.extend(composites);
        let ids: HashSet<u64> = shot.node_ids().collect();
        shot.places = from.places.iter().filter(|(node, _)| ids.contains(node)).map(|(node, place)| (*node, *place)).collect();
        shot.colours = from.colours.iter().filter(|(node, _)| ids.contains(node)).map(|(node, colour)| (*node, *colour)).collect();
        shot.loaded = from.loaded;

        let used: BTreeSet<_> = shot.drawings().flat_map(|entry| entry.drawing.swatches()).collect();
        // With the named palettes they are in, so that they find their place where they go.
        let mut kept = Palette::of(self.palette.swatches.iter().filter(|swatch| used.contains(&swatch.id)).cloned().collect());
        kept.sets = self.palette.sets.clone();
        // And with what the styles say of them: the character's colours by night go with it.
        let of_kept = |style: &crate::Style| crate::Style {
            colors: style.colors.iter().filter(|(swatch, _)| used.contains(swatch)).map(|(s, c)| (*s, *c)).collect(),
            ..style.clone()
        };
        kept.styles = self.palette.styles.iter().map(of_kept).filter(|style| !style.colors.is_empty()).collect();
        // The styles the shot was shown in, of those that go with it: it is shown so where
        // it is put.
        shot.styles = from.styles.iter().copied().filter(|id| kept.style(*id).is_some()).collect();
        // And the skins its layers have variants in, by their names.
        kept.skins =
            self.palette.skins.iter().filter(|skin| shot.layers.iter().any(|layer| layer.variants.iter().any(|v| v.skin == skin.id))).cloned().collect();
        shot.skin = shot.skin.filter(|skin| kept.skin(*skin).is_some());
        template.palette = Arc::new(kept);
        // On a storyboard, so that the file opens as a project like any other.
        let mut storyboard = Storyboard::new(StoryboardId(template.new_id()), "Storyboard");
        let _ = storyboard.put(Clip { shot: shot.id, start: 0, in_point: 0, out_point: shot.duration }, false);
        template.shots.push(Arc::new(shot));
        template.storyboards.push(Arc::new(storyboard));
        Some(template)
    }

    /// Put the template `template`, which is called `name`, into a shot: copies of its nodes
    /// with ids of this project, their drawings shared with the template's until one is drawn
    /// on. A name the shot has already gets a number after it. What the template's display
    /// shows is connected on top of the composite that new layers go to. A swatch this
    /// project's palette lacks is added to it; one it has, by its id, stays as it is here.
    /// The template's styles come with its swatches, and where it was shown in one of a
    /// palette that the shot shows in its own colours, the shot is shown in it.
    ///
    /// With `at`, the nodes that have places in the template are put around that place of the
    /// node view, arranged as they were; without, the view lays them all out.
    ///
    /// `None` if this project has no such shot or the template no shot.
    pub fn place_template(&mut self, shot: ShotId, template: &Project, name: &str, at: Option<[f64; 2]>) -> Option<Placed> {
        self.shot(shot)?;
        let mut copy = Shot::clone(template.shots.first()?);
        // Its variants in this project's skins of the same names; the shot keeps its own skin.
        let like = self.skins_like(&template.palette);
        copy.reskin(&like);
        let mut palette = Palette::clone(&self.palette);
        palette.adopt(&template.palette);
        // The styles it was shown in, by their names and their palettes' names, as they are
        // here now: each where the shot shows that palette in its own colours.
        let shown: Vec<_> = copy
            .styles
            .iter()
            .filter_map(|id| template.palette.style(*id))
            .filter_map(|style| {
                let set = template.palette.set(style.set).map(|set| set.name.as_str());
                let here = palette.sets.iter().find(|own| Some(own.name.as_str()) == set).map_or(crate::SetId::DEFAULT, |own| own.id);
                palette.styles.iter().find(|own| own.name == style.name && own.set == here).map(|own| (own.id, here))
            })
            .collect();
        if palette != *self.palette {
            self.palette = Arc::new(palette);
        }
        if !shown.is_empty() {
            let palette = self.palette.clone();
            let into = self.shot_mut(shot)?;
            for (style, set) in shown {
                if palette.style_among(&into.styles, set).is_none() {
                    into.styles.push(style);
                }
            }
        }
        self.place_nodes(shot, copy, Some(name), at, true)
    }

    /// Put copies of what was copied into a shot (the nodes of `copied`, which
    /// [`Project::template`] made of them): as [`Project::place_template`] puts a template,
    /// but the copies are of no template and say so. Without `at` they are a little down and
    /// to the right of where the nodes they were copied from are in the node view, so that
    /// in the shot those came from they do not lie on them. Without `connect` what the copies
    /// showed is not connected to what the shot shows: they are wired only to each other.
    pub fn place_copies(&mut self, shot: ShotId, copied: &Project, at: Option<[f64; 2]>, connect: bool) -> Option<Placed> {
        const BESIDE: f64 = 40.0;
        self.shot(shot)?;
        let copy = Shot::clone(copied.shots.first()?);
        let mut palette = Palette::clone(&self.palette);
        palette.adopt(&copied.palette);
        if palette != *self.palette {
            self.palette = Arc::new(palette);
        }
        let own = |node: &&u64| **node != copy.id.0 && **node != copy.camera.id.0;
        let places: Vec<[f64; 2]> = copy.places.iter().filter(|(node, _)| own(node)).map(|(_, place)| *place).collect();
        let middle = |axis: usize| places.iter().map(|place| place[axis]).sum::<f64>() / places.len().max(1) as f64;
        let beside = (!places.is_empty()).then(|| [middle(0) + BESIDE, middle(1) + BESIDE]);
        self.place_nodes(shot, copy, None, at.or(beside), connect)
    }

    /// Put copies of all the nodes of `copy`, a shot that is not the project's, into `shot`,
    /// with fresh ids, named so that no name is taken twice, and connected on top of what the
    /// shot shows as `copy` showed them. With `mark` they say that they are copies from the
    /// template of that name. The camera of `copy` stays behind: a shot has its own. A Send
    /// of `copy` whose tag the shot has already is given one of its own, with the Receives of
    /// `copy` that read it, so that the two go on reading their own. Without `connect` what
    /// `copy` showed is connected to nothing of the shot's.
    pub(crate) fn place_nodes(&mut self, shot: ShotId, mut copy: Shot, mark: Option<&str>, at: Option<[f64; 2]>, connect: bool) -> Option<Placed> {
        self.shot(shot)?;
        let id = copy.id;
        // The template's camera stays behind: a shot has its own. Its place and the display's
        // go before the nodes get their new ids, one of which may be the id the shot had.
        copy.places.remove(&id.0);
        copy.places.remove(&copy.camera.id.0);
        copy.colours.remove(&id.0);
        copy.reidentify(id, || self.new_id());
        // The composite on display stands for the shot: its inputs are what is connected.
        let stand_in = match copy.display {
            Some(Source::Composite(stand_in)) => Some(stand_in),
            _ => None,
        };
        let shown = match copy.display {
            Some(Source::Composite(stand_in)) => {
                let inputs = copy.composite(stand_in).map(|composite| composite.inputs.clone()).unwrap_or_default();
                copy.composites.retain(|composite| composite.id != stand_in);
                copy.places.remove(&stand_in.0);
                inputs
            }
            shown => shown.into_iter().collect(),
        };
        let middle = |axis: usize| copy.places.values().map(|place| place[axis]).sum::<f64>() / copy.places.len().max(1) as f64;
        let moved = at.map(|at| [at[0] - middle(0), at[1] - middle(1)]);

        let into = self.shot_mut(shot)?;
        let mut names: Vec<String> = into.node_names().map(str::to_string).collect();
        let mut named = |name: &mut String| {
            *name = unique_name(name, names.iter().map(String::as_str));
            names.push(name.clone());
        };
        let mut placed = Placed::default();
        for mut peg in copy.pegs {
            let made = Arc::make_mut(&mut peg);
            named(&mut made.name);
            made.template = mark.map(str::to_string).or(made.template.take());
            placed.pegs.push(peg.id);
            into.pegs.push(peg);
        }
        for mut composite in copy.composites {
            let made = Arc::make_mut(&mut composite);
            named(&mut made.name);
            made.template = mark.map(str::to_string).or(made.template.take());
            placed.composites.push(composite.id);
            into.composites.push(composite);
        }
        for mut group in copy.groups {
            let made = Arc::make_mut(&mut group);
            named(&mut made.name);
            made.template = mark.map(str::to_string).or(made.template.take());
            placed.groups.push(group.id);
            into.groups.push(group);
        }
        for mut deformer in copy.deformers {
            named(&mut Arc::make_mut(&mut deformer).name);
            into.deformers.push(deformer);
        }
        // A Send whose tag the shot has already is given one of its own, and the Receives
        // that came with it and read it take that too: else they would read the shot's Send.
        let mut tags = into.tags();
        let mut retagged: HashMap<String, String> = HashMap::new();
        for send in copy.effects.iter_mut().filter(|effect| effect.kind == crate::SEND) {
            let tag = send.text(crate::TAG);
            if tag.is_empty() || !tags.contains(&tag) {
                continue;
            }
            let fresh = retagged.entry(tag.clone()).or_insert_with(|| unique_name(&tag, tags.iter().map(String::as_str))).clone();
            tags.push(fresh.clone());
            if let Some(held) = Arc::make_mut(send).param_mut(crate::TAG) {
                held.value = crate::Value::Text(fresh);
            }
        }
        for receive in copy.effects.iter_mut().filter(|effect| effect.kind == crate::RECEIVE) {
            if let Some(fresh) = retagged.get(&receive.text(crate::TAG)).cloned()
                && let Some(held) = Arc::make_mut(receive).param_mut(crate::TAG)
            {
                held.value = crate::Value::Text(fresh);
            }
        }
        for mut effect in copy.effects {
            let made = Arc::make_mut(&mut effect);
            named(&mut made.name);
            made.template = mark.map(str::to_string).or(made.template.take());
            placed.effects.push(effect.id);
            into.effects.push(effect);
        }
        // The newest first, as a shot lists its layers, and in the order the template has them.
        for (index, mut layer) in copy.layers.into_iter().enumerate() {
            let made = Arc::make_mut(&mut layer);
            named(&mut made.name);
            made.template = mark.map(str::to_string).or(made.template.take());
            placed.layers.push(layer.id);
            into.layers.insert(index, layer);
        }
        // Displays beside the main one, which a shot merged in has: what showed the composite
        // that stood for it shows the one it went into.
        names.push(into.display_name.clone());
        let main = into.main_composite();
        for mut display in copy.displays {
            display.name = unique_name(&display.name, names.iter().map(String::as_str));
            names.push(display.name.clone());
            if display.shows.is_some() && display.shows == stand_in.map(Source::Composite) {
                display.shows = main.map(Source::Composite);
            }
            placed.displays.push(display.id);
            into.displays.push(display);
        }
        into.colours.extend(copy.colours.iter().map(|(node, colour)| (*node, *colour)));
        if let Some(moved) = moved {
            into.places.extend(copy.places.iter().map(|(node, place)| (*node, [place[0] + moved[0], place[1] + moved[1]])));
        }
        if connect
            && let Some(main) = into.main_composite()
            && let Some(composite) = into.composites.iter_mut().find(|composite| composite.id == main)
        {
            Arc::make_mut(composite).inputs.splice(0..0, shown);
        }
        Some(placed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Animation, Composite, DrawingId, Peg, PoolDrawing, Role, Swatch, SwatchId, Transform};

    /// A shot with a character: `Body` and `Arm` (under the peg `Shoulder`) stacked by the
    /// composite `Character`, all under the peg `Root`; and a `Sky` behind, on the shot's own
    /// composite with the character.
    fn character() -> (Project, [Part; 6]) {
        let mut project = Project::default();
        let [body, arm] = ["Body", "Arm"].map(|name| project.new_layer(name, Role::Animation));
        let (root, shoulder, character) = (PegId(project.new_id()), PegId(project.new_id()), CompositeId(project.new_id()));
        let drawing = DrawingId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        let sky = shot.layers[0].id;
        Arc::make_mut(&mut shot.layers[0]).name = "Sky".to_string();
        let peg = |id, name: &str, parent| {
            let transform = Transform { position: [10.0, 0.0], ..Transform::default() };
            Arc::new(Peg {
                id,
                name: name.to_string(),
                parent,
                transform,
                animation: Animation::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        };
        shot.pegs = vec![peg(root, "Root", None), peg(shoulder, "Shoulder", Some(root))];
        let parts = [Part::Layer(body.id), Part::Layer(arm.id), Part::Peg(root), Part::Peg(shoulder), Part::Composite(character), Part::Layer(sky)];
        let mut painted = crate::Drawing::default();
        if let crate::ArtContent::Indexed(surface) = &mut painted.art_layers[1].content {
            surface.paint(iskra_drawing::Rect::new(0, 0, 2, 1), &[1.0, 1.0], SwatchId(3));
        }
        let (mut body, mut arm) = (body, arm);
        (body.parent, arm.parent) = (Some(root), Some(shoulder));
        arm.drawings.push(PoolDrawing { id: drawing, name: "1".to_string(), drawing: Arc::new(painted) });
        arm.exposure.set(0, 4, Some(drawing));
        let inputs = vec![Source::Layer(arm.id), Source::Layer(body.id)];
        shot.composites.push(Arc::new(Composite::new(character, "Character", inputs)));
        Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Composite(character));
        shot.layers.splice(0..0, [Arc::new(arm), Arc::new(body)]);
        shot.places.insert(shoulder.0, [100.0, -200.0]);
        shot.places.insert(sky.0, [300.0, -100.0]);
        (project, parts)
    }

    fn names(shot: &Shot) -> Vec<&str> {
        shot.node_names().collect()
    }

    #[test]
    fn a_layer_goes_alone_a_peg_with_what_it_carries_and_a_composite_with_all_it_stacks() {
        let (project, [body, arm, root, shoulder, character, sky]) = character();
        let shot = &project.shots[0];
        let taken = |parts: &[Part]| shot.taken(parts, true);
        assert_eq!(taken(&[arm]), HashSet::from([arm]));
        assert_eq!(taken(&[shoulder]), HashSet::from([shoulder, arm]));
        assert_eq!(taken(&[root]), HashSet::from([root, shoulder, arm, body]), "the composite is not under the peg");
        assert_eq!(taken(&[character]), HashSet::from([character, arm, body, shoulder, root]), "with the pegs over its layers");
        assert_eq!(taken(&[sky, shoulder]), HashSet::from([sky, shoulder, arm]));
        assert_eq!(taken(&[Part::Layer(LayerId(999))]), HashSet::new());
        assert!(project.template(shot.id, &[Part::Peg(PegId(999))], "x").is_none() && project.template(ShotId(999), &[arm], "x").is_none());
    }

    #[test]
    fn a_template_is_a_project_of_one_shot_that_shows_what_was_taken() {
        let (project, [body, arm, _, shoulder, character, sky]) = character();
        let from = &project.shots[0];
        let template = project.template(from.id, &[character], "Hero").unwrap();
        let [shot] = &template.shots[..] else { panic!("one shot") };
        assert_eq!((shot.name.as_str(), shot.duration, names(shot)), ("Hero", 48, vec!["Arm", "Body", "Root", "Shoulder", "Composite", "Character"]));
        let (Part::Layer(arm_id), Part::Layer(body_id), Part::Composite(character_id)) = (arm, body, character) else { panic!() };
        assert_eq!(shot.stack(), [arm_id, body_id], "seen as it was stacked");
        assert_eq!(shot.composites[0].inputs, [Source::Composite(character_id)], "the character is what the shot's composite is given");
        assert!(Arc::ptr_eq(&shot.layers[0], &from.layers[0]), "the nodes are shared, not copied");
        assert_eq!(template.palette.swatches.iter().map(|s| s.id.0).collect::<Vec<_>>(), [3], "only the swatches its drawings use");
        assert_eq!(
            shot.places.keys().copied().collect::<Vec<_>>(),
            [match shoulder {
                Part::Peg(id) => id.0,
                _ => 0,
            }],
            "and where its nodes were put"
        );
        assert_eq!(template.storyboards[0].clips().len(), 1, "it opens as a project");
        let mut next = template.clone();
        assert!(from.node_ids().all(|id| id < next.new_id()), "new ids don't meet the old");

        // What was parented to a peg that stays behind has no parent in the template; layers
        // connected to nothing that was taken are stacked as they were.
        let template = project.template(from.id, &[sky, arm, body], "Parts").unwrap();
        let shot = &template.shots[0];
        let Part::Layer(sky_id) = sky else { panic!() };
        assert_eq!((shot.stack(), shot.pegs.len()), (vec![arm_id, body_id, sky_id], 0));
        assert!(shot.layers.iter().all(|layer| layer.parent.is_none()) && from.layers[0].parent.is_some());
    }

    #[test]
    fn placing_a_template_makes_copies_with_free_names_connected_on_top() {
        let (mut project, [_, arm, _, _, character, _]) = character();
        let from = project.shots[0].id;
        let template = project.template(from, &[character], "Hero").unwrap();
        let before = project.clone();

        // Into the shot it came from: every name is taken.
        let placed = project.place_template(from, &template, "Hero", None).unwrap();
        let shot = &project.shots[0];
        assert_eq!((placed.layers.len(), placed.pegs.len(), placed.composites.len()), (2, 2, 1), "the stand-in composite is not copied");
        assert_eq!(&names(shot)[..2], ["Arm 2", "Body 2"]);
        assert_eq!(&names(shot)[5..], ["Root", "Shoulder", "Root 2", "Shoulder 2", "Composite", "Character", "Character 2"]);
        assert!(before.shots[0].node_ids().all(|id| !placed.layers.iter().any(|l| l.0 == id) && !placed.pegs.iter().any(|p| p.0 == id)));
        let Part::Layer(arm_id) = arm else { panic!() };
        let (copy, original) = (shot.layer(placed.layers[0]).unwrap(), shot.layer(arm_id).unwrap());
        assert!(Arc::ptr_eq(&copy.drawings[0].drawing, &original.drawings[0].drawing) && copy.drawings[0].id != original.drawings[0].id);
        assert_eq!(copy.drawing_at(3).map(|d| d.id), Some(copy.drawings[0].id), "the copy's timing shows the copy's drawings");
        assert_eq!((copy.parent, shot.peg(placed.pegs[1]).unwrap().parent), (Some(placed.pegs[1]), Some(placed.pegs[0])), "the tree is of the copies");
        assert_eq!(copy.template.as_deref(), Some("Hero"), "a copy says where it came from");
        assert!(original.template.is_none() && shot.peg(placed.pegs[0]).unwrap().template.is_some());
        assert_eq!(shot.composites[0].inputs[0], Source::Composite(placed.composites[0]), "on top of what the shot shows");
        assert_eq!(&shot.stack()[..2], &placed.layers[..]);
        assert_eq!(shot.places.len(), 2, "without a place to put them, the view lays the copies out");
        assert!(Arc::ptr_eq(&project.palette, &before.palette), "every swatch was there already");

        // Into another project: the swatch it lacks is added, the one it has stays as it is.
        let mut other = Project::default();
        let mut palette = Palette::clone(&other.palette);
        palette.swatches.retain(|swatch| swatch.id != SwatchId(3));
        other.palette = Arc::new(palette);
        let mut template = template;
        let mut swatches = Palette::clone(&template.palette);
        swatches.swatches.push(Swatch {
            id: SwatchId(1),
            name: "Ink".to_string(),
            color: [9, 9, 9, 255],
            set: Default::default(),
            kind: Default::default(),
            drift: None,
        });
        template.palette = Arc::new(swatches);
        let into = other.shots[0].id;
        let placed = other.place_template(into, &template, "Hero", Some([500.0, 40.0])).unwrap();
        assert_eq!((other.palette.color(SwatchId(3)), other.palette.color(SwatchId(1))), ([220, 50, 47, 255], [0, 0, 0, 255]));
        let shot = &other.shots[0];
        assert_eq!(names(shot), ["Arm", "Body", "Drawing", "Root", "Shoulder", "Composite", "Character"], "free names stay");

        // A style goes with the template, with the colours it gives the swatches that go: into
        // a project without it, it is added; into one that has a style of that name, the
        // colours that one lacks are, and what it has stays.
        let mut night = Project::clone(&project);
        let mut palette = Palette::clone(&night.palette);
        let colors = [(SwatchId(3), [60, 20, 90, 255]), (SwatchId(2), [1, 1, 1, 255])].into();
        palette.styles.push(crate::Style { id: crate::StyleId(1), name: "Night".to_string(), colors, set: Default::default(), kinds: Default::default() });
        night.palette = Arc::new(palette);
        let template = night.template(night.shots[0].id, &[character], "Hero").unwrap();
        let styles = |palette: &Palette| serde_json::to_value(&palette.styles).unwrap();
        assert_eq!(
            styles(&template.palette),
            serde_json::json!([{ "id": 1, "name": "Night", "colors": { "3": [60, 20, 90, 255] } }]),
            "of the swatches it uses"
        );
        let mut fresh = Project::default();
        fresh.place_template(fresh.shots[0].id, &template, "Hero", None).unwrap();
        assert_eq!(styles(&fresh.palette), styles(&template.palette));
        let mut has = Project::default();
        let mut palette = Palette::clone(&has.palette);
        let colors = [(SwatchId(5), [0, 0, 80, 255])].into();
        palette.styles.push(crate::Style { id: crate::StyleId(7), name: "Night".to_string(), colors, set: Default::default(), kinds: Default::default() });
        has.palette = Arc::new(palette);
        has.place_template(has.shots[0].id, &template, "Hero", None).unwrap();
        assert_eq!(styles(&has.palette), serde_json::json!([{ "id": 7, "name": "Night", "colors": { "3": [60, 20, 90, 255], "5": [0, 0, 80, 255] } }]));
        let again = has.palette.clone();
        has.place_template(has.shots[0].id, &template, "Hero", None).unwrap();
        assert!(Arc::ptr_eq(&has.palette, &again), "a second time nothing is new");
        assert!(fresh.shots[0].styles.is_empty() && template.shots[0].styles.is_empty(), "the shot was shown in the palette's own colours");

        // Saved from a shot shown at night, the template keeps it, and a shot it is put into
        // is shown at night, unless that shot shows the palette in a style of its own.
        Arc::make_mut(&mut night.shots[0]).styles = vec![crate::StyleId(1)];
        let template = night.template(night.shots[0].id, &[character], "Hero").unwrap();
        assert_eq!(template.shots[0].styles, [crate::StyleId(1)]);
        let mut fresh = Project::default();
        fresh.place_template(fresh.shots[0].id, &template, "Hero", None).unwrap();
        let id = fresh.palette.styles.iter().find(|style| style.name == "Night").unwrap().id;
        assert_eq!(fresh.shots[0].styles, [id]);
        let mut day = Project::default();
        let mut palette = Palette::clone(&day.palette);
        palette.styles.push(crate::Style {
            id: crate::StyleId(4),
            name: "Day".to_string(),
            colors: [(SwatchId(5), [9, 9, 9, 255])].into(),
            set: Default::default(),
            kinds: Default::default(),
        });
        day.palette = Arc::new(palette);
        Arc::make_mut(&mut day.shots[0]).styles = vec![crate::StyleId(4)];
        day.place_template(day.shots[0].id, &template, "Hero", None).unwrap();
        assert_eq!(day.shots[0].styles, [crate::StyleId(4)], "the shot keeps its own");
        assert_eq!(shot.places, std::collections::BTreeMap::from([(placed.pegs[1].0, [500.0, 40.0])]), "the placed nodes are put around the place given");
        assert!(other.place_template(ShotId(999), &template, "Hero", None).is_none());
    }

    #[test]
    fn a_layer_goes_into_a_template_with_what_bends_it() {
        use crate::{Bone, Deformer, DeformerId};
        let mut project = Project::default();
        let bones = DeformerId(project.new_id());
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (id, layer) = (shot.id, shot.layers[0].id);
        shot.deformers.push(Arc::new(Deformer::new(bones, "Arm-Bones", [0.0, 0.0], vec![Bone::at_rest(0.0, 50.0), Bone::at_rest(0.0, 50.0)])));
        Arc::make_mut(&mut shot.layers[0]).deformer = Some(bones);
        Arc::make_mut(&mut shot.deformers[0]).bones[1].turn = 30.0;

        let template = project.template(id, &[Part::Layer(layer)], "Arm").unwrap();
        assert_eq!((template.shots[0].deformers.len(), template.shots[0].layers[0].deformer), (1, Some(bones)));
        // Put into the shot, the copy has bones of its own, posed as the template's are.
        let placed = project.place_template(id, &template, "Arm", None).unwrap();
        let shot = &project.shots[0];
        let copy = shot.layer(placed.layers[0]).unwrap().deformer.expect("the copy is bent too");
        assert!(copy != bones && shot.deformers.len() == 2 && shot.deformer(copy).unwrap().bones[1].turn == 30.0);
        assert_eq!((shot.deformer(copy).unwrap().name.as_str(), shot.layer(layer).unwrap().deformer), ("Arm-Bones 2", Some(bones)));
    }

    #[test]
    fn a_group_goes_into_a_template_as_a_rig_and_comes_out_as_a_group() {
        use crate::{Group, GroupId};
        let mut project = Project::default();
        let extra = project.new_layer("Sky", Role::Animation);
        let (peg, rig) = (PegId(project.new_id()), GroupId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (id, arm, sky) = (shot.id, shot.layers[0].id, extra.id);
        shot.layers.push(Arc::new(extra));
        Arc::make_mut(&mut shot.composites[0]).inputs.push(Source::Layer(sky));
        shot.pegs.push(Arc::new(Peg {
            id: peg,
            name: "Arm-P".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Animation::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        Arc::make_mut(&mut shot.layers[0]).parent = Some(peg);
        let members = [peg.0, arm.0].into_iter().collect();
        shot.groups.push(Arc::new(Group { id: rig, name: "Arm Rig".to_string(), members, folded: true, rows_folded: true, board: false, template: None }));

        // The group takes what is in it and no more: the sky stays behind.
        let template = project.template(id, &[Part::Group(rig)], "Arm").unwrap();
        let saved = &template.shots[0];
        assert_eq!((saved.layers.len(), saved.pegs.len(), saved.groups.len(), saved.layers[0].parent), (1, 1, 1, Some(peg)));
        // Put into the shot, it is a group of the copies, folded as it was saved.
        let placed = project.place_template(id, &template, "Arm", None).unwrap();
        let shot = &project.shots[0];
        let copy = shot.group(placed.groups[0]).unwrap();
        assert_eq!((copy.name.as_str(), copy.folded, copy.template.as_deref()), ("Arm Rig 2", true, Some("Arm")));
        assert_eq!(copy.members, [placed.pegs[0].0, placed.layers[0].0].into_iter().collect());
        assert_eq!(shot.group(rig).unwrap().members.len(), 2, "the group it was saved from is as it was");
    }
}
