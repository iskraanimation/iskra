//! Applying a library template onto nodes a shot already has (plan §4.12). Where putting a
//! template into a shot makes copies, applying finds the shot's own nodes by their names and
//! gives them the template's poses, animation, timing and drawings: a walk saved from one shot
//! goes onto the same character in another. What does not match is listed, not refused.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::{
    CompositeId, Deformer, DrawingId, Exposure, GroupId, Layer, Node, Peg, PegId, PoolDrawing, Project, Shot, ShotId, SkinId, Source, Transform, Variant,
};

/// How applying is asked to go.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyOptions {
    /// Left and right change places: what the template has for `armL` goes onto `armR`, and
    /// the pose is mirrored, moving and turning the other way.
    pub mirror: bool,
    /// Names matched by hand: a node of the template, by its name, to a node of the shot, by
    /// its name.
    pub map: BTreeMap<String, String>,
    /// Where the template's first frame goes. Without it, its frames are the shot's.
    pub frame: Option<u32>,
    /// Keep the keyframes and timing the shot's nodes have, and put the template's over them.
    /// Without it, a node's animation is the template's alone.
    pub merge: bool,
    /// A drawing the shot has under the same name is replaced by the template's. Without it
    /// the shot's is kept.
    pub replace_drawings: bool,
    /// Each such drawing's own choice, which goes before `replace_drawings`: by the template's
    /// layer, as [`Match::item`] names it, and the drawing's name, whether it is replaced.
    pub drawings: BTreeMap<String, BTreeMap<String, bool>>,
    /// A node of the template that the shot has not got is made, as a copy of the template's,
    /// under the node its parent went onto. Without it, it is left out.
    pub make_missing: bool,
}

/// How a node of the template fared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// The shot has it under the same name, in the same place of the tree.
    Matched,
    /// The shot has it under the same name, elsewhere under where the template is applied.
    Elsewhere,
    /// It was matched by hand.
    Mapped,
    /// The shot has no node of that name: it is left out.
    Missing,
    /// The shot has a node of that name of the other kind, a peg for a layer: left out.
    OtherKind,
    /// The shot has no node of that name, and one is made for it.
    Made,
}

impl Found {
    /// Whether the template's node is applied.
    pub fn applies(self) -> bool {
        matches!(self, Found::Matched | Found::Elsewhere | Found::Mapped | Found::Made)
    }

    pub fn label(self) -> &'static str {
        match self {
            Found::Matched => "matched",
            Found::Elsewhere => "found elsewhere",
            Found::Mapped => "matched by hand",
            Found::Missing => "missing in the shot",
            Found::OtherKind => "another kind of node",
            Found::Made => "made new",
        }
    }
}

/// What kind of node a match is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Peg,
    Layer,
    Composite,
    Group,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Peg => "peg",
            Kind::Layer => "layer",
            Kind::Composite => "composite",
            Kind::Group => "group",
        }
    }
}

/// One node of the template, and what became of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// The template's node, by its names from the top of the template down.
    pub item: String,
    /// The shot's node it goes onto, by its name.
    pub rig: Option<String>,
    pub kind: Kind,
    pub found: Found,
    /// What is worth saying beside: drawings kept or added, bones that differ.
    pub note: Option<String>,
    /// The drawings of the template's layer that the shot's has under the same names, drawn
    /// otherwise: each is kept or replaced, as asked.
    pub conflicts: Vec<String>,
}

/// What applying a template does, or would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub matches: Vec<Match>,
    /// The shot's nodes under where it is applied that the template says nothing about, by
    /// name: they are left as they are.
    pub untouched: Vec<String>,
}

impl Applied {
    pub fn applied(&self) -> usize {
        self.matches.iter().filter(|found| found.found.applies()).count()
    }

    pub fn skipped(&self) -> usize {
        self.matches.len() - self.applied()
    }
}

/// A name as it is compared: without case, separators and the ways a side is written, so that
/// `leg_L`, `legL` and `LegLeft` are one name. With `mirror`, the sides change places.
pub fn compared(name: &str, mirror: bool) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut lower = false;
    for letter in name.chars() {
        if !letter.is_alphanumeric() {
            words.push(String::new());
        } else if letter.is_uppercase() && lower || words.is_empty() {
            words.push(letter.to_lowercase().collect());
        } else if let Some(word) = words.last_mut() {
            word.extend(letter.to_lowercase());
        }
        lower = letter.is_lowercase();
    }
    let side = |word: String| match (word.as_str(), mirror) {
        ("l" | "left", false) | ("r" | "right", true) => "l".to_string(),
        ("r" | "right", false) | ("l" | "left", true) => "r".to_string(),
        _ => word,
    };
    words.into_iter().map(side).collect::<Vec<_>>().join("")
}

/// A name without the number a copy is given where the shot has the name already
/// ("armL 2"): a character put in twice is told apart by it.
fn bare(name: &str) -> &str {
    match name.rsplit_once(' ') {
        Some((base, number)) if !base.is_empty() && !number.is_empty() && number.bytes().all(|digit| digit.is_ascii_digit()) => base,
        _ => name,
    }
}

/// A shot's pegs and layers under `top`, each with its names from there down, parents first.
/// With `top` at `None`, all of them from the top of the shot.
fn tree(shot: &Shot, top: Option<PegId>) -> Vec<(Node, Vec<String>)> {
    let mut listed = Vec::new();
    let mut open: Vec<(Option<PegId>, Vec<String>)> = vec![(top, Vec::new())];
    while let Some((parent, path)) = open.pop() {
        for peg in shot.pegs.iter().filter(|peg| peg.parent == parent) {
            let below: Vec<String> = path.iter().cloned().chain([peg.name.clone()]).collect();
            listed.push((Node::Peg(peg.id), below.clone()));
            if below.len() < 64 {
                open.push((Some(peg.id), below));
            }
        }
        for layer in shot.layers.iter().filter(|layer| layer.parent == parent) {
            listed.push((Node::Layer(layer.id), path.iter().cloned().chain([layer.name.clone()]).collect()));
        }
    }
    listed
}

/// Which of the shot's nodes each node of the template goes onto.
struct Plan {
    applied: Applied,
    /// The template's node, the shot's, and which of the matches says so.
    pairs: Vec<(Node, Node, usize)>,
    /// The template's nodes to be made in the shot, parents first, and which of the matches
    /// says so.
    made: Vec<(Node, usize)>,
    /// The peg what is made goes under where its parent went onto nothing: the one applied onto.
    under: Option<PegId>,
    /// The template's composites, the shot's each goes onto, and which of the matches says so.
    composites: Vec<(CompositeId, CompositeId, usize)>,
    /// The template's groups and the shot's of the same names: what is in the one is looked
    /// for in the other, and what is made in the one goes in the other.
    groups: HashMap<GroupId, GroupId>,
}

