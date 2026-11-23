//! Golden pictures: small scenes rendered by the reference compositor and held against PNG
//! files kept in `tests/golden`. A change to the compositor that changes a picture shows here.
//!
//! When a picture is meant to change, run the tests with `ISKRA_BLESS=1` to write the files
//! anew, and look at them before committing.

use std::path::PathBuf;
use std::sync::Arc;

use common::{scene, shoot};
use iskra_drawing::Rect;
use iskra_model::{Animation, Peg, PegId, Transform, WorkingSpace};
use iskra_render::{Image, View, render};

mod common;

/// Hold `image` against the golden picture `name`, a level of each channel apart at most:
/// the last digit of a sine may differ from one machine to the next.
fn golden(name: &str, image: &Image) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.png"));
    if std::env::var_os("ISKRA_BLESS").is_some() {
        std::fs::write(&path, iskra_io::encode_png(image.width, image.height, &image.pixels).unwrap()).unwrap();
        return;
    }
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}. Run the tests with ISKRA_BLESS=1 to make it", path.display()));
    let (width, height, pixels) = iskra_io::decode_png(&bytes).unwrap();
    assert_eq!((width, height), (image.width, image.height), "{name}: the size of the picture");
    let apart = |a: &[u8; 4], b: &[u8; 4]| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    let off = image.pixels.iter().zip(&pixels).filter(|(a, b)| apart(a, b) > 1).count();
    if off > 0 {
        let actual = std::env::temp_dir().join(format!("iskra-{name}.actual.png"));
        std::fs::write(&actual, iskra_io::encode_png(image.width, image.height, &image.pixels).unwrap()).unwrap();
        panic!("{name}: {off} pixels differ from {}; what was rendered is in {}", path.display(), actual.display());
    }
}

#[test]
fn layers_are_put_over_each_other_where_their_pegs_put_them() {
    let (project, ..) = scene();
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("stack", &image);
    // The blue field, the band half over it, the red of the turned blob, and nothing outside.
    assert_eq!((image.pixel(60, 30), image.pixel(2, 2)), ([38, 110, 210, 255], [0, 0, 0, 0]));
    assert_eq!(image.pixel(60, 90), [147, 183, 233, 255], "white, half covering, over the blue");
    assert_eq!(image.pixel(200, 78), [220, 50, 47, 255]);
}

#[test]
fn what_goes_through_an_effect_is_changed_as_one_and_what_does_not_is_left() {
    use iskra_model::{Effect, EffectId, PICTURE, Source, Value, effect_kind};
    let (mut project, ball, _) = scene();
    let plain = shoot(&project, WorkingSpace::Srgb, 1.0);
    // The ball goes through a transparency, on its own wire.
    let fade = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mut node = Effect::new(fade, "Fade", effect_kind("transparency").unwrap());
    node.inputs.insert(PICTURE.to_string(), Source::Layer(ball));
    // What took the ball takes it through the node, which is then wired to the ball.
    shot.rewire(Source::Layer(ball), Some(Source::Effect(fade)));
    shot.effects.push(Arc::new(node));
    assert_eq!(shot.composites[0].inputs[0], Source::Effect(fade), "the end of the ball's wire is what is stacked");
    let set = |project: &mut iskra_model::Project, amount: f64, enabled: bool| {
        let node = Arc::make_mut(&mut project.shots[0]).effect_mut(fade).unwrap();
        (node.param_mut("amount").unwrap().value, node.enabled) = (Value::Number(amount), enabled);
    };
    // At nothing it changes nothing; half way the ball is half over the field; at all of it
    // the ball is gone and the rest is as it was.
    set(&mut project, 0.0, true);
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), plain);
    set(&mut project, 50.0, true);
    let half = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert_eq!((half.pixel(200, 78), half.pixel(60, 30), half.pixel(60, 90)), ([129, 80, 129, 255], plain.pixel(60, 30), plain.pixel(60, 90)));
    set(&mut project, 100.0, true);
    let gone = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert_eq!((gone.pixel(200, 78), gone.pixel(60, 90)), ([38, 110, 210, 255], plain.pixel(60, 90)));
    // Switched off, it lets the picture through as it is; and so does a kind that is not known.
    set(&mut project, 100.0, false);
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), plain);
    set(&mut project, 100.0, true);
    Arc::make_mut(&mut project.shots[0]).effect_mut(fade).unwrap().kind = "hologram".to_string();
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), plain);
    Arc::make_mut(&mut project.shots[0]).effect_mut(fade).unwrap().kind = "transparency".to_string();

    // Keyed, it fades over the frames.
    let node = Arc::make_mut(&mut project.shots[0]).effect_mut(fade).unwrap();
    node.param_mut("amount").unwrap().keys.set(0, 0.0);
    node.param_mut("amount").unwrap().keys.set(10, 100.0);
    let at = |frame: u32| render(&project.shots[0], &project.palette, WorkingSpace::Srgb, frame, &View::frame(&project.settings, 1.0)).pixel(200, 78);
    assert_eq!((at(0), at(5), at(10)), (plain.pixel(200, 78), [129, 80, 129, 255], [38, 110, 210, 255]));

    // After the composite, all that it stacks goes through together: the whole picture fades.
    let (mut project, ..) = scene();
    let all = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let shown = shot.display.unwrap();
    let mut node = Effect::new(all, "Fade All", effect_kind("transparency").unwrap());
    node.inputs.insert(PICTURE.to_string(), shown);
    shot.effects.push(Arc::new(node));
    shot.display = Some(Source::Effect(all));
    let faded = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert_eq!((faded.pixel(60, 30), faded.pixel(200, 78), faded.pixel(2, 2)), ([38, 110, 210, 128], [220, 50, 47, 128], [0, 0, 0, 0]));
}

/// Stack a Colour Card under everything in the scene's shot, of `colour`, at `depth`.
pub(crate) fn colour_card(project: &mut iskra_model::Project, colour: [f64; 4], depth: f64) -> iskra_model::EffectId {
    use iskra_model::{COLOUR_CARD, DEPTH, Effect, EffectId, Source, Value, effect_kind};
    let id = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mut node = Effect::new(id, "Card", effect_kind(COLOUR_CARD).unwrap());
    node.param_mut("colour").unwrap().value = Value::Colour(colour);
    node.param_mut(DEPTH).unwrap().value = Value::Number(depth);
    let main = shot.main_composite().unwrap();
    let composite = shot.composites.iter_mut().find(|composite| composite.id == main).unwrap();
    Arc::make_mut(composite).inputs.push(Source::Effect(id));
    shot.effects.push(Arc::new(node));
    id
}

#[test]
fn a_colour_card_fills_the_frame_at_its_depth() {
    let (mut project, ..) = scene();
    let card = colour_card(&mut project, [0.2, 0.6, 0.3, 1.0], 0.0);
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    // Under the rest at depth 0, by the composite's order: all of the frame, round the field.
    let green = image.pixel(2, 2);
    assert_eq!((green[3], image.pixel(317, 177)), (255, green), "{green:?}");
    assert!(green[1] > green[0] && green[1] > green[2], "{green:?}");
    assert_eq!(image.pixel(60, 30), [38, 110, 210, 255], "the field is over it");
    // Brought nearer than the rest, it covers them; switched off, it is not there.
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == card).unwrap()).param_mut(iskra_model::DEPTH).unwrap().value =
        iskra_model::Value::Number(1.0);
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0).pixel(60, 30), green);
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == card).unwrap()).enabled = false;
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0).pixel(2, 2), [0, 0, 0, 0]);
}

/// A drawing of one square of `swatch`, `side` pixels across about its origin.
fn square(side: i32, swatch: u16) -> Arc<iskra_model::Drawing> {
    use iskra_model::{ArtContent, SwatchId};
    let mut drawing = iskra_model::Drawing::default();
    if let ArtContent::Indexed(surface) = &mut drawing.art_layers[1].content {
        let rect = Rect::new(-side / 2, -side / 2, side / 2, side / 2);
        surface.paint(rect, &vec![1.0; (side * side) as usize], SwatchId(swatch));
    }
    Arc::new(drawing)
}

