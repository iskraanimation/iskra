//! Pegs (plan §4.3): a peg is a transform, and what is parented to it moves, turns and scales
//! with it. Pegs can be parented to pegs, so a shot's pegs and layers form a tree; the
//! timeline lists it one row a node.
//!
//! A peg's transform is flat but for two things. Its depth moves what is under it towards the
//! viewer or away, which decides what is drawn over what (plan §4.4). Its tilt, which most
//! pegs don't have, leans what is under it out of the picture plane (see [`crate::Space`]).
//!
//! A peg's position, rotation, scale, skew, depth and tilt can each have keyframes. A value with keys is
//! animated: it is what its keys make it on each frame. A value without is the same on every
//! frame.

use std::collections::{BTreeSet, HashMap};

use iskra_drawing::Affine;
use serde::{Deserialize, Serialize};

use crate::{Animatable, DeformerId, Interp, LayerId, PegId, Shot, Track};

/// Where a peg puts what is under it. With every value at its default nothing moves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transform {
    /// How far it is moved, in the units of the space the peg is in (scene units for a peg
    /// with no parent).
    pub position: [f64; 2],
    /// In degrees, clockwise on screen, about the pivot.
    pub rotation: f64,
    /// 1 is the drawn size. Scaling is about the pivot.
    pub scale: [f64; 2],
    /// In degrees: how far verticals lean to the left. Applied after the scale, before the
    /// rotation.
    pub skew: f64,
    /// The point that rotation, scale and skew leave where it is.
    pub pivot: [f64; 2],
    /// How far it is moved towards the viewer, in the units `position` has: what is nearer is
    /// drawn over what is farther. Below zero is away from the viewer.
    #[serde(default)]
    pub depth: f64,
    /// In degrees, about the pivot: how far the top leans away from the viewer, and how far
    /// the right side swings away. Not zero, the camera sees what the peg carries in
    /// perspective.
    #[serde(default)]
    pub tilt: [f64; 2],
}

impl Default for Transform {
    fn default() -> Self {
        Self { position: [0.0, 0.0], rotation: 0.0, scale: [1.0, 1.0], skew: 0.0, pivot: [0.0, 0.0], depth: 0.0, tilt: [0.0, 0.0] }
    }
}

impl Transform {
    /// The transform as a matrix, from the peg's own space to the space the peg is in.
    pub fn matrix(&self) -> Affine {
        let [sx, sy] = self.scale;
        let [px, py] = self.pivot;
        let scale = Affine { m: [sx, 0.0, 0.0, sy, 0.0, 0.0] };
        let skew = Affine { m: [1.0, 0.0, self.skew.to_radians().tan(), 1.0, 0.0, 0.0] };
        let place = Affine::translate(self.position[0] + px, self.position[1] + py);
        place.mul(&Affine::rotate(self.rotation.to_radians())).mul(&skew).mul(&scale).mul(&Affine::translate(-px, -py))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Peg {
    pub id: PegId,
    pub name: String,
    /// The peg this one is parented to, in the same shot.
    pub parent: Option<PegId>,
    /// The transform on every frame, for the values that have no keys.
    pub transform: Transform,
    #[serde(default)]
    pub animation: Animation,
    /// The library template the peg is a copy from, if it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// The point of a chain of bones or a curve the peg follows, if it follows one: it is
    /// carried as the point is posed, under its parent and before its own transform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follows: Option<Follow>,
    /// What the peg takes from its parent and the bones it follows.
    #[serde(default, skip_serializing_if = "Inherit::is_all")]
    pub inherit: Inherit,
    /// Of a rig whose top this peg is: the names of library templates' nodes matched by hand to
    /// the names of the rig's, kept from one template applied onto it to the next (plan §4.12).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub matches: std::collections::BTreeMap<String, String>,
}

/// A point of a deformer that a peg follows (plan §4.5), as a kinematic output: the
/// peg goes with the point as the deformer is posed. What it carries is not bent, unless it
/// `bends`: then the drawings right under it are bent by the deformer as its own drawings
/// are, and the pegs under it are carried by the point as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Follow {
    pub deformer: DeformerId,
    /// The joint of a chain, or the point of a curve: 0 is where it starts. `None` is its
    /// end, whichever that is as joints are put in and taken out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<usize>,
    /// The drawings right under the peg are bent by the deformer, not carried (the user's of
    /// 2026-10-26). Files of format 48 and before have none: carried, as they were.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bends: bool,
}

/// Which of its parent a peg takes: where the parent's own transform and the bones the peg
/// follows put it, how they turn it, and how they scale it. All three, as a peg has unless
/// told, is being carried whole. Without the position, the peg's pivot stays where its own
/// values put it under the parent's parent; without the rotation or the scale, it is not
/// turned, or not scaled, by the parent, though it still goes where that puts it. The pegs
/// above the parent always count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Inherit {
    pub position: bool,
    pub rotation: bool,
    pub scale: bool,
}

