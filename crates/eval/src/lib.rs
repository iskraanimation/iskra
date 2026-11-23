//! Evaluation of a shot's node graph (plan §4.3): what is seen on a frame, and where.
//!
//! This is the first of the plan's two passes: the transforms. Going down from the display,
//! every layer that reaches it is listed in the order it is stacked, with the drawing it
//! exposes on the frame and the transform the pegs above it give it there. The second pass,
//! which makes pixels of that, is the compositor's.
//!
//! It is headless: the viewport draws what it gives, and so will the renderer and the export.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iskra_model::{Affine, ArtLayerKind, DeformerId, DrawingId, EffectId, LayerId, PICTURE, PegId, Projection, Shot, Source, Stacking, VISIBLE};

/// One layer as it is seen on a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub layer: LayerId,
    /// The drawing the layer exposes on the frame, if its cell there isn't empty.
    pub drawing: Option<DrawingId>,
    /// From the pixels of the layer's drawings to scene units: the layer's resolution, then
    /// every peg above it, as they are on the frame.
    pub to_scene: Affine,
    /// How near the viewer the pegs above it put it on the frame.
    pub depth: f64,
    /// From the pixels of the layer's drawings to the frame, as the shot's camera sees them
    /// on the frame: the frame's middle is its origin, and a pixel of the shot its unit.
    /// `None` for a layer that is at the camera or behind it, which is not seen, and for one
    /// that a peg tilts, which no flat transform puts on the frame.
    pub to_frame: Option<Affine>,
    /// The same for any layer that can be seen, tilted or not: how the plane of its drawings
    /// lands on the frame. For a layer that faces the camera squarely this is `to_frame`.
    pub projection: Option<Projection>,
    /// The deformer that bends the layer's drawings before any of this places them
    /// (plan §4.5): as the shot has it on the frame; or a chain above that bends them through
    /// their peg. [`Shot::warp_of`] says how they are bent, by both where both are.
    pub deformer: Option<DeformerId>,
    /// The one art layer of its drawings it is, where it comes from the output of that art
    /// layer; `None` for all of them, from the layer's flat output.
    pub art: Option<ArtLayerKind>,
}

impl Placed {
    /// Whether an art layer of `kind` is drawn of it: one that its layer has switched off is
    /// not, nor one that is not the art layer it is of.
    pub fn shows(&self, kind: ArtLayerKind, art_off: &[ArtLayerKind]) -> bool {
        !art_off.contains(&kind) && self.art.is_none_or(|art| art == kind)
    }

    /// The kinds of art layer not drawn of it, with `art_off` those its layer has switched off.
    pub fn off(&self, art_off: &[ArtLayerKind]) -> Vec<ArtLayerKind> {
        ArtLayerKind::ALL.into_iter().filter(|kind| !self.shows(*kind, art_off)).collect()
    }
}

/// What is drawn on a frame as one thing: a layer's drawing where the pegs and the camera
/// put it, or what goes through an effect together, which is one flat picture (plan §8.2).
#[derive(Clone, Debug, PartialEq)]
pub enum Card {
    Layer(Placed),
    /// The cards that go through `effect`, in the order to draw them in, made into one
    /// picture and changed by it. It is as near the viewer as the nearest of them.
    ///
    /// `mattes` is what is wired into its other ports, by their names, each in the order to
    /// draw it in: pictures the effect works by, which are not themselves seen.
    Through {
        effect: EffectId,
        depth: f64,
        inside: Vec<Card>,
        mattes: Vec<(String, Vec<Card>)>,
        /// Of a Particles node whose rain or snow falls in sheets, which of how many this card
        /// is, at its own depth, with only its particles.
        plane: Option<(u32, u32)>,
    },
    /// What a composite that makes one flat card of what it stacks gives (plan §8.6): the
    /// cards in the order to draw them in, which nothing from elsewhere comes between. It is
    /// as near the viewer as the nearest of them.
    Flat {
        depth: f64,
        inside: Vec<Card>,
    },
}

