//! The node graph of a shot (plan §4.3): how pictures flow from its layers to what is seen.
//! A composite stacks what is connected to it, an effect changes the picture that goes through
//! it (see `effect`), and the display shows one node: a composite, an effect, or a single
//! layer.
//!
//! Pegs are nodes too, joined to what they carry by its `parent` (see `peg`). That is the
//! other kind of connection: a transform goes down it, where a picture goes along these.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{ArtLayerKind, CompositeId, EffectId, Layer, LayerId, Shot};

/// A node that makes a picture, which can be connected to a composite or to the display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    /// A layer's drawings, all their art layers as one picture: its flat output.
    Layer(LayerId),
    /// One art layer of a layer's drawings alone, from the output the layer has for it.
    Art(LayerId, ArtLayerKind),
    Composite(CompositeId),
    Effect(EffectId),
}

impl Source {
    /// A layer's outputs: its flat one, then one for each of its art layers.
    pub fn outputs(layer: LayerId) -> impl Iterator<Item = Source> {
        std::iter::once(Source::Layer(layer)).chain(ArtLayerKind::ALL.into_iter().map(move |kind| Source::Art(layer, kind)))
    }

    /// The layer it is an output of: its flat one or one of its art layers'.
    pub fn layer(self) -> Option<LayerId> {
        match self {
            Source::Layer(id) | Source::Art(id, _) => Some(id),
            Source::Composite(_) | Source::Effect(_) => None,
        }
    }
}

/// How a composite orders what it stacks (plan §8.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stacking {
    /// *3D:* by depth, among all else that the shot shows. What it stacks stays cards of
    /// their own, so that something from elsewhere can stand between them.
    #[default]
    Depth,
    /// *3D flat:* by depth among themselves, and then one flat card for what is above.
    DepthFlat,
    /// *2D:* by the order of its ports alone, whatever their depths, as one flat card.
    Ports,
}

impl Stacking {
    /// As a command says it.
    pub const NAMES: [(&'static str, Stacking); 3] = [("3d", Stacking::Depth), ("3dFlat", Stacking::DepthFlat), ("2d", Stacking::Ports)];

    fn is_depth(&self) -> bool {
        *self == Stacking::Depth
    }
}

/// Stacks its inputs into one picture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Composite {
    pub id: CompositeId,
    pub name: String,
    /// What is connected to it, top first, each at most once.
    pub inputs: Vec<Source>,
    /// The library template the composite is a copy from, if it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// How it orders what it stacks.
    #[serde(default, skip_serializing_if = "Stacking::is_depth")]
    pub stacking: Stacking,
    /// Stacked by its ports ([`Stacking::Ports`]), the last of `inputs` is on top, where
    /// otherwise the first is: in the node view, the right port and not the left.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub right_first: bool,
}

impl Composite {
    /// A composite that stacks `inputs`, top first, by depth.
    pub fn new(id: CompositeId, name: &str, inputs: Vec<Source>) -> Self {
        Self { id, name: name.to_string(), inputs, template: None, stacking: Stacking::Depth, right_first: false }
    }
}

impl Shot {
    pub fn composite(&self, id: CompositeId) -> Option<&Composite> {
        self.composites.iter().find(|c| c.id == id).map(|c| &**c)
    }

    /// The composite that `source` is connected to, straight or through the effects on its
    /// wire: the first, if it is connected to several.
    pub fn composite_of(&self, source: Source) -> Option<&Composite> {
        let tip = self.tip(source);
        self.composites.iter().find(|c| c.inputs.contains(&source) || c.inputs.contains(&tip)).map(|c| &**c)
    }

    /// What is wired into a node that takes pictures: a composite's inputs, an effect's ports.
    /// A layer takes none.
    pub fn inputs_of(&self, source: Source) -> Vec<Source> {
        match source {
            Source::Layer(_) | Source::Art(..) => Vec::new(),
            Source::Composite(id) => self.composite(id).map(|composite| composite.inputs.clone()).unwrap_or_default(),
            // A Receive gets its picture from the Send it names, as if by a wire.
            Source::Effect(id) => self.effect(id).map_or(Vec::new(), |effect| {
                let named = self.sender(effect).map(|send| Source::Effect(send.id));
                effect.inputs.values().copied().chain(named).collect()
            }),
        }
    }