impl Default for Inherit {
    fn default() -> Self {
        Inherit { position: true, rotation: true, scale: true }
    }
}

impl Inherit {
    pub fn is_all(&self) -> bool {
        *self == Inherit::default()
    }

    /// `above`, the transform above a peg, with what is not taken left out: about `anchor`,
    /// the peg's pivot where its values put it, in the space it is above.
    pub fn apply(&self, above: Affine, anchor: [f64; 2]) -> Affine {
        if self.is_all() {
            return above;
        }
        let [a, b, c, d, e, f] = above.m;
        let at = if self.position { [a * anchor[0] + c * anchor[1] + e, b * anchor[0] + d * anchor[1] + f] } else { anchor };
        let linear = if self.rotation && self.scale {
            [a, b, c, d]
        } else {
            let wide = a.hypot(b);
            let turn = if self.rotation { b.atan2(a) } else { 0.0 };
            let (across, down) = if self.scale && wide > 1e-12 { (wide, (a * d - b * c) / wide) } else { (1.0, 1.0) };
            let (sin, cos) = turn.sin_cos();
            [cos * across, sin * across, -sin * down, cos * down]
        };
        let [a, b, c, d] = linear;
        // About the anchor: it goes to `at`, and the rest turns and scales about it.
        Affine { m: [a, b, c, d, at[0] - (a * anchor[0] + c * anchor[1]), at[1] - (b * anchor[0] + d * anchor[1])] }
    }
}

/// The keyframes of a peg's transform, a track for each value that can be animated. The pivot
/// can't.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Animation {
    /// The position moves along a smooth path through its keys.
    pub position: Track<[f64; 2]>,
    pub rotation: Track<f64>,
    pub scale: Track<[f64; 2]>,
    pub skew: Track<f64>,
    pub depth: Track<f64>,
    pub tilt: Track<[f64; 2]>,
}

/// One of the values of a transform that can be animated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Channel {
    Position,
    Rotation,
    Scale,
    Skew,
    Depth,
    Tilt,
}

impl Channel {
    pub const ALL: [Channel; 6] = [Channel::Position, Channel::Rotation, Channel::Scale, Channel::Skew, Channel::Depth, Channel::Tilt];
}

/// Do the same to each of an animation's tracks, whatever its values are: to all of them, or
/// to the one of a channel.
macro_rules! each_track {
    ($animation:expr, $channel:expr, |$track:ident| $body:expr) => {{
        let (animation, channel): (_, Option<Channel>) = ($animation, $channel);
        if channel.is_none_or(|channel| channel == Channel::Position) {
            let $track = &mut animation.position;
            $body;
        }
        if channel.is_none_or(|channel| channel == Channel::Rotation) {
            let $track = &mut animation.rotation;
            $body;
        }
        if channel.is_none_or(|channel| channel == Channel::Scale) {
            let $track = &mut animation.scale;
            $body;
        }
        if channel.is_none_or(|channel| channel == Channel::Skew) {
            let $track = &mut animation.skew;
            $body;
        }
        if channel.is_none_or(|channel| channel == Channel::Depth) {
            let $track = &mut animation.depth;
            $body;
        }
        if channel.is_none_or(|channel| channel == Channel::Tilt) {
            let $track = &mut animation.tilt;
            $body;
        }
    }};
}

impl Animation {
    /// The animation with every key `by` frames later.
    pub fn shifted(&self, by: u32) -> Animation {
        Animation {
            position: self.position.shifted(by),
            rotation: self.rotation.shifted(by),
            scale: self.scale.shifted(by),
            skew: self.skew.shifted(by),
            depth: self.depth.shifted(by),
            tilt: self.tilt.shifted(by),
        }
    }