impl Card {
    /// How near the viewer the card is.
    pub fn depth(&self) -> f64 {
        match self {
            Card::Layer(placed) => placed.depth,
            Card::Through { depth, .. } | Card::Flat { depth, .. } => *depth,
        }
    }

    /// The card with every layer's drawing as `skin` shows it, or with none the day drawing,
    /// in place of the one the shot's own skin gave: what a Style node draws what goes through
    /// it with (plan §4.8).
    pub fn in_skin(&self, shot: &Shot, skin: Option<iskra_model::SkinId>) -> Card {
        let all = |cards: &[Card]| cards.iter().map(|card| card.in_skin(shot, skin)).collect::<Vec<_>>();
        match self {
            Card::Layer(placed) => {
                let layer = shot.layer(placed.layer);
                let drawing = placed.drawing.zip(layer).map(|(id, layer)| layer.shown(skin, layer.day_of(id)));
                Card::Layer(Placed { drawing: drawing.or(placed.drawing), ..*placed })
            }
            Card::Through { effect, depth, inside, mattes, plane } => Card::Through {
                effect: *effect,
                depth: *depth,
                inside: all(inside),
                mattes: mattes.iter().map(|(port, cards)| (port.clone(), all(cards))).collect(),
                plane: *plane,
            },
            Card::Flat { depth, inside } => Card::Flat { depth: *depth, inside: all(inside) },
        }
    }

    /// The card as what is drawn: itself, or for a flat card of a composite the cards in it,
    /// in the order to draw them in.
    pub fn leaves(&self) -> Vec<&Card> {
        match self {
            Card::Flat { inside, .. } => inside.iter().flat_map(Card::leaves).collect(),
            card => vec![card],
        }
    }

    /// The layers in the card, in the order they are drawn in.
    pub fn placed(&self) -> Vec<Placed> {
        let mut placed = Vec::new();
        self.layers(&mut placed);
        placed
    }

    fn layers(&self, into: &mut Vec<Placed>) {
        match self {
            Card::Layer(placed) => into.push(*placed),
            Card::Through { inside, .. } | Card::Flat { inside, .. } => inside.iter().for_each(|card| card.layers(into)),
        }
    }
}

/// What a shot shows on one frame: its layers that reach the display, in the order to draw
/// them in. What is farther from the viewer comes first, whatever composite it is on; layers
/// at one depth come as the composites stack them, bottom first (plan §4.4).
///
/// `cards` is the same with the effects: a layer that goes through none is a card of its own,
/// and what goes through an effect together is one card, placed among the others by its depth
/// and drawn as one picture. `layers` is every layer of the cards, whatever it goes through.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub layers: Vec<Placed>,
    pub cards: Vec<Card>,
}

impl Frame {
    pub fn layer(&self, id: LayerId) -> Option<&Placed> {
        self.layers.iter().find(|placed| placed.layer == id)
    }
}

/// The work of evaluating one frame that layers share.
struct Evaluation<'a> {
    shot: &'a Shot,
    frame: u32,
    /// Layers under the same pegs share the work of them: where they put a layer, how near,
    /// and for those that tilt how the camera sees their plane. Those a chain bends through
    /// their peg, which it carries in place of the peg, apart from those it does not.
    worlds: HashMap<(Option<PegId>, bool), (Affine, f64, Option<Projection>)>,
    /// What the camera makes of each depth.
    seen: HashMap<u64, Option<Affine>>,
    /// What has been listed: a node that reaches the display by two ways is listed where it
    /// comes first from the top.
    listed: HashSet<Source>,
    /// How many mattes deep the evaluation is.
    nesting: u32,
}