#[test]
fn particles_are_drawn_as_every_drawing_of_the_layer_in_their_sprite_port() {
    use iskra_model::{DrawingId, Effect, EffectId, PARTICLES, PoolDrawing, SPRITE, Source, Value, effect_kind};
    use iskra_render::Cache;
    // The ball's layer, with a blue square beside its red blob, seen only as the particles
    // of a snow: neither it nor the blue field is in the composite.
    let (mut project, ball, _) = scene();
    let (other, node) = (DrawingId(project.new_id()), EffectId(project.new_id()));
    let shot = Arc::make_mut(&mut project.shots[0]);
    let field = shot.layers[2].id;
    shot.disconnect(Source::Layer(ball));
    shot.disconnect(Source::Layer(field));
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    layer.drawings.push(PoolDrawing { id: other, name: "2".to_string(), drawing: square(60, 5) });
    let mut snow = Effect::new(node, "Snow", effect_kind(PARTICLES).unwrap());
    for (param, value) in
        [("kind", Value::Choice("Snow".to_string())), ("count", Value::Number(40.0)), ("far", Value::Number(0.0)), ("spriteScale", Value::Number(40.0))]
    {
        snow.param_mut(param).unwrap().value = value;
    }
    snow.inputs.insert(SPRITE.to_string(), Source::Layer(ball));
    let main = shot.main_composite().unwrap();
    Arc::make_mut(shot.composites.iter_mut().find(|composite| composite.id == main).unwrap()).inputs.insert(0, Source::Effect(node));
    shot.effects.push(Arc::new(snow));
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    let of = |image: &Image, colour: [u8; 4]| image.pixels.iter().filter(|pixel| **pixel == colour).count();
    let (red, blue) = (of(&image, [220, 50, 47, 255]), of(&image, [38, 110, 210, 255]));
    assert!(red > 20 && blue > 20, "both drawings are flakes: {red} red, {blue} blue");

    // The drawing that is not exposed changed, the cache makes the picture again.
    let view = View::frame(&project.settings, 1.0);
    let cache = Cache::default();
    let kept = |project: &iskra_model::Project| cache.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view);
    assert_eq!(kept(&project), render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view));
    let shot = Arc::make_mut(&mut project.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    layer.drawings.iter_mut().find(|entry| entry.id == other).unwrap().drawing = square(60, 3);
    let again = kept(&project);
    assert_eq!(again, render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view));
    assert_eq!(of(&again, [38, 110, 210, 255]), 0, "no blue flake is left");
    assert_eq!(cache.counts().0, 2, "made twice");
    // Tinted all the way with the node's colour, a flake keeps its shape and has no red.
    let tinted = |project: &mut iskra_model::Project, set: &[(&str, Value)]| {
        let shot = Arc::make_mut(&mut project.shots[0]);
        let snow = Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == node).unwrap());
        for (param, value) in set {
            snow.param_mut(param).unwrap().value = value.clone();
        }
        shoot(project, WorkingSpace::Srgb, 1.0)
    };
    let green = tinted(&mut project, &[("tint", Value::Number(100.0)), ("colour", Value::Colour([0.0, 1.0, 0.0, 1.0]))]);
    assert_eq!(of(&green, [220, 50, 47, 255]), 0);
    assert!(green.pixels.iter().filter(|pixel| pixel[1] > 200 && pixel[0] < 40 && pixel[3] == 255).count() > 20);
    // One parallel speed line drawn as the drawing: stretched across the frame, and low.
    let line = tinted(
        &mut project,
        &[
            ("tint", Value::Number(0.0)),
            ("kind", Value::Choice("Speed Lines".to_string())),
            ("lines", Value::Choice("Parallel".to_string())),
            ("count", Value::Number(1.0)),
            ("clear", Value::Number(0.0)),
            ("lengthVariation", Value::Number(0.0)),
            ("spriteScale", Value::Number(10.0)),
        ],
    );
    // The red of the drawing, not the white of the mist, which is in the composite.
    let red = |pixel: &[u8; 4]| pixel[0] > 150 && pixel[1] < 100;
    let painted: Vec<(u32, u32)> = line.pixels.iter().enumerate().filter(|(_, pixel)| red(pixel)).map(|(i, _)| (i as u32 % 320, i as u32 / 320)).collect();
    let span = |of: fn(&(u32, u32)) -> u32| painted.iter().map(of).max().unwrap_or(0) - painted.iter().map(of).min().unwrap_or(0);
    assert!(span(|at| at.0) > 300 && span(|at| at.1) < 20, "{} across, {} down", span(|at| at.0), span(|at| at.1));
}

#[test]
fn rain_seen_through_the_camera_is_made_again_when_the_camera_moves() {
    use iskra_model::{Effect, EffectId, PARTICLES, Source, Value, effect_kind};
    use iskra_render::Cache;
    let (mut project, ..) = scene();
    let (node, rig) = (EffectId(project.new_id()), PegId(project.new_id()));
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mut rain = Effect::new(node, "Rain", effect_kind(PARTICLES).unwrap());
    rain.param_mut("depthRange").unwrap().value = Value::Number(200.0);
    Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(node));
    shot.effects.push(Arc::new(rain));
    shot.pegs.push(Arc::new(Peg {
        id: rig,
        name: "Camera-P".to_string(),
        parent: None,
        transform: Transform::default(),
        animation: Animation::default(),
        template: None,
        follows: None,
        inherit: Default::default(),
        matches: Default::default(),
    }));
    shot.camera.parent = Some(rig);
    let view = View::frame(&project.settings, 1.0);
    let cache = Cache::default();
    let plain = |project: &iskra_model::Project| render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view);
    let kept = |project: &iskra_model::Project| cache.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view);
    let before = kept(&project);
    assert_eq!(before, plain(&project));
    // The camera panned: the rain is seen elsewhere, made again, not found.
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(shot.pegs.iter_mut().find(|peg| peg.id == rig).unwrap()).transform.position = [25.0, 0.0];
    let after = kept(&project);
    assert_eq!(after, plain(&project));
    assert_ne!(after, before);
    assert_eq!(cache.counts().0, 2);
}

#[test]
fn rain_drawn_as_sprites_leans_with_the_wind_and_splashes_as_its_own_sprites() {
    use iskra_model::{ArtContent, DrawingId, Effect, EffectId, PARTICLES, PoolDrawing, SPLASH_SPRITE, SPRITE, Source, SwatchId, Value, effect_kind};
    let (mut project, ball, _) = scene();
    let (bar, node) = (DrawingId(project.new_id()), EffectId(project.new_id()));
    let shot = Arc::make_mut(&mut project.shots[0]);
    let field = shot.layers[2].id;
    shot.disconnect(Source::Layer(ball));
    shot.disconnect(Source::Layer(field));
    // The ball's layer has one drawing, an upright bar of red, 4 wide and 40 high about its middle.
    let mut upright = iskra_model::Drawing::default();
    if let ArtContent::Indexed(surface) = &mut upright.art_layers[1].content {
        surface.paint(Rect::new(-2, -20, 2, 20), &[1.0; 160], SwatchId(3));
    }
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    layer.drawings = vec![PoolDrawing { id: bar, name: "1".to_string(), drawing: Arc::new(upright) }];
    let mut rain = Effect::new(node, "Rain", effect_kind(PARTICLES).unwrap());
    for (param, value) in [("count", Value::Number(1.0)), ("far", Value::Number(0.0)), ("speedVariation", Value::Number(0.0)), ("seed", Value::Number(4.0))] {
        rain.param_mut(param).unwrap().value = value;
    }
    rain.inputs.insert(SPRITE.to_string(), Source::Layer(ball));
    Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(node));
    shot.effects.push(Arc::new(rain));
    let set = |project: &mut iskra_model::Project, param: &str, value: Value| {
        let shot = Arc::make_mut(&mut project.shots[0]);
        Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == node).unwrap()).param_mut(param).unwrap().value = value;
    };
    let red = |pixel: &[u8; 4]| pixel[0] > 150 && pixel[1] < 100 && pixel[3] > 128;
    let extent = |image: &Image| {
        let painted: Vec<(u32, u32)> = image.pixels.iter().enumerate().filter(|(_, pixel)| red(pixel)).map(|(i, _)| (i as u32 % 320, i as u32 / 320)).collect();
        let span = |of: fn(&(u32, u32)) -> u32| painted.iter().map(of).max().unwrap_or(0) - painted.iter().map(of).min().unwrap_or(0);
        (span(|at| at.0), span(|at| at.1))
    };
    // Its drawing is at twice the shot's pixels: 2 wide and 20 high on the frame. With no wind
    // it falls upright; blown aside as fast as it falls, it leans half way over.
    let (wide, high) = extent(&shoot(&project, WorkingSpace::Srgb, 1.0));
    assert!(high > 16 && wide < 5, "upright: {wide} by {high}");
    set(&mut project, "wind", Value::Number(180.0 / 18.0));
    let (wide, high) = extent(&shoot(&project, WorkingSpace::Srgb, 1.0));
    assert!(wide > 10 && high > 10 && wide.abs_diff(high) < 5, "leaning: {wide} by {high}");
    // Splashing, with the blue field in its splash's port: blue droplets, many of them.
    set(&mut project, "count", Value::Number(400.0));
    set(&mut project, "splash", Value::Switch(true));
    set(&mut project, "ground", Value::Number(40.0));
    set(&mut project, "spriteScale", Value::Number(5.0));
    set(&mut project, "splashScale", Value::Number(5.0));
    let blue = |image: &Image| image.pixels.iter().filter(|pixel| pixel[2] > 150 && pixel[0] < 80 && pixel[3] > 128).count();
    assert_eq!(blue(&shoot(&project, WorkingSpace::Srgb, 1.0)), 0, "no splash sprite yet: the drops' own, small");
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == node).unwrap()).inputs.insert(SPLASH_SPRITE.to_string(), Source::Layer(field));
    assert!(blue(&shoot(&project, WorkingSpace::Srgb, 1.0)) > 50);
}