    /// The frames that have a key: of any value, or of the one of `channel`.
    pub fn frames(&self, channel: Option<Channel>) -> BTreeSet<u32> {
        let mut frames = BTreeSet::new();
        let mut animation = self.clone();
        each_track!(&mut animation, channel, |track| frames.extend(track.keys().iter().map(|key| key.frame)));
        frames
    }

    /// The frames that have a key of any value.
    pub fn key_frames(&self) -> BTreeSet<u32> {
        self.frames(None)
    }

    /// Whether any value moves on after `frame`, towards a later key of its own.
    pub fn moves_after(&self, frame: u32) -> bool {
        let (place, turn, size) = (self.position.moves_after(frame), self.rotation.moves_after(frame), self.scale.moves_after(frame));
        place || turn || size || self.skew.moves_after(frame) || self.depth.moves_after(frame) || self.tilt.moves_after(frame)
    }

    /// How the keys on `frame` ease, in and out: those of the first value that has a key there.
    pub fn interp(&self, frame: u32) -> Option<(Interp, Interp)> {
        self.interp_of(frame, None)
    }

    /// [`Animation::moves_after`], of any value or of the one of `channel`.
    pub fn moves_after_of(&self, frame: u32, channel: Option<Channel>) -> bool {
        let mut moves = false;
        let mut animation = self.clone();
        each_track!(&mut animation, channel, |track| moves |= track.moves_after(frame));
        moves
    }

    /// [`Animation::interp`], of one channel's key or of the first that has one.
    pub fn interp_of(&self, frame: u32, channel: Option<Channel>) -> Option<(Interp, Interp)> {
        let mut found = None;
        let mut animation = self.clone();
        each_track!(&mut animation, channel, |track| found = found.or(track.key(frame).map(|key| (key.into, key.out))));
        found
    }

    /// Set how the keys on `frame` ease, of every value or of one; a side that is `None` stays
    /// as it is.
    pub fn set_interp(&mut self, frame: u32, channel: Option<Channel>, into: Option<Interp>, out: Option<Interp>) {
        each_track!(self, channel, |track| if let Some(key) = track.key_mut(frame) {
            (key.into, key.out) = (into.unwrap_or(key.into), out.unwrap_or(key.out));
        });
    }

    /// Move the keys on `from` to `to`, of every value or of one.
    pub fn move_keys(&mut self, from: u32, to: u32, channel: Option<Channel>) {
        each_track!(self, channel, |track| track.move_key(from, to));
    }
}

impl Peg {
    /// The peg's transform on `frame`.
    pub fn transform_at(&self, frame: u32) -> Transform {
        let (still, animation) = (&self.transform, &self.animation);
        Transform {
            position: animation.position.at(frame, true).unwrap_or(still.position),
            rotation: animation.rotation.at(frame, false).unwrap_or(still.rotation),
            scale: animation.scale.at(frame, false).unwrap_or(still.scale),
            skew: animation.skew.at(frame, false).unwrap_or(still.skew),
            pivot: still.pivot,
            depth: animation.depth.at(frame, false).unwrap_or(still.depth),
            tilt: animation.tilt.at(frame, false).unwrap_or(still.tilt),
        }
    }

    /// Key every value of the transform on `frame`, as it is there.
    pub fn key_all(&mut self, frame: u32) {
        self.key_channel(frame, None);
    }

    /// Key the peg on `frame` as it is there: every value, as [`Peg::key_all`], or the one of
    /// `channel` alone.
    pub fn key_channel(&mut self, frame: u32, channel: Option<Channel>) {
        let now = self.transform_at(frame);
        if let Some(channel) = channel {
            let animation = &mut self.animation;
            match channel {
                Channel::Position => animation.position.set(frame, now.position),
                Channel::Rotation => animation.rotation.set(frame, now.rotation),
                Channel::Scale => animation.scale.set(frame, now.scale),
                Channel::Skew => animation.skew.set(frame, now.skew),
                Channel::Depth => animation.depth.set(frame, now.depth),
                Channel::Tilt => animation.tilt.set(frame, now.tilt),
            }
            return;
        }
        self.animation.position.set(frame, now.position);
        self.animation.rotation.set(frame, now.rotation);
        self.animation.scale.set(frame, now.scale);
        self.animation.skew.set(frame, now.skew);
        self.animation.depth.set(frame, now.depth);
        // Most pegs have no tilt, and get no keys of it.
        if now.tilts() || !self.animation.tilt.is_empty() {
            self.animation.tilt.set(frame, now.tilt);
        }
    }