    /// Whether `inner` is `outer` or gives its picture to it, straight or through other nodes.
    /// A node can't be wired into one that it gets its picture from.
    pub fn flows_into(&self, inner: Source, outer: Source) -> bool {
        let (mut seen, mut open) = (HashSet::new(), vec![outer]);
        while let Some(at) = open.pop() {
            if at == inner {
                return true;
            }
            if seen.insert(at) {
                open.extend(self.inputs_of(at));
            }
        }
        false
    }

    /// The composite that a new layer is connected to: the one on display, or else the first.
    pub fn main_composite(&self) -> Option<CompositeId> {
        match self.display {
            Some(Source::Composite(id)) if self.composite(id).is_some() => Some(id),
            _ => self.composites.first().map(|c| c.id),
        }
    }

    /// Whether `inside` is `composite` or feeds it, directly or through other nodes.
    pub fn feeds(&self, inside: CompositeId, composite: CompositeId) -> bool {
        self.flows_into(Source::Composite(inside), Source::Composite(composite))
    }

    /// The layers that are seen, top first: what the display shows, with every composite
    /// opened up into the layers it stacks. A layer that reaches the display by two ways is
    /// listed where it comes first.
    pub fn stack(&self) -> Vec<LayerId> {
        let (mut stack, mut seen) = (Vec::new(), HashSet::new());
        // What is still to list, the next on top.
        let mut open: Vec<Source> = self.display.into_iter().collect();
        while let Some(source) = open.pop() {
            match source {
                Source::Layer(id) | Source::Art(id, _) if self.layer(id).is_some() && !stack.contains(&id) => stack.push(id),
                Source::Layer(_) | Source::Art(..) => {}
                // The top of what it stacks is listed first: its first input, or stacked by its
                // ports with the right on top, its last.
                Source::Composite(id) if seen.insert(source) => {
                    let Some(composite) = self.composite(id) else { continue };
                    match composite.stacking == Stacking::Ports && composite.right_first {
                        true => open.extend(composite.inputs.iter()),
                        false => open.extend(composite.inputs.iter().rev()),
                    }
                }
                // What goes through an effect is seen where the effect is stacked, and what a
                // Send is given where a Receive that reads it is.
                Source::Effect(id) if seen.insert(source) => {
                    let Some(effect) = self.effect(id) else { continue };
                    match effect.kind == crate::RECEIVE {
                        true => open.extend(self.sender(effect).map(|send| Source::Effect(send.id))),
                        false => open.extend(effect.picture()),
                    }
                }
                Source::Composite(_) | Source::Effect(_) => {}
            }
        }
        stack
    }

    /// The shot's layers in the order they are listed: those that are seen as the display
    /// stacks them, top first, then the rest.
    pub fn listed(&self) -> Vec<&Layer> {
        let stack = self.stack();
        let mut listed: Vec<&Layer> = stack.iter().filter_map(|id| self.layer(*id)).collect();
        listed.extend(self.layers.iter().map(|layer| &**layer).filter(|layer| !stack.contains(&layer.id)));
        listed
    }

    /// Where a layer is among the inputs of the composite it is connected to, from the top,
    /// and how many inputs that composite has.
    pub fn place(&self, layer: LayerId) -> Option<(usize, usize)> {
        let (composite, tip) = (self.composite_of(Source::Layer(layer))?, self.tip(Source::Layer(layer)));
        Some((composite.inputs.iter().position(|input| *input == Source::Layer(layer) || *input == tip)?, composite.inputs.len()))
    }

    /// The ids of the shot's nodes: its layers, pegs, composites, deformers, groups, effects and
    /// drivers, and its camera.
    pub fn node_ids(&self) -> impl Iterator<Item = u64> {
        let (layers, pegs) = (self.layers.iter().map(|l| l.id.0), self.pegs.iter().map(|p| p.id.0));
        let pegs = pegs.chain(self.drivers.iter().map(|d| d.id.0));
        let groups = self.groups.iter().map(|g| g.id.0).chain(self.effects.iter().map(|e| e.id.0)).chain(self.displays.iter().map(|d| d.id.0));
        layers.chain(pegs).chain(self.composites.iter().map(|c| c.id.0)).chain(self.deformers.iter().map(|d| d.id.0)).chain(groups).chain([self.camera.id.0])
    }