#[test]
fn a_splash_of_several_drawings_plays_them_where_a_drop_lands() {
    use iskra_model::{DrawingId, Effect, EffectId, PARTICLES, PoolDrawing, SPLASH_SPRITE, Source, Value, effect_kind};
    let (mut project, ball, _) = scene();
    let (blue, red, node) = (DrawingId(project.new_id()), DrawingId(project.new_id()), EffectId(project.new_id()));
    let shot = Arc::make_mut(&mut project.shots[0]);
    let field = shot.layers[2].id;
    shot.disconnect(Source::Layer(ball));
    shot.disconnect(Source::Layer(field));
    // The field's layer is a splash of two drawings on twos: a blue square, then a red one.
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == field).unwrap());
    layer.drawings =
        vec![PoolDrawing { id: blue, name: "1".to_string(), drawing: square(16, 5) }, PoolDrawing { id: red, name: "2".to_string(), drawing: square(16, 3) }];
    layer.exposure = Default::default();
    layer.exposure.set(0, 2, Some(blue));
    layer.exposure.set(2, 2, Some(red));
    let mut rain = Effect::new(node, "Rain", effect_kind(PARTICLES).unwrap());
    for (param, value) in [("count", Value::Number(600.0)), ("splash", Value::Switch(true)), ("ground", Value::Number(40.0))] {
        rain.param_mut(param).unwrap().value = value;
    }
    rain.inputs.insert(SPLASH_SPRITE.to_string(), Source::Layer(field));
    Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(node));
    shot.effects.push(Arc::new(rain));
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    let of = |colour: [u8; 4]| image.pixels.iter().filter(|pixel| **pixel == colour).count();
    // Drops landed two frames ago and less show the first drawing, those before the second:
    // both are about, each a whole square of 8 by 8 at its drawn size, by the ground.
    let (blues, reds) = (of([38, 110, 210, 255]), of([220, 50, 47, 255]));
    assert!(blues > 64 && reds > 64, "{blues} blue, {reds} red");
    let rows = |colour: [u8; 4]| image.pixels.chunks(320).enumerate().filter(|(_, row)| row.contains(&colour)).map(|(y, _)| y).collect::<Vec<_>>();
    assert!(rows([220, 50, 47, 255]).iter().all(|y| (120..=140).contains(y)), "about the ground: {:?}", rows([220, 50, 47, 255]));
}

#[test]
fn a_particle_is_where_it_is_on_the_frame_whatever_the_view_looks_at_it_by() {
    use iskra_drawing::Affine;
    use iskra_model::{Effect, EffectId, PARTICLES, Source, Value, effect_kind};
    let (mut project, ball, _) = scene();
    let node = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let field = shot.layers[2].id;
    for layer in [ball, field, shot.layers[1].id] {
        shot.disconnect(Source::Layer(layer));
    }
    // One drop, on a frame far into its fall, when it has gone round its tile many times.
    let mut rain = Effect::new(node, "Rain", effect_kind(PARTICLES).unwrap());
    for (param, value) in [("count", Value::Number(1.0)), ("far", Value::Number(0.0)), ("seed", Value::Number(2.0)), ("size", Value::Number(12.0))] {
        rain.param_mut(param).unwrap().value = value;
    }
    Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(node));
    shot.effects.push(Arc::new(rain));
    // Where it is painted, on average, in the frame's units from its middle.
    let on_frame = |view: &View| {
        let image = render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 47, view);
        let back = view.from_frame.inverse().unwrap();
        let (mut sum, mut at) = (0.0, [0.0, 0.0]);
        for (i, pixel) in image.pixels.iter().enumerate() {
            let alpha = f64::from(pixel[3]);
            let point = back.apply(iskra_drawing::Point::new((i as u32 % view.width) as f64 + 0.5, (i as u32 / view.width) as f64 + 0.5));
            sum += alpha;
            at = [at[0] + alpha * point.x, at[1] + alpha * point.y];
        }
        assert!(sum > 0.0, "the drop is seen");
        [at[0] / sum, at[1] / sum]
    };
    let whole = on_frame(&View::frame(&project.settings, 1.0));
    let smaller = on_frame(&View::frame(&project.settings, 0.37));
    // A window larger than the frame round it, zoomed, as the viewport draws on the card.
    let window = View { width: 500, height: 300, from_frame: Affine::translate(250.0, 150.0).mul(&Affine::scale(0.8)), frame: [320.0, 180.0] };
    let windowed = on_frame(&window);
    for other in [smaller, windowed] {
        assert!((other[0] - whole[0]).abs() < 1.5 && (other[1] - whole[1]).abs() < 1.5, "{whole:?} against {other:?}");
    }
}

#[test]
fn rain_in_two_planes_is_in_front_of_a_layer_between_them_and_behind_it() {
    use iskra_model::{Effect, EffectId, PARTICLES, Source, Value, effect_kind};
    use iskra_render::Cache;
    // The blue field, solid, at depth 0, and rain over all the frame.
    let (mut project, ball, _) = scene();
    let node = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mist = shot.layers[1].id;
    shot.disconnect(Source::Layer(ball));
    shot.disconnect(Source::Layer(mist));
    let mut rain = Effect::new(node, "Rain", effect_kind(PARTICLES).unwrap());
    for (param, value) in [("count", Value::Number(2000.0)), ("far", Value::Number(0.0)), ("colour", Value::Colour([1.0, 1.0, 1.0, 1.0]))] {
        rain.param_mut(param).unwrap().value = value;
    }
    Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(node));
    shot.effects.push(Arc::new(rain));
    let set = |project: &mut iskra_model::Project, param: &str, value: f64| {
        let shot = Arc::make_mut(&mut project.shots[0]);
        Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == node).unwrap()).param_mut(param).unwrap().value = Value::Number(value);
    };
    // How much of the field, from 40 to 280 across and 30 to 150 down, the rain lightens.
    let over_field = |image: &Image| image.pixels.chunks(320).skip(30).take(120).flat_map(|row| &row[40..280]).filter(|pixel| pixel[0] > 60).count();
    set(&mut project, iskra_model::DEPTH, 50.0);
    let front = over_field(&shoot(&project, WorkingSpace::Srgb, 1.0));
    set(&mut project, iskra_model::DEPTH, -50.0);
    assert_eq!(over_field(&shoot(&project, WorkingSpace::Srgb, 1.0)), 0, "all of it behind");
    // Two sheets, at 50 and at -50: the field between them has some of the rain in front.
    set(&mut project, iskra_model::DEPTH, 50.0);
    set(&mut project, "depthRange", 100.0);
    set(&mut project, "planes", 2.0);
    let seen = iskra_eval::evaluate(&project.shots[0], 0);
    let sheets: Vec<(f64, Option<(u32, u32)>)> = seen
        .cards
        .iter()
        .filter_map(|card| match card {
            iskra_eval::Card::Through { depth, plane, .. } => Some((*depth, *plane)),
            _ => None,
        })
        .collect();
    assert_eq!(sheets, [(-50.0, Some((1, 2))), (50.0, Some((0, 2)))], "the far sheet drawn first, then the field, then the near");
    let both = shoot(&project, WorkingSpace::Srgb, 1.0);
    let half = over_field(&both);
    assert!(half > front / 3 && half < front * 2 / 3, "{half} of {front}");
    // Each sheet a picture of its own in the cache too.
    let view = View::frame(&project.settings, 1.0);
    let cache = Cache::default();
    assert_eq!(
        cache.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view),
        render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view)
    );
    assert_eq!(cache.counts().0, 2);
}

/// Put a new effect node of `kind` on the wire that leaves `on` in the scene's shot, with
/// its parameters set as `set` says.
fn put_on(project: &mut iskra_model::Project, kind: &str, on: iskra_model::Source, set: &[(&str, iskra_model::Value)]) -> iskra_model::EffectId {
    use iskra_model::{Effect, EffectId, PICTURE, Source, effect_kind};
    let id = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mut node = Effect::new(id, kind, effect_kind(kind).expect("the kind is known"));
    node.inputs.insert(PICTURE.to_string(), on);
    for (param, value) in set {
        node.param_mut(param).expect("the kind has the parameter").value = value.clone();
    }
    shot.rewire(on, Some(Source::Effect(id)));
    shot.effects.push(Arc::new(node));
    id
}

#[test]
fn a_blur_spreads_the_picture_past_its_edges_and_is_the_same_at_any_size() {
    use iskra_model::{Source, Value};
    // The scene's blue field is 240 by 120 in a frame of 320 by 180: from 40 to 280 across.
    let (plain_project, ..) = scene();
    let plain = shoot(&plain_project, WorkingSpace::Srgb, 1.0);
    let blurred = |radius: f64, kind: &str, more: &[(&str, Value)]| {
        let (mut project, ..) = scene();
        let shown = project.shots[0].display.unwrap();
        let set: Vec<(&str, Value)> = [("radius", Value::Number(radius))].into_iter().chain(more.iter().cloned()).collect();
        put_on(&mut project, kind, shown, &set);
        project
    };
    assert_eq!(shoot(&blurred(0.0, "blur", &[]), WorkingSpace::Srgb, 1.0), plain, "a blur of nothing changes nothing");
    let soft = shoot(&blurred(10.0, "blur", &[]), WorkingSpace::Srgb, 1.0);
    // Well inside the field it is the field still; at its edge it is half there, in the
    // field's colour; a little outside there is something of it, and far outside nothing.
    assert_eq!(soft.pixel(60, 45), [38, 110, 210, 255]);
    let (edge, outside, far) = (soft.pixel(40, 45), soft.pixel(33, 45), soft.pixel(8, 45));
    assert!((110..=145).contains(&edge[3]) && edge[2] > 200 && (1..110).contains(&outside[3]) && far[3] == 0, "{edge:?} {outside:?} {far:?}");
    assert_eq!(plain.pixel(33, 45), [0, 0, 0, 0], "where nothing was");

    // At half the size it is the same picture: the radius is in the frame's pixels.
    let small = shoot(&blurred(10.0, "blur", &[]), WorkingSpace::Srgb, 0.5);
    let (edge_small, outside_small) = (small.pixel(20, 22), small.pixel(16, 22));
    assert!(edge_small[3].abs_diff(edge[3]) < 16 && outside_small[3].abs_diff(soft.pixel(32, 45)[3]) < 16, "{edge_small:?} {outside_small:?}");

    // Seen through a view that is all inside the field, a blur darkens nothing at the view's
    // edges: what is past them is blurred in too.
    let project = blurred(10.0, "blur", &[]);
    let inside = View::around([-60.0, -40.0], [60.0, -25.0], 120, 1.0).unwrap();
    let part = render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &inside);
    assert_eq!((part.pixel(0, 0), part.pixel(inside.width - 1, inside.height - 1)), ([38, 110, 210, 255], [38, 110, 210, 255]));

    // Along a direction: level, it spreads across and not up and down.
    let level = shoot(&blurred(10.0, "directionalBlur", &[("angle", Value::Number(0.0))]), WorkingSpace::Srgb, 1.0);
    assert!(level.pixel(35, 45)[3] > 0 && level.pixel(160, 26)[3] == 0, "{:?} {:?}", level.pixel(35, 45), level.pixel(160, 26));
    let upright = shoot(&blurred(10.0, "directionalBlur", &[("angle", Value::Number(90.0))]), WorkingSpace::Srgb, 1.0);
    assert!(upright.pixel(35, 45)[3] == 0 && upright.pixel(160, 26)[3] > 0, "{:?} {:?}", upright.pixel(35, 45), upright.pixel(160, 26));

    // Round the middle of the frame: the middle stays, and a corner of the field is spread.
    let spun = |kind: &str| {
        let (mut project, ..) = scene();
        let shown = project.shots[0].display.unwrap();
        put_on(&mut project, "radialBlur", shown, &[("kind", Value::Choice(kind.to_string())), ("amount", Value::Number(30.0))]);
        shoot(&project, WorkingSpace::Srgb, 1.0)
    };
    for kind in ["Spin", "Zoom"] {
        let made = spun(kind);
        assert!(made.pixel(45, 35)[3] < 250 && made.pixel(45, 35)[3] > 0, "{kind}: {:?}", made.pixel(45, 35));
        assert_eq!(made.pixel(100, 45), [38, 110, 210, 255], "{kind}: well inside the field, and not far from the middle");
    }
    // A blur on one drawing leaves the others sharp.
    let (mut project, ball, _) = scene();
    put_on(&mut project, "blur", Source::Layer(ball), &[("radius", Value::Number(10.0))]);
    let one = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert_eq!((one.pixel(60, 30), one.pixel(40, 45)), (plain.pixel(60, 30), plain.pixel(40, 45)));
    assert_ne!(one.pixel(200, 78), plain.pixel(200, 78));
}