    /// Take the keys on `frame` away, of every value or of one. A value that loses its last
    /// key stays as that key had it.
    pub fn remove_keys(&mut self, frame: u32, channel: Option<Channel>) {
        /// Remove a track's key; if it was the last, `still` takes its value.
        fn remove<V: Animatable>(track: &mut Track<V>, frame: u32, still: &mut V) {
            if let (Some(key), true) = (track.remove(frame), track.is_empty()) {
                *still = key.value;
            }
        }
        let (still, animation) = (&mut self.transform, &mut self.animation);
        let wanted = |one: Channel| channel.is_none_or(|channel| channel == one);
        if wanted(Channel::Position) {
            remove(&mut animation.position, frame, &mut still.position);
        }
        if wanted(Channel::Rotation) {
            remove(&mut animation.rotation, frame, &mut still.rotation);
        }
        if wanted(Channel::Scale) {
            remove(&mut animation.scale, frame, &mut still.scale);
        }
        if wanted(Channel::Skew) {
            remove(&mut animation.skew, frame, &mut still.skew);
        }
        if wanted(Channel::Depth) {
            remove(&mut animation.depth, frame, &mut still.depth);
        }
        if wanted(Channel::Tilt) {
            remove(&mut animation.tilt, frame, &mut still.tilt);
        }
    }
}

// As for `Layer`: the transform's numbers are always finite.
impl Eq for Peg {}

/// A node of a shot's tree, which is a row of its timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Peg(PegId),
    Layer(LayerId),
}

impl Shot {
    pub fn peg(&self, id: PegId) -> Option<&Peg> {
        self.pegs.iter().find(|p| p.id == id).map(|p| &**p)
    }

    /// The peg a node is parented to.
    pub fn parent_of(&self, node: Node) -> Option<PegId> {
        match node {
            Node::Peg(id) => self.peg(id)?.parent,
            Node::Layer(id) => self.layer(id)?.parent,
        }
    }

    /// `parent` and the pegs above it, nearest first.
    /// The peg that `parent` names, or where it names a port or a named port (plan §8.6) the
    /// first peg above that: a port has no transform of its own, and hands down the one that
    /// comes into it.
    pub fn peg_above(&self, parent: Option<PegId>) -> Option<&Peg> {
        let mut at = parent?;
        // No more ports lie between two pegs than the shot has effects.
        for _ in 0..=self.effects.len() {
            match self.peg(at) {
                Some(peg) => return Some(peg),
                None => at = self.handed_down(at)?,
            }
        }
        None
    }

    /// Whether a layer is hidden (see [`Shot::hidden`]): its own row is, or the row of a peg
    /// that carries it, or of a group that it or one of those pegs is in.
    pub fn hides(&self, layer: LayerId) -> bool {
        self.marked_in(&self.hidden, layer)
    }

    /// Whether a layer is locked (see [`Shot::locked`]): its own row is, or the row of a peg
    /// that carries it, or of a group that it or one of those pegs is in.
    pub fn locks(&self, layer: LayerId) -> bool {
        self.marked_in(&self.locked, layer)
    }

    /// Whether a layer's row is in `rows`, or the row of a peg that carries it, or of a group
    /// that it or one of those pegs is in.
    fn marked_in(&self, rows: &BTreeSet<u64>, layer: LayerId) -> bool {
        if rows.is_empty() {
            return false;
        }
        let marked = |node: u64| rows.contains(&node) || self.groups_above(node).any(|group| rows.contains(&group.id.0));
        marked(layer.0) || self.ancestors(self.layer(layer).and_then(|layer| layer.parent)).any(|peg| marked(peg.id.0))
    }

    pub fn ancestors(&self, parent: Option<PegId>) -> impl Iterator<Item = &Peg> {
        // A tree is never deeper than it has pegs; the limit only keeps bad data from looping.
        std::iter::successors(self.peg_above(parent), |peg| self.peg_above(peg.parent)).take(self.pegs.len())
    }