impl Evaluation<'_> {
    fn placed(&mut self, id: LayerId) -> Option<Placed> {
        let (shot, frame) = (self.shot, self.frame);
        // A layer whose row is hidden, or under one that is, is not in what the shot shows.
        let layer = shot.layer(id).filter(|_| !shot.hides(id))?;
        let bent = shot.bending(layer);
        let (world, depth, tilted) = *self.worlds.entry((layer.parent, bent.is_some())).or_insert_with(|| {
            let (world, depth) = (shot.layer_world(layer, frame), shot.depth(layer.parent, frame));
            if !shot.tilted(layer.parent, frame) {
                return (world, depth, None);
            }
            // A tilted layer is as near as its own origin is, wherever its pegs lean it to.
            (world, shot.space(layer.parent, frame).offset[2], shot.projected(layer.parent, frame))
        });
        // A deformer that is gone bends nothing; a chain above that bends it through its peg does.
        let deformer = layer.deformer.filter(|id| shot.deformer(*id).is_some()).or(bent);
        let pixels = Affine::scale(1.0 / layer.density);
        let to_scene = world.mul(&pixels);
        if shot.tilted(layer.parent, frame) {
            let projection = tilted.map(|projected| projected.of(&pixels));
            return Some(Placed {
                layer: id,
                drawing: layer.exposure.at(frame).map(|day| shot.shown(layer, day)),
                to_scene,
                depth,
                to_frame: None,
                projection,
                deformer,
                art: None,
            });
        }
        let to_frame = self.seen.entry(depth.to_bits()).or_insert_with(|| shot.seen(depth, frame)).map(|seen| seen.mul(&to_scene));
        let projection = to_frame.as_ref().map(Projection::flat);
        Some(Placed {
            layer: id,
            drawing: layer.exposure.at(frame).map(|day| shot.shown(layer, day)),
            to_scene,
            depth,
            to_frame,
            projection,
            deformer,
            art: None,
        })
    }

    /// The cards that `source` gives, the top first.
    fn cards(&mut self, source: Source) -> Vec<Card> {
        if !self.listed.insert(source) {
            return Vec::new();
        }
        match source {
            Source::Layer(id) => self.placed(id).map(Card::Layer).into_iter().collect(),
            Source::Art(id, kind) => self.placed(id).map(|placed| Card::Layer(Placed { art: Some(kind), ..placed })).into_iter().collect(),
            Source::Composite(id) => {
                let Some(composite) = self.shot.composite(id) else { return Vec::new() };
                let (stacking, mut inputs) = (composite.stacking, composite.inputs.clone());
                // Stacked by its ports, either end of them can be the top.
                if stacking == Stacking::Ports && composite.right_first {
                    inputs.reverse();
                }
                let cards: Vec<Card> = inputs.into_iter().flat_map(|input| self.cards(input)).collect();
                let inside = match stacking {
                    // Each card goes on as it is, to find its place by its depth.
                    Stacking::Depth => return cards,
                    Stacking::DepthFlat => in_order(cards),
                    // As they are stacked and no other way: the bottom is drawn first.
                    Stacking::Ports => cards.into_iter().rev().collect(),
                };
                if inside.is_empty() {
                    return inside;
                }
                let depth = inside.iter().map(Card::depth).fold(f64::NEG_INFINITY, f64::max);
                vec![Card::Flat { depth, inside }]
            }
            Source::Effect(id) => {
                let Some(effect) = self.shot.effect(id) else { return Vec::new() };
                // A Visible node shows what comes into it or hides it, as it is on the frame,
                // and changes nothing of it: what it shows goes on as it came.
                let visible = effect.enabled && effect.kind == VISIBLE;
                if visible && !effect.switch("shown", self.frame) {
                    return Vec::new();
                }
                // A Receive gives what the Send it names is given; switched off, nothing.
                if effect.kind == iskra_model::RECEIVE {
                    let send = self.shot.sender(effect).filter(|_| effect.enabled).map(|send| Source::Effect(send.id));
                    return send.map(|send| self.cards(send)).unwrap_or_default();
                }
                // A node that makes its picture itself is a card of its own, at the depth it says,
                // with what is wired into its ports, which it draws by; switched off, it makes
                // nothing.
                if effect.described().is_some_and(iskra_model::EffectKind::makes_picture) {
                    if !effect.enabled {
                        return Vec::new();
                    }
                    let depth = effect.number(iskra_model::DEPTH, self.frame);
                    let mattes = self.mattes(effect);
                    // Rain or snow in sheets: a card for each, evenly from the depth back.
                    let planes = match effect.kind == iskra_model::PARTICLES && matches!(effect.choice("kind", self.frame).as_str(), "Rain" | "Snow") {
                        true => effect.number("planes", self.frame).round().clamp(1.0, 4.0) as u32,
                        false => 1,
                    };
                    if planes > 1 {
                        let range = effect.number("depthRange", self.frame).max(0.0);
                        let sheet = |at: u32| Card::Through {
                            effect: id,
                            depth: depth - range * f64::from(at) / f64::from(planes - 1),
                            inside: Vec::new(),
                            mattes: mattes.clone(),
                            plane: Some((at, planes)),
                        };
                        return (0..planes).map(sheet).collect();
                    }
                    return vec![Card::Through { effect: id, depth, inside: Vec::new(), mattes, plane: None }];
                }
                let inside = effect.picture().map(|picture| self.cards(picture)).unwrap_or_default();
                // A port and a Send hand on what comes into them as it came.
                if visible || effect.hands_on() {
                    return inside;
                }
                // One that is off, or of a kind this build does not know, lets its picture
                // through as it is; one with nothing going through it gives nothing.
                if !effect.enabled || effect.described().is_none() || inside.is_empty() {
                    return inside;
                }
                let inside = in_order(inside);
                let depth = inside.iter().map(Card::depth).fold(f64::NEG_INFINITY, f64::max);
                let mattes = self.mattes(effect);
                vec![Card::Through { effect: id, depth, inside, mattes, plane: None }]
            }
        }
    }

    /// What is wired into an effect's ports other than its picture, by their names, each as
    /// cards in the order to draw them in. What a matte is made of is listed by itself: it may
    /// be seen elsewhere too. Effects in a ring, which the engine refuses, would never end here.
    fn mattes(&mut self, effect: &iskra_model::Effect) -> Vec<(String, Vec<Card>)> {
        let others: Vec<(String, Source)> =
            effect.inputs.iter().filter(|(port, _)| port.as_str() != PICTURE).map(|(port, source)| (port.clone(), *source)).collect();
        let mut mattes = Vec::new();
        for (port, source) in others {
            if self.nesting >= 32 {
                break;
            }
            let kept = std::mem::take(&mut self.listed);
            self.nesting += 1;
            mattes.push((port, in_order(self.cards(source))));
            self.nesting -= 1;
            self.listed = kept;
        }
        mattes
    }
}