#[test]
fn a_cutter_cuts_the_picture_by_its_matte() {
    use iskra_model::{MATTE, Source, Value};
    let (plain_project, ..) = scene();
    let plain = shoot(&plain_project, WorkingSpace::Srgb, 1.0);
    let field = Source::Layer(plain_project.shots[0].layers[2].id);
    let cut = |set: &[(&str, Value)], matte: Option<Source>| {
        let (mut project, ball, _) = scene();
        let node = put_on(&mut project, "cutter", Source::Layer(ball), set);
        if let Some(matte) = matte {
            Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap().inputs.insert(MATTE.to_string(), matte);
        }
        shoot(&project, WorkingSpace::Srgb, 1.0)
    };
    // With no matte there is nothing to cut by.
    assert_eq!(cut(&[], None), plain);
    // The field, which is under all of the ball, cuts the ball away, and is itself still seen.
    let gone = cut(&[], Some(field));
    assert_eq!((gone.pixel(200, 78), gone.pixel(60, 30)), ([38, 110, 210, 255], plain.pixel(60, 30)));
    // Inverted, the ball is kept where the field is: all of it.
    assert_eq!(cut(&[("inverted", Value::Switch(true))], Some(field)), plain);
    // The matte shrunk to nothing keeps nothing, inverted; and cuts nothing, as it is.
    let shrunk = [("inverted", Value::Switch(true)), ("grow", Value::Number(-80.0))];
    assert_eq!(cut(&shrunk, Some(field)).pixel(200, 78), [38, 110, 210, 255]);
    assert_eq!(cut(&[("grow", Value::Number(-80.0))], Some(field)), plain);
    // With its edge blurred, the cut is soft: the ball's own matte, grown and blurred, leaves
    // a soft rim of the ball round where it was cut away.
    let (mut project, ball, _) = scene();
    let node = put_on(&mut project, "cutter", Source::Layer(ball), &[("grow", Value::Number(-6.0)), ("blur", Value::Number(4.0))]);
    Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap().inputs.insert(MATTE.to_string(), Source::Layer(ball));
    let rim = shoot(&project, WorkingSpace::Srgb, 1.0);
    // What the shot shows without the ball: the ball cut by all of itself.
    let effect = Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap();
    (effect.param_mut("grow").unwrap().value, effect.param_mut("blur").unwrap().value) = (Value::Number(0.0), Value::Number(0.0));
    let without = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert_eq!(without.pixel(200, 78), [38, 110, 210, 255]);
    // The ball's middle is at (200, 100): there it is cut away, and nearer its edge not all.
    assert_eq!(rim.pixel(200, 100), without.pixel(200, 100), "the middle of the ball is cut away");
    assert_eq!(rim.pixel(60, 30), plain.pixel(60, 30));
    let soft = |x: u32, y: u32| rim.pixel(x, y) != plain.pixel(x, y) && rim.pixel(x, y) != without.pixel(x, y);
    assert!((0..rim.width).any(|x| soft(x, 100)) && (0..rim.height).any(|y| soft(200, y)), "and a rim of it is left, soft");
}

#[test]
fn light_and_shade_are_laid_round_and_inside_a_pictures_shape() {
    use iskra_model::{MATTE, Source, Value};
    let (plain_project, ..) = scene();
    let plain = shoot(&plain_project, WorkingSpace::Srgb, 1.0);
    let field = Source::Layer(plain_project.shots[0].layers[2].id);
    // The ball, through a node of `kind` set as `set` says, with the field as its matte or none.
    let made = |kind: &str, set: &[(&str, Value)], matte: bool| {
        let (mut project, ball, _) = scene();
        let node = put_on(&mut project, kind, Source::Layer(ball), set);
        if matte {
            Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap().inputs.insert(MATTE.to_string(), field);
        }
        shoot(&project, WorkingSpace::Srgb, 1.0)
    };
    let changed = |picture: &Image| -> Vec<(u32, u32)> {
        (0..picture.height).flat_map(|y| (0..picture.width).map(move |x| (x, y))).filter(|(x, y)| picture.pixel(*x, *y) != plain.pixel(*x, *y)).collect()
    };
    let number = Value::Number;

    // A glow: the ball itself is as it was, and round it the field is lighter, towards the
    // glow's colour. Far from the ball nothing changes, and a glow of no strength is none.
    let glow = made("glow", &[("radius", number(10.0))], false);
    let lit = changed(&glow);
    assert!(lit.len() > 200 && glow.pixel(200, 78) == plain.pixel(200, 78) && glow.pixel(60, 30) == plain.pixel(60, 30), "{}", lit.len());
    assert!(lit.iter().all(|(x, y)| glow.pixel(*x, *y)[0] >= plain.pixel(*x, *y)[0]), "it only lightens");
    assert_eq!(made("glow", &[("strength", number(0.0))], false), plain);

    // A drop shadow falls to the side it is moved to, under the ball; one that is not solid
    // at all is not seen.
    let hard = [("offsetX", number(30.0)), ("offsetY", number(0.0)), ("softness", number(0.0)), ("opacity", number(100.0))];
    let shadow = made("dropShadow", &hard, false);
    let dark = changed(&shadow);
    let black = dark.iter().filter(|(x, y)| shadow.pixel(*x, *y) == [0, 0, 0, 255]).count();
    assert!(black > 200 && shadow.pixel(200, 78) == plain.pixel(200, 78), "{black} of {}", dark.len());
    let (left, right) = (dark.iter().map(|(x, _)| *x).min().unwrap(), dark.iter().map(|(x, _)| *x).max().unwrap());
    assert!(left > 160 && right > 250, "it lies to the right: {left} to {right}");
    assert_eq!(made("dropShadow", &[("opacity", number(0.0))], false), plain);

    // A tone with no matte shades a rim inside the ball's edge: its middle is as it was.
    let rim = made("tone", &[("width", number(5.0)), ("softness", number(0.0)), ("amount", number(100.0))], false);
    let shaded = changed(&rim);
    assert!(shaded.len() > 100 && rim.pixel(200, 100) == plain.pixel(200, 100) && rim.pixel(60, 30) == plain.pixel(60, 30), "{}", shaded.len());
    // With a matte it shades where the matte is, inside the ball only: the field, which is
    // under all of the ball, shades all of it and nothing else.
    let whole = [("width", number(0.0)), ("softness", number(0.0)), ("amount", number(100.0))];
    let toned = made("tone", &whole, true);
    assert_eq!((toned.pixel(200, 78), toned.pixel(60, 30)), ([26, 26, 77, 255], plain.pixel(60, 30)));
    let lit = made("highlight", &whole, true);
    assert_eq!((lit.pixel(200, 78), lit.pixel(60, 30)), ([255, 255, 255, 255], plain.pixel(60, 30)));
    // Half the amount is half way from the ball's red to the tone.
    let half = made("tone", &[("width", number(0.0)), ("softness", number(0.0)), ("amount", number(50.0))], true);
    assert_eq!(half.pixel(200, 78), [123, 38, 62, 255]);
    assert_eq!(made("highlight", &[("amount", number(0.0))], true), plain);
}