    /// The transform that the pegs from `parent` up give, on `frame`, what is parented to
    /// `parent`: from that node's own space to the scene.
    pub fn world(&self, parent: Option<PegId>, frame: u32) -> Affine {
        self.world_carried(parent, frame, true)
    }

    /// The same, where `carried` is false without what carries `parent` itself along the bones
    /// it follows: the world of the drawings right under a peg that bends them, which the
    /// bones move as they bend them.
    fn world_carried(&self, parent: Option<PegId>, frame: u32, carried: bool) -> Affine {
        let pegs: Vec<&Peg> = self.ancestors(parent).collect();
        let first = pegs.first().map(|peg| peg.id);
        // Down from the top: each peg's world is what is above it, then its own transform.
        // What is above a peg is the world above its parent, then the parent's own
        // transform, which is what the peg may leave out.
        let mut parent = (Affine::IDENTITY, Affine::IDENTITY);
        for peg in pegs.into_iter().rev() {
            let above = if carried || Some(peg.id) != first { self.inherited(peg, parent, frame) } else { self.inherited_still(peg, parent, frame) };
            parent = (above, peg.transform_at(frame).matrix());
        }
        parent.0.mul(&parent.1)
    }

    /// The world of a layer's drawings on `frame`: its pegs', and under a peg that bends what
    /// it carries without that peg's carry along the bones, which bend it instead.
    pub fn layer_world(&self, layer: &crate::Layer, frame: u32) -> Affine {
        self.world_carried(layer.parent, frame, self.bending(layer).is_none())
    }

    /// The deformer that bends `layer` through the peg right above it, a chain above it that
    /// the peg follows and bends what it carries by.
    pub fn bending(&self, layer: &crate::Layer) -> Option<DeformerId> {
        let follow = self.peg_above(layer.parent)?.follows.filter(|follow| follow.bends)?;
        self.deformer(follow.deformer).map(|_| follow.deformer).filter(|bones| layer.deformer != Some(*bones))
    }

    /// What bends `layer`'s drawings on `frame`, in their pixels: its own bones, then a chain
    /// above it that bends it through its peg ([`Shot::bending`]), taken into its space at
    /// rest. `None` where nothing does.
    pub fn warp_of(&self, layer: &crate::Layer, frame: u32) -> Option<crate::Warp> {
        let own = layer.deformer.and_then(|id| self.chain(id, frame)).map(|deformer| deformer.warp(frame, layer.density));
        let chain = self.bending(layer).and_then(|id| {
            let deformer = self.chain(id, frame)?;
            // The chain lies in the space of the first layer it bends of its own.
            let home = self.layers.iter().find(|other| other.deformer == Some(id));
            let (space, density) = home.map_or((layer.parent, layer.density), |home| (home.parent, home.density));
            let at_rest = self.unfollowed(space, frame).inverse()?.mul(&self.unfollowed(layer.parent, frame));
            let to = Affine::scale(density).mul(&at_rest).mul(&Affine::scale(1.0 / layer.density));
            Some(deformer.warp(frame, density).through(to))
        });
        match (own, chain) {
            (Some(own), Some(chain)) => Some(chain.after(own)),
            (own, chain) => own.or(chain),
        }
    }

    /// The transform above a peg, which its own values are in, on `frame`: its parent's
    /// world, carried by the bones it follows, less what it does not inherit.
    pub fn above(&self, peg: &Peg, frame: u32) -> Affine {
        let parent =
            self.peg_above(peg.parent).map_or((Affine::IDENTITY, Affine::IDENTITY), |parent| (self.above(parent, frame), parent.transform_at(frame).matrix()));
        self.inherited(peg, parent, frame)
    }