/// The one thing there is, if there is only one.
fn one<T>(mut found: impl Iterator<Item = T>) -> Option<T> {
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

fn id_of(node: Node) -> u64 {
    match node {
        Node::Peg(id) => id.0,
        Node::Layer(id) => id.0,
    }
}

fn plan(shot: &Shot, item: &Shot, onto: Option<Node>, options: &ApplyOptions) -> Plan {
    let mut plan = Plan { applied: Applied::default(), pairs: Vec::new(), made: Vec::new(), under: None, composites: Vec::new(), groups: HashMap::new() };
    let name_of = |shot: &Shot, node: Node| match node {
        Node::Peg(id) => shot.peg(id).map(|peg| peg.name.clone()),
        Node::Layer(id) => shot.layer(id).map(|layer| layer.name.clone()),
    };
    let is_peg = |node: Node| matches!(node, Node::Peg(_));
    let kind = |node: Node| if is_peg(node) { Kind::Peg } else { Kind::Layer };
    let matched = |item: String, rig: Option<String>, kind: Kind, found: Found| Match { item, rig, kind, found, note: None, conflicts: Vec::new() };
    let mut items = tree(item, None);
    // Onto a layer, the template's one layer is that layer, whatever it is called.
    if let Some(Node::Layer(_)) = onto {
        let layers: Vec<_> = items.iter().filter(|(node, _)| !is_peg(*node)).cloned().collect();
        if let ([(only, path)], Some(target)) = (&layers[..], onto) {
            plan.pairs.push((*only, target, plan.applied.matches.len()));
            plan.applied.matches.push(matched(path.join("/"), name_of(shot, target), Kind::Layer, Found::Matched));
        }
        return conflicts(plan, shot, item);
    }
    // The name of the shot's that a name of the template's is matched to, as it is compared,
    // and whether that is by hand.
    let wanted = |name: &str| match options.map.get(name) {
        Some(to) => (compared(to, false), true),
        None => (compared(name, options.mirror), false),
    };
    // Groups first, by their names: what is in one of the template's is looked for in the
    // shot's of that name alone, so that one of two characters alike is told from the other.
    let mut taken_groups: HashSet<GroupId> = HashSet::new();
    for group in &item.groups {
        let (name, by_hand) = wanted(&group.name);
        let free = || shot.groups.iter().filter(|own| !taken_groups.contains(&own.id));
        let own = one(free().filter(|own| compared(&own.name, false) == name))
            .or_else(|| one(free().filter(|own| !by_hand && compared(bare(&own.name), false) == compared(bare(&group.name), options.mirror))));
        let found = match (own, by_hand) {
            (Some(_), true) => Found::Mapped,
            (Some(_), false) => Found::Matched,
            (None, _) => Found::Missing,
        };
        if let Some(own) = own {
            taken_groups.insert(own.id);
            plan.groups.insert(group.id, own.id);
        }
        plan.applied.matches.push(matched(group.name.clone(), own.map(|own| own.name.clone()), Kind::Group, found));
    }
    // What of the shot a node of the template is looked for in: the nodes of the group of
    // the shot's that its nearest matched group goes onto, or anywhere.
    let mut scopes: HashMap<GroupId, BTreeSet<u64>> = HashMap::new();
    let mut scope_of = |node: u64| -> Option<BTreeSet<u64>> {
        let own = item.groups_above(node).find_map(|group| plan.groups.get(&group.id).copied())?;
        Some(scopes.entry(own).or_insert_with(|| shot.group_nodes(own)).clone())
    };
    // Onto a peg, a template that has one peg at its top is that peg, and the rest is matched
    // below it; else the template's tops are matched among what the peg carries.
    let under = match onto {
        Some(Node::Peg(id)) => Some(id),
        _ => None,
    };
    plan.under = under;
    let tops: Vec<Node> = items.iter().filter(|(_, path)| path.len() == 1).map(|(node, _)| *node).collect();
    let mut taken: HashSet<Node> = HashSet::new();
    if let (Some(target), [Node::Peg(top)]) = (onto.filter(|node| is_peg(*node)), &tops[..]) {
        let top = Node::Peg(*top);
        plan.pairs.push((top, target, plan.applied.matches.len()));
        taken.insert(target);
        plan.applied.matches.push(matched(name_of(item, top).unwrap_or_default(), name_of(shot, target), Kind::Peg, Found::Matched));
        items.retain(|(node, _)| *node != top);
        for (_, path) in &mut items {
            path.remove(0);
        }
    }
    let rig = tree(shot, under);
    let key = |path: &[String], mirror: bool| path.iter().map(|name| compared(name, mirror)).collect::<Vec<_>>().join("/");
    let bare_key = |path: &[String], mirror: bool| path.iter().map(|name| compared(bare(name), mirror)).collect::<Vec<_>>().join("/");
    let mut by_path: HashMap<String, Vec<Node>> = HashMap::new();
    let mut by_name: HashMap<String, Vec<Node>> = HashMap::new();
    let (mut by_bare_path, mut by_bare_name): (HashMap<String, Vec<Node>>, HashMap<String, Vec<Node>>) = Default::default();
    for (node, path) in &rig {
        let name = path.last().map_or("", String::as_str);
        by_path.entry(key(path, false)).or_default().push(*node);
        by_name.entry(compared(name, false)).or_default().push(*node);
        by_bare_path.entry(bare_key(path, false)).or_default().push(*node);
        by_bare_name.entry(compared(bare(name), false)).or_default().push(*node);
    }
    for (node, path) in &items {
        let name = path.last().cloned().unwrap_or_default();
        let scope = scope_of(id_of(*node));
        let free = |found: &Vec<Node>, taken: &HashSet<Node>| {
            let within = |node: &Node| scope.as_ref().is_none_or(|scope| scope.contains(&id_of(*node)));
            found.iter().copied().filter(|node| !taken.contains(node) && within(node)).collect::<Vec<_>>()
        };
        // By hand first; then where it is in the tree; then by its name alone, if one node
        // under here has it.
        let by_hand = options.map.get(&name).and_then(|to| by_name.get(&compared(to, false))).map(|found| free(found, &taken));
        let mut at_path = by_path.get(&key(path, options.mirror)).map(|found| free(found, &taken)).unwrap_or_default();
        let mut named = by_name.get(&compared(&name, options.mirror)).map(|found| free(found, &taken)).unwrap_or_default();
        // In a group, a copy's number is not looked at: "armL" of the template's "Hero" is
        // "armL 2" of the shot's "Hero 2".
        if scope.is_some() && at_path.is_empty() && named.is_empty() {
            at_path = by_bare_path.get(&bare_key(path, options.mirror)).map(|found| free(found, &taken)).unwrap_or_default();
            named = by_bare_name.get(&compared(bare(&name), options.mirror)).map(|found| free(found, &taken)).unwrap_or_default();
        }
        let (found, onto) = match (by_hand.as_deref(), &at_path[..], &named[..]) {
            (Some([one]), ..) => (Found::Mapped, Some(*one)),
            (_, [one, ..], _) => (Found::Matched, Some(*one)),
            (_, [], [one]) => (Found::Elsewhere, Some(*one)),
            _ => (Found::Missing, None),
        };
        let found = if onto.is_some_and(|onto| is_peg(onto) != is_peg(*node)) { Found::OtherKind } else { found };
        // Missing, it is made if that is asked for.
        let found = if found == Found::Missing && options.make_missing {
            plan.made.push((*node, plan.applied.matches.len()));
            Found::Made
        } else {
            found
        };
        if let (true, Some(onto)) = (found.applies(), onto) {
            taken.insert(onto);
            plan.pairs.push((*node, onto, plan.applied.matches.len()));
        }
        let rig = onto.filter(|_| !matches!(found, Found::Missing | Found::Made)).and_then(|onto| name_of(shot, onto));
        plan.applied.matches.push(matched(path.join("/"), rig, kind(*node), found));
    }
    plan.applied.untouched = rig.iter().filter(|(node, _)| !taken.contains(node)).map(|(_, path)| path.join("/")).collect();

    // Composites, by their names, for how they stack what both have. The template's first
    // stands for the shot it was saved from, and goes onto the shot's own: listed only where
    // it orders two or more of what is matched.
    let main = shot.main_composite();
    let mut taken_composites: HashSet<CompositeId> = HashSet::new();
    let mut sources: HashMap<Source, Source> = plan
        .pairs
        .iter()
        .filter_map(
            |(from, onto, _)| if let (Node::Layer(from), Node::Layer(onto)) = (from, onto) { Some((Source::Layer(*from), Source::Layer(*onto))) } else { None },
        )
        .collect();
    for composite in item.composites.iter().skip(1) {
        let scope = scope_of(composite.id.0);
        let free = |own: &&Arc<crate::Composite>| {
            Some(own.id) != main && !taken_composites.contains(&own.id) && scope.as_ref().is_none_or(|scope| scope.contains(&own.id.0))
        };
        let (name, by_hand) = wanted(&composite.name);
        let own = one(shot.composites.iter().filter(free).filter(|own| compared(&own.name, false) == name)).or_else(|| {
            let alike = |own: &&Arc<crate::Composite>| !by_hand && compared(bare(&own.name), false) == compared(bare(&composite.name), options.mirror);
            one(shot.composites.iter().filter(free).filter(alike).filter(|_| scope.is_some()))
        });
        let found = match (own, by_hand) {
            (Some(_), true) => Found::Mapped,
            (Some(_), false) => Found::Matched,
            (None, _) => Found::Missing,
        };
        if let Some(own) = own {
            taken_composites.insert(own.id);
            sources.insert(Source::Composite(composite.id), Source::Composite(own.id));
            plan.composites.push((composite.id, own.id, plan.applied.matches.len()));
        }
        plan.applied.matches.push(matched(composite.name.clone(), own.map(|own| own.name.clone()), Kind::Composite, found));
    }
    if let (Some(stand_in), Some(own)) = (item.composites.first(), main.and_then(|main| shot.composite(main))) {
        let ordered = stand_in.inputs.iter().filter(|input| sources.get(input).is_some_and(|onto| own.inputs.contains(onto))).count();
        if ordered >= 2 {
            plan.composites.push((stand_in.id, own.id, plan.applied.matches.len()));
            plan.applied.matches.push(matched(stand_in.name.clone(), Some(own.name.clone()), Kind::Composite, Found::Matched));
        }
    }
    conflicts(plan, shot, item)
}

/// The plan with the drawings each matched layer of the template has under a name the shot's
/// layer has too, drawn otherwise.
fn conflicts(mut plan: Plan, shot: &Shot, item: &Shot) -> Plan {
    for (from, onto, listed) in &plan.pairs {
        let (Node::Layer(from), Node::Layer(onto)) = (from, onto) else { continue };
        let (Some(from), Some(own)) = (item.layer(*from), shot.layer(*onto)) else { continue };
        let differ = |entry: &&PoolDrawing| {
            own.drawings.iter().any(|own| own.name == entry.name && !Arc::ptr_eq(&own.drawing, &entry.drawing) && own.drawing != entry.drawing)
        };
        if let Some(found) = plan.applied.matches.get_mut(*listed) {
            found.conflicts = from.drawings.iter().filter(differ).map(|entry| entry.name.clone()).collect();
        }
    }
    plan
}

/// A place or a handle mirrored: across the upright line, to the other side.
fn across([x, y]: [f64; 2]) -> [f64; 2] {
    [-x, y]
}

/// The template with its pose mirrored, left for right: what moves sideways moves the other
/// way, and what turns, leans or swings to one side does to the other. Sizes, depth and what
/// is at rest are as they were.
fn mirrored(item: &Shot) -> Shot {
    let mut item = item.clone();
    for peg in &mut item.pegs {
        let peg = Arc::make_mut(peg);
        let (transform, animation) = (&mut peg.transform, &mut peg.animation);
        transform.position = across(transform.position);
        (transform.rotation, transform.skew, transform.tilt[1]) = (-transform.rotation, -transform.skew, -transform.tilt[1]);
        animation.position = animation.position.mapped(across, across);
        animation.rotation = animation.rotation.mapped(|turn| -turn, across);
        animation.skew = animation.skew.mapped(|lean| -lean, across);
        animation.tilt = animation.tilt.mapped(|[top, side]| [top, -side], across);
    }
    for deformer in &mut item.deformers {
        let deformer = Arc::make_mut(deformer);
        (deformer.offset, deformer.offsets) = (across(deformer.offset), deformer.offsets.mapped(across, across));
        for bone in &mut deformer.bones {
            (bone.turn, bone.turns) = (-bone.turn, bone.turns.mapped(|turn| -turn, across));
        }
        for knot in &mut deformer.spine {
            (knot.moved, knot.bent.into, knot.bent.out) = (across(knot.moved), across(knot.bent.into), across(knot.bent.out));
            for track in [&mut knot.moves, &mut knot.bends_into, &mut knot.bends_out] {
                *track = track.mapped(across, across);
            }
        }
        // The IK's goal goes across, and two straight bones bend the other way, as turned ones do.
        if let Some(ik) = &mut deformer.ik {
            (ik.goal, ik.goals, ik.flip) = (across(ik.goal), ik.goals.mapped(across, across), !ik.flip);
        }
    }
    item
}

/// Give a peg of the shot the pose and the animation of the template's: all but its pivot,
/// which is the rig's own.
fn apply_peg(rig: &mut Peg, item: &Peg, by: u32, merge: bool) {
    let moved = item.animation.shifted(by);
    if !merge {
        rig.animation = moved;
        rig.transform = Transform { pivot: rig.transform.pivot, ..item.transform };
        return;
    }
    macro_rules! over {
        ($($value:ident),*) => {$(
            // A value that neither animates is the template's; else its keys go over the rig's.
            if moved.$value.is_empty() && rig.animation.$value.is_empty() {
                rig.transform.$value = item.transform.$value;
            } else {
                rig.animation.$value.merge(&moved.$value);
            }
        )*};
    }
    over!(position, rotation, scale, skew, depth, tilt);
}

/// Give a deformer of the shot the pose of the template's, if they are alike: the same kind,
/// with as many bones or points. Says why not, if not.
fn apply_bend(rig: &mut Deformer, item: &Deformer, by: u32, merge: bool) -> Option<String> {
    if rig.kind() != item.kind() || rig.bones.len() != item.bones.len() || rig.spine.len() != item.spine.len() {
        return Some(format!("its {} differ from the template's: the pose is not applied", if rig.is_curve() { "points" } else { "bones" }));
    }
    macro_rules! over {
        ($rig:expr, $item:expr, $($value:ident).+, $keys:ident) => {{
            let moved = $item.$keys.shifted(by);
            if !merge {
                ($rig.$($value).+, $rig.$keys) = ($item.$($value).+, moved);
            } else if moved.is_empty() && $rig.$keys.is_empty() {
                $rig.$($value).+ = $item.$($value).+;
            } else {
                $rig.$keys.merge(&moved);
            }
        }};
    }
    over!(rig, item, offset, offsets);
    for (bone, from) in rig.bones.iter_mut().zip(&item.bones) {
        over!(bone, from, turn, turns);
        over!(bone, from, stretch, stretches);
    }
    for (knot, from) in rig.spine.iter_mut().zip(&item.spine) {
        over!(knot, from, moved, moves);
        over!(knot, from, bent.into, bends_into);
        over!(knot, from, bent.out, bends_out);
    }
    // The IK's switch and goal, where both have IK.
    if let (Some(ik), Some(from)) = (rig.ik.as_mut(), item.ik.as_ref()) {
        over!(ik, from, on, ons);
        over!(ik, from, goal, goals);
    }
    None
}

/// Give a layer of the shot the timing of the template's, with the drawings it shows: one the
/// layer has under the same name is kept, or replaced; one it lacks is added. Says how many.
/// The template's variants in skins, in the project's skins `like` says, follow their
/// drawings: one replaced or added brings its variants, one kept only those the layer lacks.
fn apply_layer(
    rig: &mut Layer,
    item: &Layer,
    options: &ApplyOptions,
    listed: &str,
    like: &HashMap<SkinId, SkinId>,
    mut new_id: impl FnMut() -> u64,
) -> Option<String> {
    let chosen = options.drawings.get(listed);
    let (mut kept, mut replaced, mut added) = (0, 0, 0);
    let mut shown: HashMap<DrawingId, DrawingId> = HashMap::new();
    for entry in &item.drawings {
        // Whether the template's variants of it go over the layer's.
        let taken = match rig.drawings.iter_mut().find(|own| own.name == entry.name) {
            Some(own) => {
                shown.insert(entry.id, own.id);
                if Arc::ptr_eq(&own.drawing, &entry.drawing) || own.drawing == entry.drawing {
                    false
                } else if chosen.and_then(|chosen| chosen.get(&entry.name)).copied().unwrap_or(options.replace_drawings) {
                    own.drawing = entry.drawing.clone();
                    replaced += 1;
                    true
                } else {
                    kept += 1;
                    false
                }
            }
            None => {
                let id = DrawingId(new_id());
                shown.insert(entry.id, id);
                rig.drawings.push(PoolDrawing { id, name: entry.name.clone(), drawing: entry.drawing.clone() });
                added += 1;
                true
            }
        };
        let day = shown[&entry.id];
        for variant in item.variants.iter().filter(|variant| variant.of == entry.id) {
            let Some(&skin) = like.get(&variant.skin) else { continue };
            let had = rig.variants.iter().position(|own| own.skin == skin && own.of == day);
            if had.is_some() && !taken {
                continue;
            }
            if let Some(had) = had {
                rig.variants.remove(had);
            }
            rig.variants.push(Variant { skin, of: day, drawing: PoolDrawing { id: DrawingId(new_id()), ..variant.drawing.clone() } });
        }
    }
    if !options.merge {
        rig.exposure = Exposure::default();
    }
    let by = options.frame.unwrap_or(0);
    for run in item.exposure.runs() {
        rig.exposure.set(run.start.saturating_add(by), run.len, shown.get(&run.drawing).copied());
    }
    let said: Vec<String> = [(added, "added"), (replaced, "replaced"), (kept, "kept as the shot has them")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, what)| format!("{count} {} {what}", if count == 1 { "drawing" } else { "drawings" }))
        .collect();
    (!said.is_empty()).then(|| said.join(", "))
}