#[test]
fn a_colour_correction_changes_the_colours_and_leaves_the_shape() {
    use iskra_model::{Source, Value};
    let (plain_project, ..) = scene();
    let plain = shoot(&plain_project, WorkingSpace::Srgb, 1.0);
    // The ball, which is red (220, 50, 47), through a correction set as `set` says.
    let made = |set: &[(&str, Value)], space: WorkingSpace| {
        let (mut project, ball, _) = scene();
        put_on(&mut project, "colourCorrection", Source::Layer(ball), set);
        shoot(&project, space, 1.0)
    };
    let ball = |set: &[(&str, Value)]| {
        let picture = made(set, WorkingSpace::Srgb);
        assert_eq!((picture.pixel(60, 30), picture.pixel(2, 2)), (plain.pixel(60, 30), plain.pixel(2, 2)), "only the ball is changed");
        picture.pixel(200, 78)
    };
    let number = Value::Number;
    // As it starts, it changes nothing.
    assert_eq!(made(&[], WorkingSpace::Srgb), plain);
    // Levels: what is darker than the black is black, and the rest is spread up to the white;
    // the middle lifts what is between.
    assert_eq!(ball(&[("black", number(50.0))]), [185, 0, 0, 255]);
    assert_eq!(ball(&[("white", number(50.0))]), [255, 100, 94, 255]);
    assert_eq!(ball(&[("middle", number(2.0))]), [237, 113, 109, 255]);
    // Curves: the master turned over makes a negative, the red one alone changes the red,
    // and a straight one drawn out point by point changes nothing.
    let curves =
        |master: &[[f64; 2]], red: &[[f64; 2]]| Value::Curves(iskra_model::Curves { master: master.to_vec(), red: red.to_vec(), ..Default::default() });
    assert_eq!(ball(&[("curves", curves(&[[0.0, 1.0], [1.0, 0.0]], &[]))]), [35, 205, 208, 255]);
    assert_eq!(ball(&[("curves", curves(&[], &[[0.0, 0.0], [1.0, 0.5]]))]), [110, 50, 47, 255]);
    assert_eq!(ball(&[("curves", curves(&[[0.0, 0.0], [0.25, 0.25], [1.0, 1.0]], &[]))]), plain.pixel(200, 78));
    // A third of the way round, red is green; with no saturation it is the grey as light as it.
    assert_eq!(ball(&[("hue", number(120.0))]), [47, 220, 50, 255]);
    assert_eq!(ball(&[("saturation", number(-100.0))]), [86, 86, 86, 255]);
    assert_eq!((ball(&[("lightness", number(100.0))]), ball(&[("lightness", number(-100.0))])), ([255, 255, 255, 255], [0, 0, 0, 255]));
    // Through blue glass only the blue is left, and half the amount takes half of the rest.
    let blue = Value::Colour([0.0, 0.0, 1.0, 1.0]);
    assert_eq!(ball(&[("tint", blue.clone())]), [0, 0, 47, 255]);
    assert_eq!(ball(&[("tint", blue.clone()), ("tintAmount", number(50.0))]), [110, 25, 47, 255]);
    assert_eq!(ball(&[("tint", blue), ("tintAmount", number(0.0))]), plain.pixel(200, 78));
    // In linear light the numbers do the same, to a level.
    let grey = made(&[("saturation", number(-100.0))], WorkingSpace::Linear).pixel(200, 78);
    assert!(grey.iter().zip([86u8, 86, 86, 255]).all(|(a, b)| a.abs_diff(b) <= 1), "{grey:?}");
    let same = made(&[], WorkingSpace::Linear);
    let linear = shoot(&plain_project, WorkingSpace::Linear, 1.0);
    let apart = (0..same.height).flat_map(|y| (0..same.width).map(move |x| (x, y)));
    assert!(apart.clone().all(|(x, y)| same.pixel(x, y).iter().zip(linear.pixel(x, y)).all(|(a, b)| a.abs_diff(b) <= 1)));
}

#[test]
fn a_style_node_shows_its_picture_in_another_style_as_much_as_its_amount_and_matte_say() {
    use iskra_model::{MATTE, STYLE, STYLE_PALETTE, SetId, Source, Style, StyleId, SwatchId, Value};
    use iskra_render::Cache;
    // A Night style of the scene's palettes: the ball's red darker in the first, and the
    // field's blue in a Field palette of its own.
    let night = |project: &mut iskra_model::Project| {
        let palette = Arc::make_mut(&mut project.palette);
        let field = palette.new_set("Field").unwrap();
        palette.swatches.iter_mut().find(|swatch| swatch.id == SwatchId(5)).unwrap().set = field;
        let style = |id, swatch, colour, set| Style {
            id: StyleId(id),
            name: "Night".to_string(),
            colors: [(SwatchId(swatch), colour)].into(),
            set,
            kinds: Default::default(),
        };
        palette.styles.extend([style(1, 3, [90, 20, 20, 255], SetId::default()), style(2, 5, [10, 30, 60, 255], field)]);
    };
    let (mut plain_project, ..) = scene();
    night(&mut plain_project);
    let plain = shoot(&plain_project, WorkingSpace::Srgb, 1.0);
    // On the ball's wire or on the whole shot's, in `style` of `palette`, with the ball as its
    // matte or none.
    let styled = |on_ball: bool, palette: &str, style: &str, amount: f64, by_ball: bool, shown: &[StyleId]| {
        let (mut project, ball, _) = scene();
        night(&mut project);
        Arc::make_mut(&mut project.shots[0]).styles = shown.to_vec();
        let on = if on_ball { Source::Layer(ball) } else { project.shots[0].display.unwrap() };
        let set = [(STYLE_PALETTE, Value::Text(palette.to_string())), (STYLE, Value::Text(style.to_string())), ("amount", Value::Number(amount))];
        let node = put_on(&mut project, STYLE, on, &set);
        if by_ball {
            Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap().inputs.insert(MATTE.to_string(), Source::Layer(ball));
        }
        let image = shoot(&project, WorkingSpace::Srgb, 1.0);
        let view = View::frame(&project.settings, 1.0);
        assert_eq!(Cache::default().render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view), image, "the same through the cache");
        image
    };
    // On the ball, in Night: the ball is, and the field is not.
    let ball = styled(true, "", "Night", 100.0, false, &[]);
    assert_eq!((ball.pixel(200, 78), ball.pixel(60, 30)), ([90, 20, 20, 255], plain.pixel(60, 30)));
    // At 0 nothing of it is seen, and half way it is half way.
    assert_eq!(styled(true, "", "Night", 0.0, false, &[]), plain);
    let half = styled(true, "", "Night", 50.0, false, &[]).pixel(200, 78);
    assert!(half[0] < 220 && half[0] > 90 && half[1] < 50 && half[1] > 20, "{half:?}");
    // A style no palette has leaves the picture as the shot shows it.
    assert_eq!(styled(true, "", "Day", 100.0, false, &[]), plain);
    // The shot shown in Night, and the ball in the palettes' own colours.
    let own = styled(true, "", "", 100.0, false, &[StyleId(1), StyleId(2)]);
    assert_eq!((own.pixel(200, 78), own.pixel(60, 30)), ([220, 50, 47, 255], [10, 30, 60, 255]));
    // Over the whole shot, in the Night of every palette, of one, or of one there is none of.
    let every = styled(false, "", "Night", 100.0, false, &[]);
    assert_eq!((every.pixel(200, 78), every.pixel(60, 30)), ([90, 20, 20, 255], [10, 30, 60, 255]));
    let field = styled(false, "Field", "Night", 100.0, false, &[]);
    assert_eq!((field.pixel(200, 78), field.pixel(60, 30)), (plain.pixel(200, 78), [10, 30, 60, 255]));
    let first = styled(false, "Default", "Night", 100.0, false, &[]);
    assert_eq!((first.pixel(200, 78), first.pixel(60, 30)), ([90, 20, 20, 255], plain.pixel(60, 30)));
    assert_eq!(styled(false, "Sky", "Night", 100.0, false, &[]), plain);
    // In the shot shown in Night, the Field palette alone back in its own colours.
    let own_field = styled(false, "Field", "", 100.0, false, &[StyleId(1), StyleId(2)]);
    assert_eq!((own_field.pixel(200, 78), own_field.pixel(60, 30)), ([90, 20, 20, 255], plain.pixel(60, 30)));
    // Over the whole shot, by the ball as its matte: Night where the ball is, and the field
    // as it was round it.
    let shaded = styled(false, "", "Night", 100.0, true, &[]);
    assert_eq!((shaded.pixel(200, 78), shaded.pixel(60, 30)), ([90, 20, 20, 255], plain.pixel(60, 30)));
    golden("style", &shaded);
}