    /// What is above a peg, given what is above its parent and the parent's own transform:
    /// the two, then the bones the peg follows. What it does not inherit is left out of the
    /// parent's own transform and the bones, those it follows and those its parent follows
    /// (a fist under the lower arm's peg, which rides the arm's bones), not of what is above.
    pub(crate) fn inherited(&self, peg: &Peg, (over_parent, parent): (Affine, Affine), frame: u32) -> Affine {
        let own = parent.mul(&self.followed(peg, frame));
        if peg.inherit.is_all() {
            return over_parent.mul(&own);
        }
        // The parent's carry along its bones ends what is above it: taken from there to here.
        let carry = self.peg_above(peg.parent).filter(|parent| parent.follows.is_some()).map(|parent| self.followed(parent, frame));
        let (over_parent, own) = match carry.and_then(|carry| Some((carry.inverse()?, carry))) {
            Some((back, carry)) => (over_parent.mul(&back), carry.mul(&own)),
            None => (over_parent, own),
        };
        over_parent.mul(&peg.inherit.apply(own, self.anchor(peg, frame)))
    }

    /// What is above a peg as [`Shot::inherited`] has it, without the bones it follows.
    fn inherited_still(&self, peg: &Peg, (over_parent, parent): (Affine, Affine), frame: u32) -> Affine {
        if peg.inherit.is_all() {
            return over_parent.mul(&parent);
        }
        let pose = peg.transform_at(frame);
        over_parent.mul(&peg.inherit.apply(parent, [pose.position[0] + pose.pivot[0], pose.position[1] + pose.pivot[1]]))
    }

    /// The point of what is above a peg that it is carried by, in the space it is above:
    /// the point it follows, where that is at rest, or its own pivot where its values put it.
    fn anchor(&self, peg: &Peg, frame: u32) -> [f64; 2] {
        let followed = peg.follows.and_then(|follow| {
            let rest = self.deformer(follow.deformer)?.joints(None);
            let at = *rest.get(follow.point.unwrap_or(rest.len().checked_sub(1)?))?;
            // From the bones' space to the parent's, where those are not the same.
            let space = self.layers.iter().find(|layer| layer.deformer == Some(follow.deformer)).map_or(peg.parent, |layer| layer.parent);
            let at = if space == peg.parent { at } else { self.unfollowed(peg.parent, frame).inverse()?.apply(self.unfollowed(space, frame).apply(at)) };
            Some([at.x, at.y])
        });
        followed.unwrap_or_else(|| {
            let pose = peg.transform_at(frame);
            [pose.position[0] + pose.pivot[0], pose.position[1] + pose.pivot[1]]
        })
    }

    /// The same, leaving out what pegs that follow a deformer are carried by: how a
    /// deformer's space is found, which no follow can then go round in a circle through.
    fn unfollowed(&self, parent: Option<PegId>, frame: u32) -> Affine {
        self.ancestors(parent).fold(Affine::IDENTITY, |below, peg| peg.transform_at(frame).matrix().mul(&below))
    }

    /// How the point a peg follows is carried on `frame`, from its rest to its pose, in the
    /// space of what is parented to the peg's parent: what comes between the parent's
    /// transform and the peg's own. Nothing for a peg that follows nothing, or a point the
    /// deformer has not got.
    ///
    /// A deformer lies in the space of the layers it bends, the first of them; a peg under
    /// another parent has the point's move brought into its parent's space.
    pub fn followed(&self, peg: &Peg, frame: u32) -> Affine {
        let Some(follow) = peg.follows else { return Affine::IDENTITY };
        let Some(carry) = self.chain(follow.deformer, frame).and_then(|deformer| deformer.carry(follow.point, frame)) else {
            return Affine::IDENTITY;
        };
        let space = self.layers.iter().find(|layer| layer.deformer == Some(follow.deformer)).map_or(peg.parent, |layer| layer.parent);
        if space == peg.parent {
            return carry;
        }
        let (theirs, ours) = (self.unfollowed(space, frame), self.unfollowed(peg.parent, frame));
        match (theirs.inverse(), ours.inverse()) {
            (Some(from_theirs), Some(from_ours)) => from_ours.mul(&theirs).mul(&carry).mul(&from_theirs).mul(&ours),
            _ => Affine::IDENTITY,
        }
    }

    /// How near the viewer the pegs from `parent` up put, on `frame`, what is parented to
    /// `parent`: their depths, added up.
    pub fn depth(&self, parent: Option<PegId>, frame: u32) -> f64 {
        self.ancestors(parent).map(|peg| peg.transform_at(frame).depth).sum()
    }