impl Project {
    /// What applying `template` to a shot would do: which of the shot's nodes each of its
    /// nodes goes onto, by name, and what is left out. `onto` is the node it is applied to: a
    /// peg, with what the peg carries, or a layer; without it, the whole shot.
    pub fn match_template(&self, shot: ShotId, template: &Project, onto: Option<Node>, options: &ApplyOptions) -> Option<Applied> {
        Some(plan(self.shot(shot)?, template.shots.first()?, onto, options).applied)
    }

    /// Apply `template` to a shot (see [`Project::match_template`]): the shot's pegs get the
    /// poses and animation of the template's, its layers their timing and drawings, and what
    /// bends a layer the pose of what bends the template's, and its composites the template's
    /// order of what both stack. Nothing is made but drawings, unless what is missing is asked
    /// to be, and nothing the template does not name is touched.
    pub fn apply_template(&mut self, shot: ShotId, template: &Project, onto: Option<Node>, options: &ApplyOptions) -> Option<Applied> {
        let item = template.shots.first()?;
        let Plan { mut applied, pairs, made, under, composites, groups } = plan(self.shot(shot)?, item, onto, options);
        // Where each of the template's pegs is listed, for what its driver does to be said there.
        let mut listed_peg: HashMap<PegId, usize> =
            pairs.iter().filter_map(|(from, _, listed)| if let Node::Peg(from) = from { Some((*from, *listed)) } else { None }).collect();
        listed_peg.extend(made.iter().filter_map(|(from, listed)| if let Node::Peg(from) = from { Some((*from, *listed)) } else { None }));
        let by = options.frame.unwrap_or(0);
        // The template's skins as the project's of the same names, made where it has none.
        let like = self.skins_like(&template.palette);
        let mut next = self.next_id;
        let rig = self.shot_mut(shot)?;
        // Which of the shot's pegs each of the template's went onto, or was made as: what is
        // made goes under the one its parent did.
        let mut placed: HashMap<PegId, PegId> = pairs
            .iter()
            .filter_map(|(from, onto, _)| if let (Node::Peg(from), Node::Peg(onto)) = (from, onto) { Some((*from, *onto)) } else { None })
            .collect();
        for (from, listed) in made {
            let mut names: Vec<String> = rig.node_names().map(str::to_string).collect();
            let named = |name: &str, names: &mut Vec<String>| {
                let name = crate::unique_name(name, names.iter().map(String::as_str));
                names.push(name.clone());
                name
            };
            let parent_of = |parent: Option<PegId>| parent.and_then(|parent| placed.get(&parent).copied()).or(under);
            // In the shot's group that the template's it is in went onto.
            let group = item.groups_above(id_of(from)).find_map(|group| groups.get(&group.id).copied());
            let (name, note) = match from {
                Node::Peg(from) => {
                    let Some(from) = item.peg(from) else { continue };
                    let id = PegId(next);
                    next += 1;
                    let name = named(&from.name, &mut names);
                    let mut peg = Peg { id, name: name.clone(), parent: parent_of(from.parent), template: None, follows: None, ..from.clone() };
                    peg.animation = peg.animation.shifted(by);
                    peg.matches.clear();
                    placed.insert(from.id, id);
                    rig.pegs.push(Arc::new(peg));
                    rig.add_to_group(group, id.0);
                    (name, None)
                }
                Node::Layer(from) => {
                    let Some(from) = item.layer(from) else { continue };
                    let id = crate::LayerId(next);
                    next += 1;
                    let name = named(&from.name, &mut names);
                    let mut layer = Layer { id, name: name.clone(), parent: parent_of(from.parent), template: None, deformer: None, ..from.clone() };
                    // Its drawings, as copies of the template's, and its timing with them.
                    let mut shown: HashMap<DrawingId, DrawingId> = HashMap::new();
                    for entry in &mut layer.drawings {
                        let drawing = DrawingId(next);
                        next += 1;
                        shown.insert(entry.id, drawing);
                        entry.id = drawing;
                    }
                    // And its variants, in the project's skins of the same names.
                    layer.variants.clear();
                    for variant in &from.variants {
                        let (Some(&skin), Some(&of)) = (like.get(&variant.skin), shown.get(&variant.of)) else { continue };
                        layer.variants.push(Variant { skin, of, drawing: PoolDrawing { id: DrawingId(next), ..variant.drawing.clone() } });
                        next += 1;
                    }
                    layer.exposure = Exposure::default();
                    for run in from.exposure.runs() {
                        layer.exposure.set(run.start.saturating_add(by), run.len, shown.get(&run.drawing).copied());
                    }
                    rig.layers.insert(0, Arc::new(layer));
                    rig.connect_on_top(id);
                    rig.add_to_group(group, id.0);
                    let note = from.deformer.is_some().then(|| "made without the bones that bend the template's".to_string());
                    (name, note)
                }
            };
            if let Some(found) = applied.matches.get_mut(listed) {
                (found.rig, found.note) = (Some(name), note);
            }
        }
        // Mirrored, what goes onto the shot's own nodes has its pose mirrored too; what is made
        // is the template's as it is, since its drawings are.
        let flipped = options.mirror.then(|| mirrored(item));
        let item = flipped.as_ref().unwrap_or(item);
        // What a picture of the template's is, of the shot's: for how composites stack them.
        let mut sources: HashMap<Source, Source> = pairs
            .iter()
            .filter_map(
                |(from, onto, _)| {
                    if let (Node::Layer(from), Node::Layer(onto)) = (from, onto) { Some((Source::Layer(*from), Source::Layer(*onto))) } else { None }
                },
            )
            .collect();
        sources.extend(composites.iter().map(|(from, onto, _)| (Source::Composite(*from), Source::Composite(*onto))));
        for (from, onto, listed) in pairs {
            let note = match (from, onto) {
                (Node::Peg(from), Node::Peg(onto)) => {
                    if let (Some(from), Some(onto)) = (item.peg(from), rig.pegs.iter_mut().find(|peg| peg.id == onto)) {
                        apply_peg(Arc::make_mut(onto), from, by, options.merge);
                    }
                    None
                }
                (Node::Layer(from), Node::Layer(onto)) => {
                    let (Some(from), Some(own)) = (item.layer(from), rig.layers.iter_mut().find(|layer| layer.id == onto)) else { continue };
                    let listed_as = applied.matches.get(listed).map(|found| found.item.clone()).unwrap_or_default();
                    let drawings = apply_layer(Arc::make_mut(own), from, options, &listed_as, &like, || {
                        next += 1;
                        next - 1
                    });
                    // What bends the one is posed as what bends the other.
                    let bends = (from.deformer.and_then(|id| item.deformer(id)), own.deformer);
                    let bones = match bends {
                        (Some(from), Some(own)) => rig.deformer_mut(own).and_then(|own| apply_bend(own, from, by, options.merge)),
                        (Some(_), None) => Some("the template's layer is bent and the shot's is not: the pose is not applied".to_string()),
                        _ => None,
                    };
                    let said: Vec<String> = drawings.into_iter().chain(bones).collect();
                    (!said.is_empty()).then(|| said.join("; "))
                }
                _ => None,
            };
            if let Some(found) = applied.matches.get_mut(listed) {
                found.note = note;
            }
        }
        // A composite stacks what both have in the order the template's does, in the places
        // they had; the rest stays where it is. One of the shot's own, and not the shot's main
        // one, also stacks as the template's does.
        let main = item.composites.first().map(|composite| composite.id);
        for (from, onto, listed) in composites {
            let (Some(from), Some(own)) = (item.composite(from), rig.composites.iter_mut().find(|composite| composite.id == onto)) else { continue };
            let own = Arc::make_mut(own);
            let order: Vec<Source> = from.inputs.iter().filter_map(|input| sources.get(input).copied()).filter(|input| own.inputs.contains(input)).collect();
            let places: Vec<usize> = own.inputs.iter().enumerate().filter(|(_, input)| order.contains(input)).map(|(at, _)| at).collect();
            let moved = places.iter().zip(&order).filter(|(at, input)| own.inputs[**at] != **input).count();
            for (at, input) in places.into_iter().zip(order) {
                own.inputs[at] = input;
            }
            if Some(from.id) != main {
                (own.stacking, own.right_first) = (from.stacking, from.right_first);
            }
            if let Some(found) = applied.matches.get_mut(listed) {
                found.note = (moved > 0).then(|| format!("{moved} {} restacked", if moved == 1 { "input" } else { "inputs" }));
            }
        }
        // A driver of the template's goes onto the driver of the peg its own peg went onto,
        // or is made there (plan §8.8): its expressions, its parameters with their keys, and
        // its inputs wired to where the template's pegs went, else as the shot's had them.
        for from in &item.drivers {
            let Some(&onto) = placed.get(&from.drives) else { continue };
            let mut parameters = from.parameters.clone();
            for parameter in &mut parameters {
                parameter.track = parameter.track.shifted(by);
            }
            let inputs = |own: Option<&crate::Driver>| -> Vec<crate::DriverInput> {
                let wired =
                    |input: &crate::DriverInput| input.peg.and_then(|peg| placed.get(&peg).copied()).or(own.and_then(|own| own.input(&input.name)?.peg));
                from.inputs.iter().map(|input| crate::DriverInput { name: input.name.clone(), peg: wired(input) }).collect()
            };
            let said = match rig.drivers.iter_mut().find(|driver| driver.drives == onto) {
                Some(own) => {
                    let own = Arc::make_mut(own);
                    let wired = inputs(Some(own));
                    // Merged, the shot's keys stay and the template's go over them.
                    let parameters = parameters.into_iter().map(|mut parameter| {
                        if let (true, Some(had)) = (options.merge, own.parameter(&parameter.name)) {
                            let mut track = had.track.clone();
                            track.merge(&parameter.track);
                            parameter.track = track;
                        }
                        parameter
                    });
                    let parameters: Vec<crate::DriverParameter> = parameters.collect();
                    // What the shot's had besides stays, for anything else it reads.
                    let kept: Vec<crate::DriverParameter> =
                        own.parameters.iter().filter(|had| !parameters.iter().any(|p| p.name == had.name)).cloned().collect();
                    let kept_inputs: Vec<crate::DriverInput> =
                        own.inputs.iter().filter(|had| !wired.iter().any(|input| input.name == had.name)).cloned().collect();
                    (own.expressions, own.preset, own.off) = (from.expressions.clone(), from.preset.clone(), from.off);
                    own.parameters = parameters.into_iter().chain(kept).collect();
                    own.inputs = wired.into_iter().chain(kept_inputs).collect();
                    "its driver applied"
                }
                None => {
                    let names: Vec<String> = rig.node_names().map(str::to_string).collect();
                    let id = crate::DriverId(next);
                    next += 1;
                    let name = crate::unique_name(&from.name, names.iter().map(String::as_str));
                    let made = crate::Driver { id, name, drives: onto, inputs: inputs(None), parameters, template: None, ..(**from).clone() };
                    rig.drivers.push(Arc::new(made));
                    "its driver made"
                }
            };
            if let Some(found) = listed_peg.get(&from.drives).and_then(|listed| applied.matches.get_mut(*listed)) {
                found.note = Some(found.note.take().map_or(said.to_string(), |note| format!("{note}; {said}")));
            }
        }
        // An input that would now have a driven peg move itself is let go.
        while let Some(looping) = rig.driver_in_a_loop().map(|driver| driver.id) {
            let Some(driver) = rig.drivers.iter_mut().find(|driver| driver.id == looping) else { break };
            for input in &mut Arc::make_mut(driver).inputs {
                input.peg = None;
            }
        }
        self.next_id = next;
        Some(applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Animation, Bend, Bone, Key, Part, Role, Track};
    use std::collections::HashSet;

    #[test]
    fn names_are_compared_without_case_separators_or_the_way_a_side_is_written() {
        assert!(compared("leg_L", false) == compared("legL", false) && compared("legL", false) == compared("LegLeft", false));
        assert!(compared("arm.R", false) == compared("armRight", false) && compared("armL", false) != compared("armR", false));
        assert_eq!(compared("armL", true), compared("arm_r", false));
        assert!(compared("Hips", false) == "hips" && compared("ball 2", false) == "ball2" && compared("roll", true) == "roll");
    }

    /// A character: hips over two legs, each a peg over a layer. Returns the hips.
    fn character(project: &mut Project) -> PegId {
        let ids: Vec<u64> = (0..3).map(|_| project.new_id()).collect();
        let (leg_l, leg_r) = (project.new_layer("legL", Role::Animation), project.new_layer("legR", Role::Animation));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let peg = |id: u64, name: &str, parent: Option<u64>| {
            Arc::new(Peg {
                id: PegId(id),
                name: name.to_string(),
                parent: parent.map(PegId),
                transform: Transform::default(),
                animation: Animation::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        };
        shot.pegs.extend([peg(ids[0], "hips", None), peg(ids[1], "legL-P", Some(ids[0])), peg(ids[2], "legR-P", Some(ids[0]))]);
        for (mut layer, parent) in [(leg_l, ids[1]), (leg_r, ids[2])] {
            layer.parent = Some(PegId(parent));
            shot.layers.push(Arc::new(layer));
        }
        PegId(ids[0])
    }

    #[test]
    fn a_template_is_applied_onto_the_nodes_of_a_shot_that_have_its_names() {
        let mut project = Project::default();
        let hips = character(&mut project);
        let shot = project.shots[0].id;
        let peg = |project: &Project, name: &str| project.shots[0].pegs.iter().find(|peg| peg.name == name).map(|peg| (**peg).clone()).unwrap();
        // A pose is saved from it: the hips moved and keyed, the left leg turned.
        let mut posed = project.clone();
        {
            let shot = Arc::make_mut(&mut posed.shots[0]);
            let hips = Arc::make_mut(shot.pegs.iter_mut().find(|peg| peg.name == "hips").unwrap());
            hips.transform.pivot = [9.0, 9.0];
            hips.animation.position = Track::default();
            hips.animation.position.set(0, [0.0, 0.0]);
            hips.animation.position.set(8, [50.0, 0.0]);
            Arc::make_mut(shot.pegs.iter_mut().find(|peg| peg.name == "legL-P").unwrap()).transform.rotation = 30.0;
        }
        let template = posed.template(shot, &[Part::Peg(hips)], "Walk").unwrap();

        // The shot has since been changed: a scarf was added, and a leg renamed.
        let scarf = project.new_layer("scarf", Role::Animation);
        {
            let shot = Arc::make_mut(&mut project.shots[0]);
            shot.layers.push(Arc::new(Layer { parent: Some(hips), ..scarf }));
            Arc::make_mut(shot.pegs.iter_mut().find(|peg| peg.name == "legR-P").unwrap()).name = "leg_Right-P".to_string();
        }
        let options = ApplyOptions::default();
        let would = project.match_template(shot, &template, Some(Node::Peg(hips)), &options).unwrap();
        assert_eq!((would.applied(), would.skipped(), would.untouched.clone()), (5, 0, vec!["scarf".to_string()]));
        assert!(would.matches.iter().any(|found| found.item == "legR-P" && found.rig.as_deref() == Some("leg_Right-P")), "{:?}", would.matches);
        assert_eq!(peg(&project, "hips").animation.position.keys().len(), 0, "a look changes nothing");

        // Applied, the shot's own pegs have the pose and the keys; their pivots are their own.
        let before = project.shots[0].pegs.len();
        project.apply_template(shot, &template, Some(Node::Peg(hips)), &options).unwrap();
        assert_eq!((project.shots[0].pegs.len(), project.shots[0].layers.len()), (before, 4), "nothing is made");
        assert_eq!((peg(&project, "hips").animation.position.keys().len(), peg(&project, "hips").transform.pivot), (2, [0.0, 0.0]));
        assert_eq!((peg(&project, "legL-P").transform.rotation, peg(&project, "leg_Right-P").transform.rotation), (30.0, 0.0));
        // Mirrored, what was the left leg's is the right's, turned the other way, and the hips
        // move to the left. At another frame, the keys start there.
        let mirrored = ApplyOptions { mirror: true, frame: Some(10), ..ApplyOptions::default() };
        project.apply_template(shot, &template, Some(Node::Peg(hips)), &mirrored).unwrap();
        assert_eq!((peg(&project, "legL-P").transform.rotation, peg(&project, "leg_Right-P").transform.rotation), (0.0, -30.0));
        let keys = |project: &Project| peg(project, "hips").animation.position.keys().iter().map(|key| (key.frame, key.value)).collect::<Vec<_>>();
        assert_eq!(keys(&project), [(10, [0.0, 0.0]), (18, [-50.0, 0.0])]);
        // Merged, the keys it had stay under the template's.
        let merged = ApplyOptions { merge: true, ..ApplyOptions::default() };
        project.apply_template(shot, &template, Some(Node::Peg(hips)), &merged).unwrap();
        assert_eq!(peg(&project, "hips").animation.position.keys().iter().map(|key| key.frame).collect::<Vec<_>>(), [0, 8, 10, 18]);

        // A name the shot has not got is listed and left out, unless it is matched by hand.
        Arc::make_mut(Arc::make_mut(&mut project.shots[0]).pegs.iter_mut().find(|peg| peg.name == "legL-P").unwrap()).name = "shin".to_string();
        let would = project.match_template(shot, &template, Some(Node::Peg(hips)), &options).unwrap();
        let left = would.matches.iter().find(|found| found.item == "legL-P").unwrap();
        assert_eq!((left.found, would.skipped()), (Found::Missing, 1));
        let by_hand = ApplyOptions { map: [("legL-P".to_string(), "shin".to_string())].into(), ..ApplyOptions::default() };
        let would = project.match_template(shot, &template, Some(Node::Peg(hips)), &by_hand).unwrap();
        assert_eq!((would.matches.iter().find(|found| found.item == "legL-P").unwrap().found, would.skipped()), (Found::Mapped, 0));
    }

    #[test]
    fn each_drawing_drawn_otherwise_is_kept_or_replaced_as_it_is_asked() {
        let mut project = Project::default();
        let hips = character(&mut project);
        let shot = project.shots[0].id;
        let blank = || Arc::new(crate::Drawing::default());
        let other = || Arc::new(crate::Drawing { art_layers: Vec::new(), texts: Vec::new(), placements: Vec::new() });
        let pool = |project: &mut Project, names: &[&str], drawing: &dyn Fn() -> Arc<crate::Drawing>| {
            let ids: Vec<u64> = names.iter().map(|_| project.new_id()).collect();
            let shot = Arc::make_mut(&mut project.shots[0]);
            let leg = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.name == "legL").unwrap());
            leg.drawings = names.iter().zip(ids).map(|(name, id)| PoolDrawing { id: DrawingId(id), name: name.to_string(), drawing: drawing() }).collect();
        };
        // The template's leg has a foot and a toe drawn otherwise than the shot's, and a knee drawn alike.
        let mut posed = project.clone();
        pool(&mut posed, &["foot", "toe"], &other);
        let template = posed.template(shot, &[Part::Peg(hips)], "Feet").unwrap();
        pool(&mut project, &["foot", "toe", "knee"], &blank);
        let would = project.match_template(shot, &template, Some(Node::Peg(hips)), &ApplyOptions::default()).unwrap();
        let leg = would.matches.iter().find(|found| found.item.ends_with("legL")).unwrap();
        assert_eq!(leg.conflicts, ["foot", "toe"]);
        // The toe replaced alone; then all but the toe.
        let drawn = |project: &Project, name: &str| {
            let leg = project.shots[0].layers.iter().find(|layer| layer.name == "legL").unwrap();
            leg.drawings.iter().find(|entry| entry.name == name).unwrap().drawing.art_layers.is_empty()
        };
        let toe = [(leg.item.clone(), [("toe".to_string(), true)].into())].into();
        let mut first = project.clone();
        first.apply_template(shot, &template, Some(Node::Peg(hips)), &ApplyOptions { drawings: toe, ..ApplyOptions::default() }).unwrap();
        assert_eq!((drawn(&first, "foot"), drawn(&first, "toe")), (false, true));
        let not_toe = [(leg.item.clone(), [("toe".to_string(), false)].into())].into();
        let options = ApplyOptions { replace_drawings: true, drawings: not_toe, ..ApplyOptions::default() };
        project.apply_template(shot, &template, Some(Node::Peg(hips)), &options).unwrap();
        assert_eq!((drawn(&project, "foot"), drawn(&project, "toe")), (true, false));
    }

    /// A character in a group, named `name` with the copy's `number` after every name, its
    /// legs stacked by a composite of their own: the group and the composite.
    fn grouped(project: &mut Project, name: &str, number: &str) -> (GroupId, CompositeId) {
        let before: HashSet<u64> = project.shots[0].node_ids().collect();
        character(project);
        let (group, composite) = (GroupId(project.new_id()), CompositeId(project.new_id()));
        let shot = Arc::make_mut(&mut project.shots[0]);
        let new = |id: u64| !before.contains(&id);
        for peg in shot.pegs.iter_mut().filter(|peg| new(peg.id.0)) {
            Arc::make_mut(peg).name.push_str(number);
        }
        for layer in shot.layers.iter_mut().filter(|layer| new(layer.id.0)) {
            Arc::make_mut(layer).name.push_str(number);
        }
        let legs: Vec<Source> = shot.layers.iter().filter(|layer| new(layer.id.0)).map(|layer| Source::Layer(layer.id)).collect();
        shot.composites.push(Arc::new(crate::Composite::new(composite, &format!("Legs{number}"), legs)));
        let members = shot.node_ids().filter(|id| new(*id)).collect();
        shot.groups.push(Arc::new(crate::Group {
            id: group,
            name: format!("{name}{number}"),
            members,
            folded: false,
            rows_folded: false,
            board: false,
            template: None,
        }));
        (group, composite)
    }

    #[test]
    fn what_is_in_a_group_goes_onto_the_shots_group_of_its_name_and_composites_stack_alike() {
        let mut project = Project::default();
        let shot = project.shots[0].id;
        let (hero, legs) = grouped(&mut project, "Hero", "");
        let (twin, twin_legs) = grouped(&mut project, "Hero", " 2");
        let inputs = |project: &Project, id: CompositeId| project.shots[0].composite(id).unwrap().inputs.clone();
        let turned = |project: &Project, name: &str| project.shots[0].pegs.iter().find(|peg| peg.name == name).unwrap().transform.rotation;
        // A pose of the hero's, with the right leg stacked over the left.
        let mut posed = project.clone();
        {
            let shot = Arc::make_mut(&mut posed.shots[0]);
            Arc::make_mut(shot.pegs.iter_mut().find(|peg| peg.name == "legL-P").unwrap()).transform.rotation = 30.0;
            Arc::make_mut(shot.composites.iter_mut().find(|composite| composite.id == legs).unwrap()).inputs.reverse();
        }
        let template = posed.template(shot, &[Part::Group(hero)], "Hero").unwrap();
        let (before, twin_before) = (inputs(&project, legs), inputs(&project, twin_legs));

        // Onto the whole shot it goes onto the hero, and the twin is left as it is.
        let applied = project.apply_template(shot, &template, None, &ApplyOptions::default()).unwrap();
        let found = |applied: &Applied, kind: Kind| {
            applied.matches.iter().find(|found| found.kind == kind).map(|found| (found.found, found.rig.clone(), found.note.clone())).unwrap()
        };
        assert_eq!(found(&applied, Kind::Group), (Found::Matched, Some("Hero".to_string()), None));
        assert_eq!(found(&applied, Kind::Composite), (Found::Matched, Some("Legs".to_string()), Some("2 inputs restacked".to_string())));
        assert_eq!((turned(&project, "legL-P"), turned(&project, "legL-P 2")), (30.0, 0.0));
        assert_eq!((inputs(&project, legs), inputs(&project, twin_legs)), (before.iter().rev().copied().collect(), twin_before.clone()));

        // Matched by hand to the twin's group, it goes onto the twin, its copies' numbers not looked at.
        let onto_twin = ApplyOptions { map: [("Hero".to_string(), "Hero 2".to_string())].into(), ..ApplyOptions::default() };
        let applied = project.apply_template(shot, &template, None, &onto_twin).unwrap();
        assert_eq!(found(&applied, Kind::Group).0, Found::Mapped);
        assert_eq!(applied.skipped(), 0, "{:?}", applied.matches);
        assert_eq!(turned(&project, "legL-P 2"), 30.0);
        assert_eq!(inputs(&project, twin_legs), twin_before.iter().rev().copied().collect::<Vec<_>>());
        // What is made new goes into the group.
        let twin_id = |project: &Project| project.shots[0].group(twin).unwrap().members.len();
        let members = twin_id(&project);
        Arc::make_mut(&mut project.shots[0]).pegs.retain(|peg| peg.name != "legR-P 2");
        let ids: HashSet<u64> = project.shots[0].node_ids().collect();
        let made = ApplyOptions { make_missing: true, ..onto_twin };
        project.apply_template(shot, &template, None, &made).unwrap();
        let new = project.shots[0].pegs.iter().find(|peg| !ids.contains(&peg.id.0)).unwrap();
        // The peg, and the leg it carried, which hung from nothing once the peg was gone.
        assert_eq!((new.name.as_str(), twin_id(&project)), ("legR-P 2", members + 2));
        assert!(project.shots[0].group(twin).unwrap().members.contains(&new.id.0));
    }

    #[test]
    fn a_mirrored_pose_turns_and_moves_the_other_way() {
        let mut project = Project::default();
        let mut item = (*project.shots[0]).clone();
        let mut bend = Deformer::new(crate::DeformerId(1), "arm", [0.0; 2], vec![Bone::at_rest(90.0, 40.0)]);
        bend.offset = [3.0, 4.0];
        bend.bones[0].turns.set(0, 25.0);
        bend.bones[0].stretch = 1.5;
        let mut peg = Peg {
            id: PegId(project.new_id()),
            name: "hand".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Animation::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        };
        peg.transform = Transform { position: [5.0, 6.0], rotation: 10.0, skew: 4.0, tilt: [7.0, 8.0], scale: [2.0, 1.0], ..Transform::default() };
        peg.animation.position.put(Key { bend: Some(Bend { into: [-4.0, 1.0], out: [4.0, -1.0] }), ..Key::new(3, [1.0, 2.0]) });
        item.deformers.push(Arc::new(bend));
        item.pegs.push(Arc::new(peg));
        let flipped = mirrored(&item);
        let (bend, peg) = (&flipped.deformers[0], &flipped.pegs[0]);
        assert_eq!((bend.offset, bend.bones[0].turns.keys()[0].value, bend.bones[0].stretch), ([-3.0, 4.0], -25.0, 1.5));
        let transform = peg.transform;
        assert_eq!(
            (transform.position, transform.rotation, transform.skew, transform.tilt, transform.scale),
            ([-5.0, 6.0], -10.0, -4.0, [7.0, -8.0], [2.0, 1.0])
        );
        let key = &peg.animation.position.keys()[0];
        assert_eq!((key.value, key.bend), ([-1.0, 2.0], Some(Bend { into: [4.0, 1.0], out: [-4.0, -1.0] })));
    }

    #[test]
    fn what_the_shot_has_not_got_is_made_under_where_its_parent_went_when_asked() {
        let mut project = Project::default();
        let hips = character(&mut project);
        let shot = project.shots[0].id;
        let mut posed = project.clone();
        Arc::make_mut(Arc::make_mut(&mut posed.shots[0]).pegs.iter_mut().find(|peg| peg.name == "legR-P").unwrap()).transform.rotation = 20.0;
        let template = posed.template(shot, &[Part::Peg(hips)], "Stand").unwrap();
        // The shot has lost its right leg, peg and layer.
        {
            let shot = Arc::make_mut(&mut project.shots[0]);
            shot.pegs.retain(|peg| peg.name != "legR-P");
            shot.layers.retain(|layer| layer.name != "legR");
        }
        let (pegs, layers) = (project.shots[0].pegs.len(), project.shots[0].layers.len());
        let left_out = project.apply_template(shot, &template, Some(Node::Peg(hips)), &ApplyOptions::default()).unwrap();
        assert_eq!((left_out.skipped(), project.shots[0].pegs.len(), project.shots[0].layers.len()), (2, pegs, layers));

        let make = ApplyOptions { make_missing: true, ..ApplyOptions::default() };
        let would = project.match_template(shot, &template, Some(Node::Peg(hips)), &make).unwrap();
        assert_eq!((would.applied(), would.skipped(), project.shots[0].pegs.len()), (5, 0, pegs), "a look makes nothing");
        let applied = project.apply_template(shot, &template, Some(Node::Peg(hips)), &make).unwrap();
        let found = |item: &str| applied.matches.iter().find(|found| found.item.ends_with(item)).map(|found| (found.found, found.rig.clone())).unwrap();
        assert_eq!((found("legR-P"), found("legR")), ((Found::Made, Some("legR-P".to_string())), (Found::Made, Some("legR".to_string()))));
        // The peg is under the hips, with the template's pose; the layer under the peg made for its own.
        let shot = &project.shots[0];
        let made = shot.pegs.iter().find(|peg| peg.name == "legR-P").unwrap();
        let layer = shot.layers.iter().find(|layer| layer.name == "legR").unwrap();
        assert_eq!((made.parent, made.transform.rotation, layer.parent), (Some(hips), 20.0, Some(made.id)));
        assert!(made.id != hips && layer.template.is_none() && shot.pegs.len() == pegs + 1 && shot.layers.len() == layers + 1);
    }

    #[test]
    fn a_templates_driver_goes_onto_the_driver_of_the_peg_its_own_went_onto_or_is_made_there() {
        use crate::{Driver, DriverId, DriverInput, DriverParameter, Expressions};
        let mut project = Project::default();
        let hips = character(&mut project);
        let shot = project.shots[0].id;
        let id_of = |project: &Project, name: &str| project.shots[0].pegs.iter().find(|peg| peg.name == name).unwrap().id;
        // Saved from it: the left leg driven by the hips, with a keyed parameter.
        let mut driven = project.clone();
        let (leg, id) = (id_of(&driven, "legL-P"), DriverId(driven.new_id()));
        {
            let mut driver = Driver::new(id, "Swing", leg);
            driver.expressions = Expressions::parse("rotation = hips.x * k").unwrap();
            driver.inputs = vec![DriverInput { name: "hips".into(), peg: Some(hips) }];
            let mut track = Track::default();
            track.set(2, 0.5);
            driver.parameters = vec![DriverParameter { name: "k".into(), value: 1.0, track }];
            Arc::make_mut(&mut driven.shots[0]).drivers.push(Arc::new(driver));
        }
        let template = driven.template(shot, &[Part::Peg(hips)], "Swing").unwrap();
        assert_eq!(template.shots[0].drivers.len(), 1);

        // Applied onto the shot, which has no driver: one is made on its left leg, reading its hips.
        let options = ApplyOptions { frame: Some(10), ..ApplyOptions::default() };
        let applied = project.apply_template(shot, &template, Some(Node::Peg(hips)), &options).unwrap();
        let made = project.shots[0].driver_of(id_of(&project, "legL-P")).cloned().expect("a driver was made");
        assert_eq!((made.name.as_str(), made.inputs[0].peg, made.expressions.text()), ("Swing", Some(hips), "rotation = hips.x * k"));
        assert_eq!(made.parameters[0].track.keys()[0].frame, 12, "its keys go where the template's frames go");
        let note = |applied: &Applied| applied.matches.iter().find(|found| found.rig.as_deref() == Some("legL-P")).and_then(|found| found.note.clone());
        assert_eq!(note(&applied).as_deref(), Some("its driver made"));

        // Applied again, it goes onto that driver, not beside it; what the shot's had besides stays.
        {
            let shot = Arc::make_mut(&mut project.shots[0]);
            let own = Arc::make_mut(shot.drivers.iter_mut().find(|driver| driver.id == made.id).unwrap());
            own.parameters.push(DriverParameter { name: "spare".into(), value: 3.0, track: Track::default() });
        }
        let applied = project.apply_template(shot, &template, Some(Node::Peg(hips)), &ApplyOptions::default()).unwrap();
        let drivers = &project.shots[0].drivers;
        assert_eq!(drivers.len(), 1);
        assert_eq!(drivers[0].parameters.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["k", "spare"]);
        assert_eq!(note(&applied).as_deref(), Some("its driver applied"));
    }
}