#[test]
fn what_went_through_an_effect_is_made_once_while_it_stays_the_same() {
    use iskra_model::{Source, Value};
    use iskra_render::Cache;
    // The field through a blur, under a ball that its peg moves from frame 0 to 10.
    let (mut project, _, peg) = scene();
    let field = Source::Layer(project.shots[0].layers[2].id);
    let blur = put_on(&mut project, "blur", field, &[("radius", Value::Number(6.0))]);
    let shot = Arc::make_mut(&mut project.shots[0]);
    let bounce = Arc::make_mut(shot.pegs.iter_mut().find(|held| held.id == peg).unwrap());
    bounce.animation.position.set(0, [40.0, 10.0]);
    bounce.animation.position.set(10, [-60.0, 10.0]);
    let view = View::frame(&project.settings, 1.0);
    let plain = |project: &iskra_model::Project, frame: u32| render(&project.shots[0], &project.palette, WorkingSpace::Srgb, frame, &view);
    let cache = Cache::default();
    let kept = |project: &iskra_model::Project, frame: u32| cache.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, frame, &view);
    // The ball moves and the field does not: every frame is as it is without the cache, and
    // the blur is made on the first only.
    assert_ne!(plain(&project, 0), plain(&project, 5));
    for frame in [0, 5, 10, 5] {
        assert_eq!(kept(&project, frame), plain(&project, frame), "frame {frame}");
    }
    assert_eq!(cache.counts(), (1, 3));
    // An edit elsewhere in the shot leaves it found; another radius, size or colour does not.
    Arc::make_mut(Arc::make_mut(&mut project.shots[0]).pegs.iter_mut().find(|held| held.id == peg).unwrap()).animation.position.set(10, [0.0, 0.0]);
    assert_eq!((kept(&project, 10), cache.counts()), (plain(&project, 10), (1, 4)));
    Arc::make_mut(&mut project.shots[0]).effect_mut(blur).unwrap().param_mut("radius").unwrap().value = Value::Number(2.0);
    assert_eq!((kept(&project, 0), cache.counts()), (plain(&project, 0), (2, 4)));
    let half = View::frame(&project.settings, 0.5);
    let small = cache.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &half);
    assert_eq!((small, cache.counts()), (render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &half), (3, 4)));
    Arc::make_mut(&mut project.palette).swatches.iter_mut().find(|swatch| swatch.id == iskra_model::SwatchId(5)).unwrap().color = [10, 200, 30, 255];
    assert_eq!((kept(&project, 0), cache.counts()), (plain(&project, 0), (4, 4)));
    // The colours as they were are still known, with what was made in them.
    Arc::make_mut(&mut project.palette).swatches.iter_mut().find(|swatch| swatch.id == iskra_model::SwatchId(5)).unwrap().color = [38, 110, 210, 255];
    assert_eq!((kept(&project, 0), cache.counts()), (plain(&project, 0), (4, 5)));
    // A radius that is keyed is another picture on each frame, and each is found again.
    let radius = Arc::make_mut(&mut project.shots[0]).effect_mut(blur).unwrap().param_mut("radius").unwrap();
    radius.keys.set(0, 2.0);
    radius.keys.set(10, 12.0);
    for frame in [0, 5, 5, 0] {
        assert_eq!(kept(&project, frame), plain(&project, frame), "frame {frame}");
    }
    assert_eq!(cache.counts(), (5, 8), "frame 0 was kept from before the keys, which say the same there");
    // An effect on what another gave: the first is found when only the second is changed.
    let fade = put_on(&mut project, "transparency", Source::Effect(blur), &[]);
    assert_eq!((kept(&project, 0), cache.counts()), (plain(&project, 0), (6, 9)));
    Arc::make_mut(&mut project.shots[0]).effect_mut(fade).unwrap().param_mut("amount").unwrap().value = Value::Number(20.0);
    assert_eq!((kept(&project, 0), cache.counts()), (plain(&project, 0), (7, 10)));
    // No more is kept than there is room for: with room for one picture, only the last.
    let tight = Cache::new(320 * 180 * 16);
    for frame in [0, 5, 0] {
        tight.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, frame, &view);
    }
    assert_eq!(tight.counts().1, 0);
}

#[test]
fn a_composite_that_stacks_by_its_ports_draws_them_whatever_their_depths() {
    use iskra_model::Stacking;
    // The ball is on top of the stack, and its peg has moved it back behind the field.
    let (mut project, _, peg) = scene();
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(shot.pegs.iter_mut().find(|held| held.id == peg).unwrap()).transform.depth = -1.0;
    let (field, red) = ([38, 110, 210, 255], |pixel: [u8; 4]| pixel[0] > 200 && pixel[2] < 80);
    let centre = |project: &iskra_model::Project| {
        // Farther, the ball is smaller: it is still over the middle of where it was.
        let picture = shoot(project, WorkingSpace::Srgb, 1.0);
        (0..picture.width).flat_map(|x| (0..picture.height).map(move |y| (x, y))).filter(|(x, y)| red(picture.pixel(*x, *y))).count()
    };
    let by_depth = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert_eq!((by_depth.pixel(60, 30), centre(&project) < 200), (field, true), "by depth the field covers the ball");
    let set = |project: &mut iskra_model::Project, stacking: Stacking, right_first: bool| {
        let composite = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).composites[0]);
        (composite.stacking, composite.right_first) = (stacking, right_first);
    };
    set(&mut project, Stacking::Ports, false);
    assert!(centre(&project) > 1000, "by its ports the ball is on top, as it is stacked: {}", centre(&project));
    set(&mut project, Stacking::Ports, true);
    assert!(centre(&project) < 200, "and with the right port on top it is at the bottom");
    set(&mut project, Stacking::DepthFlat, false);
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), by_depth, "flat, by depth, a composite alone draws as by depth");
}

#[test]
fn what_is_farther_is_under_what_is_nearer() {
    let (mut project, _, peg) = scene();
    Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]).transform.depth = -1.0;
    assert_eq!(project.shots[0].pegs[0].id, peg);
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("depth", &image);
    assert_eq!(image.pixel(200, 78), [38, 110, 210, 255], "the field is over the blob now");
    assert_eq!(image.pixel(275, 60)[3], 255, "which still shows beside the field");
}

#[test]
fn the_camera_sees_what_is_near_larger_and_moving_faster() {
    let (mut project, ..) = scene();
    let rig = PegId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let distance = shot.camera.distance;
    // The ball is brought a third of the way to the camera; the camera is panned and rolled.
    Arc::make_mut(&mut shot.pegs[0]).transform.depth = distance / 3.0;
    let moved = Transform { position: [-30.0, 10.0], rotation: 8.0, ..Transform::default() };
    shot.pegs.push(Arc::new(Peg {
        id: rig,
        name: "Camera-P".to_string(),
        parent: None,
        transform: moved,
        animation: Animation::default(),
        template: None,
        follows: None,
        inherit: Default::default(),
        matches: Default::default(),
    }));
    shot.camera.parent = Some(rig);
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("camera", &image);
    assert_eq!((image.pixel(250, 90), image.pixel(100, 60)), ([220, 50, 47, 255], [38, 110, 210, 255]), "the ball large on the right, the field behind");
    // Behind the camera it is not seen at all.
    Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]).transform.depth = distance * 2.0;
    assert!(shoot(&project, WorkingSpace::Srgb, 1.0).pixels.iter().all(|pixel| pixel != &[220, 50, 47, 255]));
}

#[test]
fn a_tilted_layer_is_seen_in_perspective() {
    let (mut project, ..) = scene();
    // The ball's card leans back and swings aside, about its middle.
    Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]).transform.tilt = [55.0, 25.0];
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("tilt", &image);
    // Flat, the red square is 90 by 45 of the picture's pixels, turned; leant back it is less
    // high, and its far side is shorter than its near one.
    let red = |image: &Image| image.pixels.iter().filter(|pixel| pixel[0] > 180 && pixel[1] < 90 && pixel[3] == 255).count();
    let (flat, ..) = scene();
    let (tilted, upright) = (red(&image), red(&shoot(&flat, WorkingSpace::Srgb, 1.0)));
    assert!(tilted * 10 > upright * 3 && tilted * 10 < upright * 8, "{tilted} red pixels against {upright} flat");
    // The same picture at twice the size, as for any other: but for the soft edge of the line
    // over the red, which takes a little more of it at the smaller size.
    let twice = shoot(&project, WorkingSpace::Srgb, 2.0);
    assert!((red(&twice) as f64 / 4.0 - tilted as f64).abs() < tilted as f64 * 0.04, "{} at twice against {tilted}", red(&twice));

    // A card leant right over, so large that it passes the camera, is seen as far as it is in
    // front of it, and the picture is made all the same.
    let peg = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]);
    (peg.transform.tilt, peg.transform.scale, peg.transform.rotation) = ([80.0, 0.0], [6.0, 6.0], 0.0);
    let floor = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert!(red(&floor) > 500 && floor.pixels.iter().all(|pixel| pixel[3] == 0 || pixel[3] == 255 || pixel[3] > 0));
}