    /// The timeline's rows: every peg and layer with how deep it is nested, each peg followed
    /// by what is parented to it. Siblings are in stacking order, top first, with the layers
    /// that are not seen after those that are; a peg stands where the topmost layer under it
    /// would, and a peg with no layer under it comes last.
    pub fn rows(&self) -> Vec<(Node, usize)> {
        // Under the peg named, or where a port is named, the peg above the port.
        let known = |parent: Option<PegId>| self.peg_above(parent).map(|peg| peg.id);
        // Each layer's place from the top: as the display stacks them, then the rest as listed.
        let order = self.listed();
        let mut top: HashMap<PegId, usize> = HashMap::new();
        for (index, layer) in order.iter().enumerate() {
            for peg in self.ancestors(layer.parent) {
                top.entry(peg.id).or_insert(index);
            }
        }
        let mut children: HashMap<Option<PegId>, Vec<((usize, usize), Node)>> = HashMap::new();
        for (index, peg) in self.pegs.iter().enumerate() {
            let place = (top.get(&peg.id).copied().unwrap_or(usize::MAX), index);
            children.entry(known(peg.parent)).or_default().push((place, Node::Peg(peg.id)));
        }
        for (index, layer) in order.iter().enumerate() {
            children.entry(known(layer.parent)).or_default().push(((index, 0), Node::Layer(layer.id)));
        }
        for siblings in children.values_mut() {
            siblings.sort_by_key(|(place, _)| *place);
        }
        let mut rows = Vec::new();
        let mut open = vec![(None, 0, 0)];
        // Depth first: `open` holds, for each peg being listed, how far its children have got.
        while let Some((parent, depth, at)) = open.pop() {
            let Some((_, node)) = children.get(&parent).and_then(|siblings| siblings.get(at)) else { continue };
            rows.push((*node, depth));
            open.push((parent, depth, at + 1));
            if let Node::Peg(id) = node {
                open.push((Some(*id), depth + 1, 0));
            }
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iskra_drawing::Point;

    use super::*;
    use crate::{Project, Role};

    fn close(a: Point, b: [f64; 2]) -> bool {
        (a.x - b[0]).hypot(a.y - b[1]) < 1e-9
    }

    #[test]
    fn a_transform_turns_and_scales_about_its_pivot() {
        assert_eq!(Transform::default().matrix(), Affine::IDENTITY);
        let turn = Transform { rotation: 90.0, pivot: [10.0, 0.0], ..Default::default() };
        assert!(close(turn.matrix().apply(Point::new(10.0, 0.0)), [10.0, 0.0]), "the pivot stays");
        assert!(close(turn.matrix().apply(Point::new(20.0, 0.0)), [10.0, 10.0]), "clockwise on screen, where y is down");
        let all = Transform { position: [5.0, -3.0], scale: [2.0, 0.5], skew: 45.0, ..turn };
        assert!(close(all.matrix().apply(Point::new(10.0, 0.0)), [15.0, -3.0]), "the pivot only moves with the position");
        // (0, 4) from the pivot: scaled to (0, 2), leant to (2, 2), turned to (-2, 2).
        assert!(close(all.matrix().apply(Point::new(10.0, 4.0)), [13.0, -1.0]));
    }

    /// A shot with layers `names`, top first, and pegs `pegs` as (name, parent).
    fn shot(names: &[&str], pegs: &[(&str, Option<usize>)]) -> (Project, Vec<PegId>) {
        let mut project = Project::default();
        let layers = names.iter().map(|name| Arc::new(project.new_layer(name, Role::Animation))).collect();
        let ids: Vec<PegId> = pegs.iter().map(|_| PegId(project.new_id())).collect();
        let pegs = pegs.iter().zip(&ids).map(|((name, parent), id)| {
            Arc::new(Peg {
                id: *id,
                name: name.to_string(),
                parent: parent.map(|p| ids[p]),
                transform: Transform::default(),
                animation: Animation::default(),
                template: None,
                follows: None,
                inherit: Default::default(),
                matches: Default::default(),
            })
        });
        let shot = Arc::make_mut(&mut project.shots[0]);
        (shot.layers, shot.pegs) = (layers, pegs.collect());
        (project, ids)
    }

    fn listed(shot: &Shot) -> String {
        let name = |node: Node| match node {
            Node::Peg(id) => shot.peg(id).unwrap().name.clone(),
            Node::Layer(id) => shot.layer(id).unwrap().name.clone(),
        };
        shot.rows().into_iter().map(|(node, depth)| format!("{}{}", ">".repeat(depth), name(node))).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn rows_nest_under_their_pegs_in_stacking_order() {
        let (mut project, pegs) = shot(&["hat", "head", "body", "shadow"], &[("spare", None), ("character", None), ("top", Some(1))]);
        assert_eq!(listed(&project.shots[0]), "hat head body shadow spare character >top", "pegs that carry nothing come last");
        let shot = Arc::make_mut(&mut project.shots[0]);
        for (layer, peg) in [(0, 2), (1, 2), (2, 1)] {
            Arc::make_mut(&mut shot.layers[layer]).parent = Some(pegs[peg]);
        }
        assert_eq!(listed(shot), "character >top >>hat >>head >body shadow spare");
        // The peg stands where its topmost layer would: here under the shadow.
        shot.layers.rotate_right(1);
        assert_eq!(listed(shot), "shadow character >top >>hat >>head >body spare");
    }

    #[test]
    fn what_is_under_pegs_gets_all_their_transforms() {
        let (mut project, pegs) = shot(&["hand"], &[("arm", None), ("wrist", Some(0))]);
        let shot = Arc::make_mut(&mut project.shots[0]);
        Arc::make_mut(&mut shot.pegs[0]).transform = Transform { position: [100.0, 0.0], rotation: 90.0, ..Default::default() };
        Arc::make_mut(&mut shot.pegs[1]).transform.position = [10.0, 0.0];
        Arc::make_mut(&mut shot.layers[0]).parent = Some(pegs[1]);
        assert_eq!(shot.world(None, 0), Affine::IDENTITY);
        // The wrist moves the hand 10 along the arm, which the arm has turned to point down.
        assert!(close(shot.world(shot.layers[0].parent, 0).apply(Point::new(0.0, 0.0)), [100.0, 10.0]));
        // Keyed, the arm is where its keys put it on each frame; the turn has no key and stays.
        let arm = Arc::make_mut(&mut shot.pegs[0]);
        arm.animation.position.set(0, [100.0, 0.0]);
        arm.animation.position.set(10, [200.0, 0.0]);
        assert!(close(shot.world(shot.layers[0].parent, 5).apply(Point::new(0.0, 0.0)), [150.0, 10.0]));
        assert_eq!(shot.pegs[0].animation.key_frames().into_iter().collect::<Vec<_>>(), [0, 10]);
        assert_eq!(shot.ancestors(Some(pegs[1])).map(|p| p.name.as_str()).collect::<Vec<_>>(), ["wrist", "arm"]);
        assert_eq!(shot.parent_of(Node::Peg(pegs[1])), Some(pegs[0]));
        // Keying everything on a frame keeps the pose; taking the last key away keeps it too.
        let arm = Arc::make_mut(&mut shot.pegs[0]);
        arm.key_all(5);
        assert_eq!((arm.animation.rotation.at(5, false), arm.animation.position.at(5, true)), (Some(90.0), Some([150.0, 0.0])));
        arm.animation.set_interp(5, None, None, Some(Interp::Hold));
        assert_eq!(arm.animation.interp(5), Some((Interp::Linear, Interp::Hold)));
        arm.animation.move_keys(5, 7, None);
        arm.animation.move_keys(7, 8, Some(Channel::Rotation));
        assert_eq!((arm.animation.frames(Some(Channel::Rotation)), arm.animation.frames(Some(Channel::Skew))), (BTreeSet::from([8]), BTreeSet::from([7])));
        arm.animation.move_keys(8, 7, Some(Channel::Rotation));
        assert_eq!(arm.animation.key_frames().into_iter().collect::<Vec<_>>(), [0, 7, 10]);
        for frame in [0, 10, 7] {
            arm.remove_keys(frame, None);
        }
        assert!(arm.animation == Animation::default() && arm.transform.position == [150.0, 0.0] && arm.transform.rotation == 90.0);
        arm.transform.position = [100.0, 0.0];
        // Bad data: pegs parented in a ring are walked once, not for ever.
        Arc::make_mut(&mut shot.pegs[0]).parent = Some(pegs[1]);
        assert_eq!(shot.ancestors(Some(pegs[1])).count(), 2);
    }
}