/// Cards listed the top first, in the order to draw them in: the farthest first, and those
/// equally near as they were stacked, the bottom first.
fn in_order(mut cards: Vec<Card>) -> Vec<Card> {
    cards.reverse();
    // The sort keeps the order of what is equally near; a depth of -0, which the pegs' sum
    // can give, is as near as 0 (adding 0 makes it 0).
    cards.sort_by(|a, b| (a.depth() + 0.0).total_cmp(&(b.depth() + 0.0)));
    cards
}

/// Evaluate `shot` on `frame`.
pub fn evaluate(shot: &Shot, frame: u32) -> Frame {
    let mut evaluation = Evaluation { shot, frame, worlds: HashMap::new(), seen: HashMap::new(), listed: HashSet::new(), nesting: 0 };
    let cards = in_order(shot.display.map(|display| evaluation.cards(display)).unwrap_or_default());
    let mut layers = Vec::new();
    cards.iter().for_each(|card| card.layers(&mut layers));
    Frame { layers, cards }
}

/// How many pixels of the picture one pixel of a drawing may be across before the drawing is
/// said to be shown too large (plan §4.10).
pub const COARSE: f64 = 1.5;

/// The layers that the camera shows on an evaluated frame of `shot` so large that their pixels
/// show, each with how many pixels of the picture one of its own is across. A layer with
/// nothing painted on the frame is not among them, nor is one seen in perspective.
pub fn enlarged(shot: &Shot, frame: &Frame) -> Vec<(LayerId, f64)> {
    let enlarged = frame.layers.iter().filter_map(|placed| {
        let drawing = shot.layer(placed.layer)?.drawing(placed.drawing?)?;
        let across = placed.to_frame?.determinant().abs().sqrt();
        (across > COARSE && !drawing.drawing.bounds().is_empty()).then_some((placed.layer, across))
    });
    enlarged.collect()
}