#[test]
fn a_bone_bends_the_drawing_it_is_given() {
    use iskra_model::{Bone, Deformer, DeformerId};
    let (mut project, ..) = scene();
    // The mist's band, 300 of the shot's pixels long, gets two bones along it, from its left
    // end; the ball is put out of the way.
    let id = DeformerId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(&mut shot.pegs[0]).transform.position = [1000.0, 0.0];
    let bones = vec![Bone::at_rest(0.0, 150.0), Bone::at_rest(0.0, 150.0)];
    shot.deformers.push(Arc::new(Deformer::new(id, "Band", [-150.0, 0.0], bones)));
    Arc::make_mut(&mut shot.layers[1]).deformer = Some(id);
    assert_eq!(shot.layers[1].name, "Mist");
    let at_rest = shoot(&project, WorkingSpace::Srgb, 1.0);

    // Moved as a whole by its deformer, the band is the picture a peg would make of it: the
    // mesh shows the drawing as the plain way does.
    let shot = Arc::make_mut(&mut project.shots[0]);
    shot.deformer_mut(id).unwrap().offset = [10.0, 5.0];
    let moved = shoot(&project, WorkingSpace::Srgb, 1.0);
    let shot = Arc::make_mut(&mut project.shots[0]);
    shot.deformer_mut(id).unwrap().offset = [0.0; 2];
    let peg = PegId(project.new_id());
    let transform = Transform { position: [10.0, 5.0], ..Transform::default() };
    let shot = Arc::make_mut(&mut project.shots[0]);
    shot.pegs.push(Arc::new(Peg {
        id: peg,
        name: "Shift".to_string(),
        parent: None,
        transform,
        animation: Animation::default(),
        template: None,
        follows: None,
        inherit: Default::default(),
        matches: Default::default(),
    }));
    Arc::make_mut(&mut shot.layers[1]).parent = Some(peg);
    let by_peg = shoot(&project, WorkingSpace::Srgb, 1.0);
    let apart = moved.pixels.iter().zip(&by_peg.pixels).filter(|(a, b)| a.iter().zip(*b).any(|(a, b)| a.abs_diff(*b) > 1)).count();
    assert_eq!(apart, 0, "pixels of the moved band that a peg would have made otherwise");
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(&mut shot.layers[1]).parent = None;

    // The second bone turned down by 60°: the band's right half swings about its middle, and
    // its left half lies as it did.
    Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap().bones[1].turn = 60.0;
    let bent = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("bone", &bent);
    let misty = |image: &Image, x: u32, y: u32| image.pixel(x, y) != [38, 110, 210, 255] && image.pixel(x, y)[3] == 255;
    assert!(misty(&at_rest, 270, 90) && !misty(&bent, 270, 90), "the right end has left where it was");
    assert!(misty(&bent, 60, 90) && misty(&bent, 120, 90), "the left half is where it was");
    // Along the turned bone, 40 pixels from the middle at 60°.
    assert!(misty(&bent, 160 + 20, 90 + 35) && !misty(&at_rest, 160 + 20, 90 + 35));
    // About as much mist as before: less by the fold at the joint, and by the end that has
    // swung out of the picture.
    let mist = |image: &Image| image.pixels.iter().filter(|pixel| pixel[3] > 0 && pixel[0] > 100).count() as f64;
    assert!((mist(&bent) / mist(&at_rest) - 1.0).abs() < 0.25, "{} against {}", mist(&bent), mist(&at_rest));

    // A sharper bend, 110°, which pinches the band at the joint; with a round joint, within 40 of
    // it and fading out to 70, it goes round the joint at its full width, and loses less of itself.
    let chain = Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap();
    chain.bones[1].turn = 110.0;
    let pinched = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("bone-sharp", &pinched);
    let chain = Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap();
    (chain.bones[1].round, chain.bones[1].fade) = (40.0, 70.0);
    let round = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("bone-round", &round);
    assert!(mist(&round) > mist(&pinched), "{} against {}", mist(&round), mist(&pinched));
    // With the joint's pull, what is within the circles is drawn towards the middle of the
    // bend: more of the band turns by half the angle.
    Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap().bones[1].pull = 0.6;
    let pulled = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("bone-pull", &pulled);
    assert!(pulled.pixels != round.pixels);
    Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap().bones[1].pull = 0.0;
    // The first bone half as long again: the left half of the band is stretched along it,
    // and the joint with the turned half is further right.
    let chain = Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap();
    (chain.bones[0].stretch, chain.bones[1].turn) = (1.5, 60.0);
    let long = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("bone-long", &long);
    assert!(misty(&long, 160 + 75 + 20, 90 + 35) && !misty(&long, 160 + 20, 90 + 60), "the turned half hangs from the joint, which has moved");
}

#[test]
fn a_curve_bends_the_drawing_along_its_line() {
    use iskra_model::{Deformer, DeformerId};
    let (mut project, ..) = scene();
    // The mist's band gets a curve of three points along it; the ball is put out of the way.
    let id = DeformerId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(&mut shot.pegs[0]).transform.position = [1000.0, 0.0];
    shot.deformers.push(Arc::new(Deformer::curve(id, "Band", &[[-150.0, 0.0], [0.0, 0.0], [150.0, 0.0]])));
    Arc::make_mut(&mut shot.layers[1]).deformer = Some(id);
    let at_rest = shoot(&project, WorkingSpace::Srgb, 1.0);
    // Its middle lifted and its end let down: the band follows the line in one smooth sweep,
    // as wide as it was.
    let curve = Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap();
    curve.pose_knot(1, 0, [0.0, -30.0]);
    curve.pose_knot(2, 0, [110.0, 40.0]);
    let swept = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("curve", &swept);
    let misty = |image: &Image, x: u32, y: u32| image.pixel(x, y) != [38, 110, 210, 255] && image.pixel(x, y)[3] == 255;
    assert!(misty(&at_rest, 160, 90) && !misty(&swept, 160, 90) && misty(&swept, 160, 60), "the middle has gone up with its point");
    let mist = |image: &Image| image.pixels.iter().filter(|pixel| pixel[3] > 0 && pixel[0] > 100).count() as f64;
    assert!((mist(&swept) / mist(&at_rest) - 1.0).abs() < 0.3, "{} against {}", mist(&swept), mist(&at_rest));
}

#[test]
fn an_envelope_bulges_the_shape_it_holds() {
    use iskra_model::{Deformer, DeformerId};
    let (mut project, ..) = scene();
    // The mist's band gets an envelope round it, of four points; the ball is out of the way.
    let id = DeformerId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(&mut shot.pegs[0]).transform.position = [1000.0, 0.0];
    shot.deformers.push(Arc::new(Deformer::envelope(id, "Band", &[[0.0, -40.0], [170.0, 0.0], [0.0, 40.0], [-170.0, 0.0]])));
    Arc::make_mut(&mut shot.layers[1]).deformer = Some(id);
    let at_rest = shoot(&project, WorkingSpace::Srgb, 1.0);
    // Its top point lifted: the band swells upwards in its middle, and its ends stay.
    Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap().pose_knot(0, 0, [0.0, -75.0]);
    let bulged = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("envelope", &bulged);
    let misty = |image: &Image, x: u32, y: u32| image.pixel(x, y) != [38, 110, 210, 255] && image.pixel(x, y)[3] == 255;
    assert!(!misty(&at_rest, 160, 72) && misty(&bulged, 160, 72) && misty(&bulged, 160, 88), "the middle has swollen upwards");
    let mist = |image: &Image| image.pixels.iter().filter(|pixel| pixel[3] > 0 && pixel[0] > 100).count() as f64;
    assert!(mist(&bulged) > mist(&at_rest) * 1.1, "{} against {}", mist(&bulged), mist(&at_rest));
}

#[test]
fn in_linear_light_a_half_covered_pixel_is_lighter() {
    let (project, ..) = scene();
    let image = shoot(&project, WorkingSpace::Linear, 1.0);
    golden("linear", &image);
    let (blended, plain) = (image.pixel(60, 90), shoot(&project, WorkingSpace::Srgb, 1.0).pixel(60, 90));
    assert!(blended[0] > plain[0] + 20 && blended[3] == 255, "{blended:?} {plain:?}");
    assert_eq!(image.pixel(60, 30), [38, 110, 210, 255], "what is not blended is the colour that was picked");
}

#[test]
fn a_picture_at_another_size_is_the_same_picture() {
    let (project, ..) = scene();
    let (small, large) = (shoot(&project, WorkingSpace::Srgb, 0.25), shoot(&project, WorkingSpace::Srgb, 2.0));
    golden("quarter", &small);
    assert_eq!(((small.width, small.height), (large.width, large.height)), ((80, 45), (640, 360)));
    assert_eq!((small.pixel(15, 9), large.pixel(120, 64), large.pixel(400, 156)), ([38, 110, 210, 255], [38, 110, 210, 255], [220, 50, 47, 255]));
    // The black stroke is 7 scene units wide: at a quarter of the size it is still there.
    let darkest = small.pixels.iter().map(|[r, g, b, _]| u32::from(*r) + u32::from(*g) + u32::from(*b)).min();
    assert!(darkest.is_some_and(|sum| sum < 120), "{darkest:?}");
}

#[test]
fn a_drawing_shown_pixel_for_pixel_comes_out_as_it_was_painted() {
    let (mut project, ball, _) = scene();
    let shot = Arc::make_mut(&mut project.shots[0]);
    shot.pegs.clear();
    Arc::make_mut(&mut shot.layers[0]).parent = None;
    assert_eq!(shot.layers[0].id, ball);
    // At twice the shot's size, a drawing pixel is a pixel of the picture.
    let image = shoot(&project, WorkingSpace::Srgb, 2.0);
    let drawing = &project.layer(ball).unwrap().drawings[0].drawing;
    let around = Rect::new(-100, -70, 100, 70);
    let painted = drawing.to_rgba8(around, |swatch| project.palette.color(swatch));
    for (index, pixel) in painted.iter().enumerate().filter(|(_, pixel)| pixel[3] == 255) {
        let (x, y) = (index as u32 % around.width(), index as u32 / around.width());
        assert_eq!(image.pixel(320 - 100 + x, 180 - 70 + y), *pixel, "at ({x}, {y})");
    }
    // Nothing on the frame, or nothing seen, is an empty picture.
    assert!(render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 99, &View::frame(&project.settings, 1.0)).pixels.iter().all(|pixel| pixel[3] == 0));
    assert!(iskra_render::painted(&project.shots[0], 99).is_none());
    let (low, high) = iskra_render::painted(&project.shots[0], 0).unwrap();
    assert_eq!((low, high), ([-150.0, -60.0], [150.0, 60.0]), "the field and the band, in scene units");
    assert!(View::around(low, high, 100, 4.0).is_some_and(|view| (view.width, view.height) == (100, 40)) && View::around(low, low, 100, 4.0).is_none());
}