    /// The names of the shot's nodes, of every kind. No two share one.
    pub fn node_names(&self) -> impl Iterator<Item = &str> + Clone {
        let (layers, pegs) = (self.layers.iter().map(|l| l.name.as_str()), self.pegs.iter().map(|p| p.name.as_str()));
        let pegs = pegs.chain(self.drivers.iter().map(|d| d.name.as_str()));
        let groups = self.groups.iter().map(|g| g.name.as_str()).chain(self.effects.iter().map(|e| e.name.as_str()));
        let groups = groups.chain(self.displays.iter().map(|d| d.name.as_str()));
        layers.chain(pegs).chain(self.composites.iter().map(|c| c.name.as_str())).chain(self.deformers.iter().map(|d| d.name.as_str())).chain(groups)
    }

    /// Connect a layer on top of the composite new layers go to, if the shot has one: the end
    /// of its wire, if it has effects on it.
    pub fn connect_on_top(&mut self, layer: LayerId) {
        let (Some(main), tip) = (self.main_composite(), self.tip(Source::Layer(layer))) else { return };
        if let Some(composite) = self.composites.iter_mut().find(|c| c.id == main) {
            std::sync::Arc::make_mut(composite).inputs.insert(0, tip);
        }
    }

    /// Wherever `from` is wired to, wire `to` there instead: in the composites that stack it,
    /// in the effects' ports and on the displays. With `None` it is taken out.
    pub fn rewire(&mut self, from: Source, to: Option<Source>) {
        let Some(to) = to else { return self.disconnect(from) };
        for composite in self.composites.iter_mut().filter(|c| c.inputs.contains(&from)) {
            let composite = std::sync::Arc::make_mut(composite);
            // A composite stacks a node once: if it has the other already, this one only goes.
            match composite.inputs.contains(&to) {
                true => composite.inputs.retain(|input| *input != from),
                false => composite.inputs.iter_mut().filter(|input| **input == from).for_each(|input| *input = to),
            }
        }
        for effect in self.effects.iter_mut().filter(|e| e.inputs.values().any(|input| *input == from)) {
            std::sync::Arc::make_mut(effect).inputs.values_mut().filter(|input| **input == from).for_each(|input| *input = to);
        }
        if self.display == Some(from) {
            self.display = Some(to);
        }
        for display in self.displays.iter_mut().filter(|display| display.shows == Some(from)) {
            display.shows = Some(to);
        }
    }

    /// Wherever `from`'s picture goes on as a picture, wire `to` there instead: as
    /// [`Shot::rewire`], but a matte port of an effect, which takes it to cut or to mask with
    /// and not to change it, keeps its wire from `from`. This is the wire an effect is put on.
    pub fn rewire_picture(&mut self, from: Source, to: Source) {
        // The mattes wired from it, which stay as they are.
        let mattes: Vec<(crate::EffectId, String)> = self
            .effects
            .iter()
            .flat_map(|effect| {
                effect.inputs.iter().filter(|(port, input)| **input == from && port.as_str() != crate::PICTURE).map(|(port, _)| (effect.id, port.clone()))
            })
            .collect();
        self.rewire(from, Some(to));
        for (effect, port) in mattes {
            if let Some(effect) = self.effects.iter_mut().find(|own| own.id == effect) {
                std::sync::Arc::make_mut(effect).inputs.insert(port, from);
            }
        }
    }