/// For each layer of `shot` that is shown too large on some frame (see [`enlarged`]): the
/// stretches of frames on which it is, as first frame and one past the last.
pub fn enlarged_frames(shot: &Shot) -> HashMap<LayerId, Vec<(u32, u32)>> {
    let mut stretches: HashMap<LayerId, Vec<(u32, u32)>> = HashMap::new();
    for frame in 0..shot.duration {
        for (layer, _) in enlarged(shot, &evaluate(shot, frame)) {
            let of_layer = stretches.entry(layer).or_default();
            match of_layer.last_mut() {
                Some(last) if last.1 == frame => last.1 = frame + 1,
                _ => of_layer.push((frame, frame + 1)),
            }
        }
    }
    stretches
}

/// Evaluated frames of one shot, kept while the shot doesn't change. A shot is a snapshot:
/// when anything in it is edited the project holds a new one, and the frames of the old one
/// are let go.
#[derive(Default)]
pub struct Cache {
    /// The shot the frames are of. Holding it keeps its address from being used again.
    shot: Option<Arc<Shot>>,
    frames: HashMap<u32, Arc<Frame>>,
    /// How many frames were evaluated, as against found: for tests and for tuning.
    pub evaluated: u64,
}

impl Cache {
    /// What `shot` shows on `frame`, evaluated at most once for as long as the shot is the same.
    pub fn frame(&mut self, shot: &Arc<Shot>, frame: u32) -> Arc<Frame> {
        if !self.shot.as_ref().is_some_and(|kept| Arc::ptr_eq(kept, shot)) {
            (self.shot, self.frames) = (Some(shot.clone()), HashMap::new());
        }
        if let Some(found) = self.frames.get(&frame) {
            return found.clone();
        }
        self.evaluated += 1;
        let made = Arc::new(evaluate(shot, frame));
        self.frames.insert(frame, made.clone());
        made
    }
}

#[cfg(test)]
mod tests {
    use iskra_model::{Peg, PoolDrawing, Project, Role, Source, Transform};

    use super::*;

    /// A shot with a sky under a ball; the ball is on a peg that moves it from frame 0 to 10,
    /// and has a drawing on frames 0 to 4 only.
    fn shot() -> (Arc<Shot>, LayerId, LayerId) {
        let mut project = Project::default();
        let mut ball = project.new_layer("Ball", Role::Animation);
        let (drawing, peg) = (DrawingId(project.new_id()), PegId(project.new_id()));
        ball.drawings.push(PoolDrawing { id: drawing, name: "1".to_string(), drawing: Arc::default() });
        ball.exposure.set(0, 5, Some(drawing));
        ball.parent = Some(peg);
        let mut bounce = Peg {
            id: peg,
            name: "Bounce".to_string(),
            parent: None,
            transform: Transform::default(),
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        };
        bounce.animation.position.set(0, [0.0, 0.0]);
        bounce.animation.position.set(10, [100.0, 0.0]);
        let shot = Arc::make_mut(&mut project.shots[0]);
        let (sky, ball_id) = (shot.layers[0].id, ball.id);
        shot.connect_on_top(ball.id);
        shot.layers.push(Arc::new(ball));
        shot.pegs.push(Arc::new(bounce));
        (project.shots[0].clone(), sky, ball_id)
    }