#[test]
fn an_art_layers_output_gives_that_art_layer_alone() {
    use iskra_model::{ArtLayerKind, Source};
    let (mut project, ball, _) = scene();
    let plain = shoot(&project, WorkingSpace::Srgb, 1.0);
    let wire = |project: &mut iskra_model::Project, to: Vec<Source>| {
        let shot = Arc::make_mut(&mut project.shots[0]);
        let stack = Arc::make_mut(&mut shot.composites[0]);
        let at = stack.inputs.iter().position(|input| input.layer() == Some(ball)).unwrap();
        let after = stack.inputs.iter().rposition(|input| input.layer() == Some(ball)).unwrap();
        stack.inputs.splice(at..=after, to);
    };
    // The ball is all line art: its line art's output is all of it, its colour art's nothing.
    wire(&mut project, vec![Source::Art(ball, ArtLayerKind::LineArt)]);
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), plain);
    wire(&mut project, vec![Source::Art(ball, ArtLayerKind::ColourArt)]);
    let without = shoot(&project, WorkingSpace::Srgb, 1.0);
    assert_eq!(without.pixel(200, 78), [38, 110, 210, 255], "the field where the ball was");
    // Painted on the colour art instead, it is the colour art's output that gives it.
    let shot = Arc::make_mut(&mut project.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
    let lines = std::mem::replace(&mut drawing.art_layers[1].content, iskra_model::ArtContent::Indexed(Default::default()));
    drawing.art_layers[0].content = lines;
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), plain);
    wire(&mut project, vec![Source::Art(ball, ArtLayerKind::LineArt)]);
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), without);
    // From both, the lines over the fills, it is as its flat output.
    wire(&mut project, vec![Source::Art(ball, ArtLayerKind::LineArt), Source::Art(ball, ArtLayerKind::ColourArt)]);
    assert_eq!(shoot(&project, WorkingSpace::Srgb, 1.0), plain);
}

/// A shot of a burst thrown down from the frame's middle, and a layer, not itself seen, of a
/// bar across the frame from 40 to 60 below its middle wired into its collider port, or
/// `None` without one. The burst and the layer, by their ids.
fn burst_over_a_floor(collider: bool) -> (iskra_model::Project, iskra_model::EffectId, iskra_model::LayerId) {
    use iskra_model::{ArtContent, COLLIDER, DrawingId, Effect, EffectId, PARTICLES, PoolDrawing, Source, SwatchId, Value, effect_kind};
    let (mut project, ball, _) = scene();
    let (bar, node) = (DrawingId(project.new_id()), EffectId(project.new_id()));
    let shot = Arc::make_mut(&mut project.shots[0]);
    let (mist, field) = (shot.layers[1].id, shot.layers[2].id);
    for layer in [ball, mist, field] {
        shot.disconnect(Source::Layer(layer));
    }
    // At twice the shot's pixels.
    let mut floor = iskra_model::Drawing::default();
    if let ArtContent::Indexed(surface) = &mut floor.art_layers[1].content {
        let rect = Rect::new(-400, 80, 400, 120);
        surface.paint(rect, &vec![1.0; (rect.width() * rect.height()) as usize], SwatchId(3));
    }
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == field).unwrap());
    layer.drawings = vec![PoolDrawing { id: bar, name: "1".to_string(), drawing: Arc::new(floor) }];
    layer.exposure = Default::default();
    layer.exposure.set(0, 48, Some(bar));
    let mut burst = Effect::new(node, "Burst", effect_kind(PARTICLES).unwrap());
    for (param, value) in [
        ("kind", Value::Choice("Burst".to_string())),
        ("count", Value::Number(200.0)),
        ("angle", Value::Number(90.0)),
        ("spread", Value::Number(60.0)),
        ("life", Value::Number(80.0)),
        ("far", Value::Number(0.0)),
        ("colour", Value::Colour([1.0, 1.0, 1.0, 1.0])),
    ] {
        burst.param_mut(param).unwrap().value = value;
    }
    if collider {
        burst.inputs.insert(COLLIDER.to_string(), Source::Layer(field));
    }
    Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(node));
    shot.effects.push(Arc::new(burst));
    (project, node, field)
}

/// The shot on `frame`, at the frame's size.
fn shot_on(project: &iskra_model::Project, frame: u32) -> Image {
    iskra_render::render(&project.shots[0], &project.palette, WorkingSpace::Srgb, frame, &View::frame(&project.settings, 1.0))
}

/// How many pixels of a picture of the whole frame are painted from row `low` to before `high`.
fn painted_rows(image: &Image, low: u32, high: u32) -> usize {
    image.pixels.chunks(image.width as usize).skip(low as usize).take((high - low) as usize).flatten().filter(|pixel| pixel[3] > 40).count()
}

#[test]
fn a_burst_bounces_off_where_its_collider_is_painted() {
    // The bar is from row 130 to 150. Without the collider the sparks are falling through it
    // on frame 12; with it, none is below it, and they have bounced back up above it, to come
    // to lie on it.
    let (free, ..) = burst_over_a_floor(false);
    let (bounced, ..) = burst_over_a_floor(true);
    let (through, kept) = (shot_on(&free, 12), shot_on(&bounced, 12));
    assert!(painted_rows(&through, 152, 180) > 50, "they fall through: {}", painted_rows(&through, 152, 180));
    assert_eq!(painted_rows(&kept, 152, 180), 0, "none below the bar");
    assert!(painted_rows(&kept, 90, 120) > 50, "they bounce up: {}", painted_rows(&kept, 90, 120));
    let lying = shot_on(&bounced, 30);
    assert_eq!(painted_rows(&lying, 0, 115) + painted_rows(&lying, 152, 180), 0, "they lie on it");
    assert!(painted_rows(&lying, 115, 132) > 50, "{}", painted_rows(&lying, 115, 132));
    // Before any reaches it, the two are alike.
    let (early_free, early_kept) = (shot_on(&free, 3), shot_on(&bounced, 3));
    let apart = early_free.pixels.iter().zip(&early_kept.pixels).map(|(a, b)| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap()).max().unwrap();
    assert!(apart <= 1, "{apart}");
}

#[test]
fn a_stepped_burst_is_the_same_however_its_frame_is_reached_and_follows_its_collider() {
    use iskra_model::Value;
    let (project, node, field) = burst_over_a_floor(true);
    // Played from the start, frame by frame.
    let played: Vec<Image> = (0..=30).map(|frame| shot_on(&project, frame)).collect();
    assert_eq!(shot_on(&project, 30).pixels, played[30].pixels, "found again");
    assert_eq!(shot_on(&project, 12).pixels, played[12].pixels, "back to an earlier frame");
    // The same shot made anew, its drawings no longer the very ones, jumped to at once.
    let mut anew = project.clone();
    let shot = Arc::make_mut(&mut anew.shots[0]);
    for layer in &mut shot.layers {
        for entry in &mut Arc::make_mut(layer).drawings {
            entry.drawing = Arc::new((*entry.drawing).clone());
        }
    }
    assert_eq!(shot_on(&anew, 30).pixels, played[30].pixels, "jumped to");
    // The bar gone from frame 10 on: the sparks that bounced before fall through after, and
    // the frames before are as they were.
    let mut gone = project.clone();
    let shot = Arc::make_mut(&mut gone.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == field).unwrap());
    let bar = layer.exposure.at(0);
    layer.exposure = Default::default();
    layer.exposure.set(0, 10, bar);
    assert_eq!(shot_on(&gone, 8).pixels, played[8].pixels);
    assert!(painted_rows(&shot_on(&gone, 24), 145, 180) > 20, "they fall through");
    assert_eq!(painted_rows(&shot_on(&project, 24), 145, 180), 0, "and not where it stays");
    // Not bouncing, they stay low.
    let mut dull = project.clone();
    let shot = Arc::make_mut(&mut dull.shots[0]);
    Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == node).unwrap()).param_mut("bounce").unwrap().value = Value::Number(0.0);
    let high = |image: &Image| painted_rows(image, 0, 120);
    assert!(high(&shot_on(&dull, 12)) < high(&played[12]) / 2, "{} against {}", high(&shot_on(&dull, 12)), high(&played[12]));
}

#[test]
fn gradients_and_pictures_paint_the_areas_of_their_swatches() {
    let (mut project, ..) = scene();
    common::shaded(&mut project);
    let image = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("shades", &image);
    // The field runs from orange at its top to blue at its bottom, as it was placed.
    let (top, bottom) = (image.pixel(60, 31), image.pixel(60, 148));
    assert!(top[0] > 240 && top[2] < 60 && bottom[0] < 50 && bottom[2] > 200, "{top:?} to {bottom:?}");
    // The band is whitest in its middle and fades out to its ends, round its own middle.
    let band = |x| image.pixel(x, 85)[1];
    assert!(band(120) > band(90) && band(90) > band(45), "{} {} {}", band(120), band(90), band(45));
    // In linear light the gradients are blended otherwise.
    assert_ne!(shoot(&project, WorkingSpace::Linear, 1.0).pixel(60, 90), image.pixel(60, 90));
}

#[test]
fn a_gradient_bends_with_its_drawing() {
    use iskra_model::{Deformer, DeformerId};
    let (mut project, ..) = scene();
    common::shaded(&mut project);
    // As in the curve's test: the mist's band, with its round gradient, swept up and down.
    let id = DeformerId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    Arc::make_mut(&mut shot.pegs[0]).transform.position = [1000.0, 0.0];
    shot.deformers.push(Arc::new(Deformer::curve(id, "Band", &[[-150.0, 0.0], [0.0, 0.0], [150.0, 0.0]])));
    Arc::make_mut(&mut shot.layers[1]).deformer = Some(id);
    let curve = Arc::make_mut(&mut project.shots[0]).deformer_mut(id).unwrap();
    curve.pose_knot(1, 0, [0.0, -30.0]);
    let swept = shoot(&project, WorkingSpace::Srgb, 1.0);
    golden("shades-bent", &swept);
    // The band's whitest is its middle, which has gone up with its point.
    assert!(swept.pixel(160, 60)[0] > swept.pixel(160, 90)[0], "{:?} {:?}", swept.pixel(160, 60), swept.pixel(160, 90));
}