    /// Take a node out of every composite, out of every effect's ports and off the displays.
    /// A layer goes with the outputs of its art layers.
    pub fn disconnect(&mut self, source: Source) {
        let gone = |input: &Source| *input == source || matches!((source, *input), (Source::Layer(of), Source::Art(art, _)) if of == art);
        for composite in self.composites.iter_mut().filter(|c| c.inputs.iter().any(gone)) {
            std::sync::Arc::make_mut(composite).inputs.retain(|input| !gone(input));
        }
        for effect in self.effects.iter_mut().filter(|e| e.inputs.values().any(gone)) {
            std::sync::Arc::make_mut(effect).inputs.retain(|_, input| !gone(input));
        }
        if self.display.as_ref().is_some_and(gone) {
            self.display = None;
        }
        for display in self.displays.iter_mut().filter(|display| display.shows.as_ref().is_some_and(gone)) {
            display.shows = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{Project, Role};

    #[test]
    fn the_stack_is_what_the_display_shows_with_composites_opened_up() {
        let mut project = Project::default();
        let layers: Vec<LayerId> = ["hat", "head", "sky", "spare"].map(|name| project.new_layer(name, Role::Animation)).map(|l| l.id).to_vec();
        let (character, all) = (CompositeId(project.new_id()), CompositeId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let [hat, head, sky, spare] = [0, 1, 2, 3].map(|i| Source::Layer(layers[i]));
        // No layer of these is in the shot yet: nothing of them is seen.
        shot.composites = vec![
            Arc::new(Composite::new(character, "Character", vec![hat, head])),
            Arc::new(Composite::new(all, "All", vec![Source::Composite(character), sky, hat])),
        ];
        shot.display = Some(Source::Composite(all));
        assert_eq!(shot.stack(), []);
        let made = |id: LayerId| Arc::new(crate::Layer { id, ..Project::default().new_layer("x", Role::Animation) });
        shot.layers = layers.iter().map(|id| made(*id)).collect();
        assert_eq!(shot.stack(), [layers[0], layers[1], layers[2]], "the character over the sky; the spare layer is connected to nothing");
        assert_eq!((shot.main_composite(), shot.composite_of(hat).map(|c| c.id)), (Some(all), Some(character)));
        assert!(shot.feeds(character, all) && !shot.feeds(all, character) && shot.feeds(all, all));
        // Stacked by its ports with the right on top, the character lists its head first.
        let character_at = shot.composites.iter().position(|c| c.id == character).unwrap();
        let made_2d = Arc::make_mut(&mut shot.composites[character_at]);
        (made_2d.stacking, made_2d.right_first) = (Stacking::Ports, true);
        assert_eq!(shot.stack(), [layers[1], layers[0], layers[2]]);
        Arc::make_mut(&mut shot.composites[character_at]).right_first = false;
        assert_eq!(shot.stack(), [layers[0], layers[1], layers[2]]);
        // A Receive lists what its Send is given where the Receive is.
        let kind = crate::effect_kind(crate::SEND).unwrap();
        let mut send = crate::Effect::new(crate::EffectId(900), "Spare Out", kind);
        send.inputs.insert(crate::PICTURE.to_string(), spare);
        if let Some(tag) = send.param_mut(crate::TAG) {
            tag.value = crate::Value::Text("Spare Out".to_string());
        }
        let mut receive = crate::Effect::new(crate::EffectId(901), "Spare In", crate::effect_kind(crate::RECEIVE).unwrap());
        if let Some(tag) = receive.param_mut(crate::TAG) {
            tag.value = crate::Value::Text("Spare Out".to_string());
        }
        shot.effects = vec![Arc::new(send), Arc::new(receive)];
        Arc::make_mut(&mut shot.composites[character_at]).inputs.insert(0, Source::Effect(crate::EffectId(901)));
        assert_eq!(shot.stack(), [layers[3], layers[0], layers[1], layers[2]]);
        Arc::make_mut(&mut shot.composites[character_at]).inputs.remove(0);
        shot.effects.clear();

        // The display can show one layer alone, or nothing.
        shot.display = Some(spare);
        assert_eq!(shot.stack(), [layers[3]]);
        shot.display = Some(Source::Composite(all));
        shot.disconnect(Source::Composite(character));
        assert_eq!(shot.stack(), [layers[2], layers[0]]);
        shot.connect_on_top(layers[3]);
        assert_eq!(shot.stack(), [layers[3], layers[2], layers[0]]);
        shot.disconnect(Source::Composite(all));
        assert_eq!((shot.stack(), shot.display), (vec![], None));
        // Bad data: composites in a ring are walked once.
        Arc::make_mut(&mut shot.composites[0]).inputs = vec![Source::Composite(all)];
        Arc::make_mut(&mut shot.composites[1]).inputs = vec![Source::Composite(character), sky];
        shot.display = Some(Source::Composite(all));
        assert_eq!(shot.stack(), [layers[2]]);
    }
}