    #[test]
    fn a_frame_lists_what_reaches_the_display_bottom_first_and_where_the_pegs_put_it() {
        let (shot, sky, ball) = shot();
        let frame = evaluate(&shot, 5);
        assert_eq!(frame.layers.iter().map(|placed| placed.layer).collect::<Vec<_>>(), [sky, ball], "the sky is drawn first, under the ball");
        // The ball has two drawing pixels a unit and is half way along its move.
        assert_eq!(frame.layer(ball).unwrap().to_scene.m, [0.5, 0.0, 0.0, 0.5, 50.0, 0.0]);
        assert_eq!(frame.layer(sky).unwrap().to_scene.m, [0.5, 0.0, 0.0, 0.5, 0.0, 0.0]);
        assert!(evaluate(&shot, 4).layer(ball).unwrap().drawing.is_some() && frame.layer(ball).unwrap().drawing.is_none(), "its cell on frame 5 is empty");

        // Depth decides before the composites do: the ball moved back is drawn under the sky,
        // on the frames on which it is farther, and a peg above adds its depth.
        let mut deep = Shot::clone(&shot);
        let peg = Arc::make_mut(&mut deep.pegs[0]);
        peg.animation.depth.set(0, -1.0);
        peg.animation.depth.set(10, 1.0);
        let order = |shot: &Shot, frame: u32| evaluate(shot, frame).layers.iter().map(|placed| placed.layer).collect::<Vec<_>>();
        assert_eq!((order(&deep, 2), order(&deep, 5), order(&deep, 8)), (vec![ball, sky], vec![sky, ball], vec![sky, ball]), "equally near, as stacked");
        assert_eq!(evaluate(&deep, 0).layer(ball).unwrap().depth, -1.0);
        let above = PegId(deep.pegs[0].id.0 + 1000);
        let lift = Transform { depth: 3.0, ..Transform::default() };
        Arc::make_mut(&mut deep.pegs[0]).parent = Some(above);
        deep.pegs.push(Arc::new(Peg {
            id: above,
            name: "Lift".to_string(),
            parent: None,
            transform: lift,
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        assert_eq!((order(&deep, 0), evaluate(&deep, 0).layer(ball).unwrap().depth), (vec![sky, ball], 2.0));

        // What is connected to nothing the display shows is not on the frame at all.
        let mut alone = Shot::clone(&shot);
        alone.display = Some(Source::Layer(sky));
        assert_eq!(evaluate(&alone, 5).layers.len(), 1);
        alone.display = None;
        assert_eq!(evaluate(&alone, 5), Frame::default());
    }

    #[test]
    fn a_composite_orders_what_it_stacks_by_depth_or_by_its_ports() {
        use iskra_model::{Composite, CompositeId, Stacking};
        // The ball is stacked over the sky, and its peg has moved it back, behind the sky.
        let (shot, sky, ball) = shot();
        let mut shot = Shot::clone(&shot);
        Arc::make_mut(&mut shot.pegs[0]).transform.depth = -1.0;
        let order = |shot: &Shot| evaluate(shot, 0).layers.iter().map(|placed| placed.layer).collect::<Vec<_>>();
        let set = |shot: &mut Shot, stacking: Stacking, right_first: bool| {
            let composite = Arc::make_mut(&mut shot.composites[0]);
            (composite.stacking, composite.right_first) = (stacking, right_first);
        };
        // By depth, each layer is a card of its own, and the ball is drawn first, under.
        assert_eq!((order(&shot), evaluate(&shot, 0).cards.len()), (vec![ball, sky], 2));
        // By its ports the depths count for nothing: the sky first, as it is stacked, and
        // all of it one flat card, as near as the nearest in it.
        set(&mut shot, Stacking::Ports, false);
        let frame = evaluate(&shot, 0);
        assert_eq!((order(&shot), frame.cards.len(), frame.cards[0].depth(), frame.cards[0].leaves().len()), (vec![sky, ball], 1, 0.0, 2));
        // With the right port on top, the stack is the other way up.
        set(&mut shot, Stacking::Ports, true);
        assert_eq!(order(&shot), [ball, sky]);
        // Flat by depth: ordered as by depth, and one card.
        set(&mut shot, Stacking::DepthFlat, true);
        assert_eq!((order(&shot), evaluate(&shot, 0).cards.len()), (vec![ball, sky], 1));

        // A cloud in a composite of its own, on top, half way back between the sky and the
        // ball. By depth it comes between them; when the first composite is flat, nothing
        // comes between what it stacks, and the cloud is behind all of it or before.
        let (cloud_id, peg, top) = (LayerId(1001), PegId(1002), CompositeId(1003));
        let mut cloud = iskra_model::Layer::clone(shot.layer(ball).unwrap());
        (cloud.id, cloud.name, cloud.parent) = (cloud_id, "Cloud".to_string(), Some(peg));
        let back = Transform { depth: -0.5, ..Transform::default() };
        shot.pegs.push(Arc::new(Peg {
            id: peg,
            name: "Back".to_string(),
            parent: None,
            transform: back,
            animation: Default::default(),
            template: None,
            follows: None,
            inherit: Default::default(),
            matches: Default::default(),
        }));
        shot.layers.push(Arc::new(cloud));
        let first = Source::Composite(shot.composites[0].id);
        shot.composites.push(Arc::new(Composite::new(top, "Top", vec![Source::Layer(cloud_id), first])));
        shot.display = Some(Source::Composite(top));
        set(&mut shot, Stacking::Depth, false);
        assert_eq!(order(&shot), [ball, cloud_id, sky]);
        set(&mut shot, Stacking::DepthFlat, false);
        assert_eq!(order(&shot), [cloud_id, ball, sky], "the flat card is as near as the sky, which is nearer than the cloud");
        set(&mut shot, Stacking::Ports, false);
        assert_eq!(order(&shot), [cloud_id, sky, ball]);
        // The composite on top stacked by its ports puts the cloud over all, wherever it is.
        let outer = Arc::make_mut(&mut shot.composites[1]);
        outer.stacking = Stacking::Ports;
        assert_eq!(order(&shot), [sky, ball, cloud_id]);
        // A composite that stacks nothing gives no card.
        Arc::make_mut(&mut shot.composites[0]).inputs.clear();
        assert_eq!((order(&shot), evaluate(&shot, 0).cards[0].leaves().len()), (vec![cloud_id], 1));
    }

    #[test]
    fn a_frame_is_evaluated_once_for_as_long_as_the_shot_is_the_same() {
        let (shot, _, ball) = shot();
        let mut cache = Cache::default();
        let first = cache.frame(&shot, 5);
        assert!(Arc::ptr_eq(&first, &cache.frame(&shot, 5)) && cache.evaluated == 1);
        cache.frame(&shot, 6);
        assert_eq!(cache.evaluated, 2);
        // An edit makes a new shot, and the frames of the old one are let go.
        let mut edited = Shot::clone(&shot);
        Arc::make_mut(&mut edited.pegs[0]).animation.position.set(10, [200.0, 0.0]);
        let edited = Arc::new(edited);
        assert_eq!(cache.frame(&edited, 5).layer(ball).unwrap().to_scene.m[4], 100.0);
        assert_eq!((cache.evaluated, first.layer(ball).unwrap().to_scene.m[4]), (3, 50.0));
        cache.frame(&shot, 5);
        assert_eq!(cache.evaluated, 4, "going back to the old shot evaluates it again: one shot's frames are kept");
    }
}
