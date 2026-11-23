//! The GPU compositor held against the reference (plan §3): the same scenes, rendered by both,
//! have to come out the same, a level of a channel apart at most.
//!
//! Where there is no graphics card, nor a stand-in for one, there is nothing to hold: the
//! tests say so and pass.

use std::sync::Arc;

use common::scene;
use iskra_gpu::Compositor;
use iskra_model::{Animation, Peg, PegId, Project, Transform, WorkingSpace};
use iskra_render::{Image, View};

#[path = "../../render/tests/common/mod.rs"]
#[allow(dead_code)]
mod common;

fn compositor() -> Option<Compositor> {
    let found = Compositor::headless();
    if found.is_none() {
        eprintln!("no graphics card here: the GPU compositor is not tried");
    }
    found
}

/// Render the project's first shot on frame 0 both ways and hold them against each other.
fn same(compositor: &mut Compositor, what: &str, project: &Project, space: WorkingSpace, scale: f64) -> Image {
    let view = View::frame(&project.settings, scale);
    let reference = iskra_render::render(&project.shots[0], &project.palette, space, 0, &view);
    let frame = compositor.render(&project.shots[0], &project.palette, space, 0, &view);
    let made = compositor.read(&frame).expect("the frame is read back");
    assert_eq!((made.width, made.height), (reference.width, reference.height), "{what}");
    // Where there is next to nothing of a pixel, a level of alpha or none, its colour says
    // nothing: one compositor's nothing is the other's faintest white.
    let apart = |a: &[u8; 4], b: &[u8; 4]| if a[3] <= 1 && b[3] <= 1 { 0 } else { a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0) };
    let off: Vec<_> = made.pixels.iter().zip(&reference.pixels).enumerate().filter(|(_, (a, b))| apart(a, b) > 1).collect();
    if let Some((at, (made, reference))) = off.first() {
        let (x, y) = (*at as u32 % view.width, *at as u32 / view.width);
        panic!("{what}: {} pixels differ, the first at ({x}, {y}): {made:?} on the card, {reference:?} by the reference", off.len());
    }
    assert!(made.pixels.iter().any(|pixel| pixel[3] > 0), "{what}: something is drawn");
    // Made ready to be shown, it is the same picture with its colours times their alpha.
    let shown = compositor.read_shown(&compositor.shown(&frame)).expect("the shown frame is read back");
    let times = |pixel: &[u8; 4]| [0, 1, 2, 3].map(|c| if c == 3 { pixel[3] } else { ((u32::from(pixel[c]) * u32::from(pixel[3]) + 127) / 255) as u8 });
    let off = shown.iter().zip(&reference.pixels).filter(|(shown, reference)| apart(shown, &times(reference)) > 2).count();
    assert_eq!(off, 0, "{what}: pixels of the shown frame differ");
    made
}

/// The scene with the ball brought near and the camera panned and rolled by a peg.
fn with_camera() -> Project {
    let (mut project, ..) = scene();
    let rig = PegId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let distance = shot.camera.distance;
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
    project
}

#[test]
fn the_card_draws_what_the_reference_draws() {
    let Some(mut compositor) = compositor() else { return };
    let (project, ..) = scene();
    let stack = same(&mut compositor, "layers over each other", &project, WorkingSpace::Srgb, 1.0);
    assert_eq!((stack.pixel(60, 30), stack.pixel(200, 78), stack.pixel(2, 2)), ([38, 110, 210, 255], [220, 50, 47, 255], [0, 0, 0, 0]));
    same(&mut compositor, "in linear light", &project, WorkingSpace::Linear, 1.0);
    same(&mut compositor, "at a quarter of the size", &project, WorkingSpace::Srgb, 0.25);
    same(&mut compositor, "at twice the size", &project, WorkingSpace::Srgb, 2.0);
    same(&mut compositor, "enlarged", &project, WorkingSpace::Linear, 3.5);
    let camera = with_camera();
    same(&mut compositor, "through a moved camera", &camera, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "through a moved camera, small", &camera, WorkingSpace::Linear, 0.4);
    // Depth decides what is over what on the card too.
    let (mut deep, ..) = scene();
    Arc::make_mut(&mut Arc::make_mut(&mut deep.shots[0]).pegs[0]).transform.depth = -1.0;
    assert_eq!(same(&mut compositor, "a layer moved back", &deep, WorkingSpace::Srgb, 1.0).pixel(200, 78), [38, 110, 210, 255]);
    // A composite that stacks by its ports, whatever the depths, does so on the card too.
    Arc::make_mut(&mut Arc::make_mut(&mut deep.shots[0]).composites[0]).stacking = iskra_model::Stacking::Ports;
    assert!(same(&mut compositor, "stacked by its ports", &deep, WorkingSpace::Srgb, 1.0).pixel(200, 78) != [38, 110, 210, 255]);
    // A mirrored layer is seen from behind, and seen all the same.
    let (mut mirrored, ..) = scene();
    Arc::make_mut(&mut Arc::make_mut(&mut mirrored.shots[0]).pegs[0]).transform.scale = [-1.5, 0.75];
    same(&mut compositor, "mirrored", &mirrored, WorkingSpace::Srgb, 1.0);
}

#[test]
fn the_card_draws_a_tilted_layer_as_the_reference_does() {
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ..) = scene();
    Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]).transform.tilt = [55.0, 25.0];
    same(&mut compositor, "tilted", &project, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "tilted, in linear light and small", &project, WorkingSpace::Linear, 0.4);
    same(&mut compositor, "tilted, enlarged", &project, WorkingSpace::Srgb, 2.5);
    let mut camera = with_camera();
    Arc::make_mut(&mut Arc::make_mut(&mut camera.shots[0]).pegs[0]).transform.tilt = [-40.0, 60.0];
    same(&mut compositor, "tilted, through a moved camera", &camera, WorkingSpace::Srgb, 1.0);
    // A card that passes the camera is drawn as far as it is in front of it.
    let peg = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]);
    (peg.transform.tilt, peg.transform.scale, peg.transform.rotation) = ([80.0, 0.0], [6.0, 6.0], 0.0);
    same(&mut compositor, "a floor that passes the camera", &project, WorkingSpace::Srgb, 1.0);
}

#[test]
fn a_drawing_larger_than_a_texture_goes_up_in_pieces_and_looks_the_same() {
    let Some(mut compositor) = compositor() else { return };
    // As on a card whose textures are 100 pixels a side at most: the field alone is 480 wide.
    compositor.limit_textures(100);
    let (project, ..) = scene();
    same(&mut compositor, "in pieces", &project, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "in pieces, enlarged", &project, WorkingSpace::Srgb, 2.0);
    same(&mut compositor, "in pieces, through a moved camera", &with_camera(), WorkingSpace::Linear, 1.0);
    same(&mut compositor, "in pieces, small", &project, WorkingSpace::Srgb, 0.5);
    let mut tilted = project.clone();
    Arc::make_mut(&mut Arc::make_mut(&mut tilted.shots[0]).pegs[0]).transform.tilt = [50.0, 20.0];
    same(&mut compositor, "in pieces, tilted", &tilted, WorkingSpace::Srgb, 1.0);
}

#[test]
fn a_drawing_larger_than_a_texture_is_kept_as_it_was_painted_in_parts() {
    use iskra_model::{ArtContent, SwatchId};
    let Some(mut compositor) = compositor() else { return };
    // Textures of three tiles a side: the field, made eight tiles wide, is in parts of one
    // tile, each with a tile round it.
    compositor.limit_textures(768);
    let (mut project, ..) = scene();
    let shot = Arc::make_mut(&mut project.shots[0]);
    let field = Arc::make_mut(&mut Arc::make_mut(&mut shot.layers[2]).drawings[0].drawing);
    if let ArtContent::Indexed(surface) = &mut field.art_layers[1].content {
        let wide = iskra_drawing::Rect::new(-1000, -60, 1000, 60);
        surface.paint(wide, &vec![0.75; 2000 * 120], SwatchId(5));
    }
    same(&mut compositor, "in parts, through a moved camera", &with_camera(), WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "in parts", &project, WorkingSpace::Srgb, 1.0);
    let sent = compositor.sent;
    assert!(sent > 0, "the field went up tile by tile");
    same(&mut compositor, "in parts, small", &project, WorkingSpace::Linear, 0.3);
    same(&mut compositor, "in parts, enlarged", &project, WorkingSpace::Srgb, 2.5);
    let sky = project.palette.swatches.iter().position(|swatch| swatch.id == SwatchId(5)).expect("the field's swatch");
    Arc::make_mut(&mut project.palette).swatches[sky].color = [20, 180, 90, 255];
    same(&mut compositor, "in parts, a swatch changed", &project, WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.sent, sent, "the palette is looked up on the card");
    Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]).transform.tilt = [50.0, 20.0];
    same(&mut compositor, "in parts, tilted", &project, WorkingSpace::Srgb, 1.0);
}

#[test]
fn drawings_stay_on_the_card_while_they_are_the_same_and_within_a_budget() {
    let Some(mut compositor) = compositor() else { return };
    let (project, ..) = scene();
    let view = View::frame(&project.settings, 1.0);
    compositor.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view);
    let kept = compositor.kept();
    assert!(kept > 0);
    compositor.render(&project.shots[0], &project.palette, WorkingSpace::Linear, 0, &view);
    assert_eq!(compositor.kept(), kept, "the same drawings under the same palette are not sent again");
    // Another palette is other colours: the drawings go up again, and with a budget too small
    // for both, the ones not drawn any more leave.
    let recoloured = Arc::new(iskra_model::Palette::clone(&project.palette));
    compositor.budget = kept;
    let frame = compositor.render(&project.shots[0], &recoloured, WorkingSpace::Srgb, 0, &view);
    assert_eq!(compositor.kept(), kept);
    assert!(compositor.read(&frame).is_some_and(|image| image.pixel(60, 30) == [38, 110, 210, 255]));
    // An empty frame is an empty picture.
    let empty = compositor.render(&project.shots[0], &project.palette, WorkingSpace::Srgb, 99, &view);
    assert!(compositor.read(&empty).is_some_and(|image| image.pixels.iter().all(|pixel| pixel == &[0; 4])));
}

#[test]
fn a_drawing_is_kept_as_it_was_painted_so_a_swatch_sends_nothing_and_a_stroke_its_tiles() {
    use iskra_drawing::Rect;
    use iskra_model::{ArtContent, SwatchId};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ..) = scene();
    let plain = same(&mut compositor, "as it was painted", &project, WorkingSpace::Srgb, 1.0);
    let sent = compositor.sent;
    assert!(sent > 0, "the drawings went up tile by tile");
    // A swatch of another colour is another picture, and no drawing is sent for it.
    let sky = project.palette.swatches.iter().position(|swatch| swatch.id == SwatchId(5)).expect("the field's swatch");
    Arc::make_mut(&mut project.palette).swatches[sky].color = [20, 180, 90, 255];
    let recoloured = same(&mut compositor, "a swatch changed", &project, WorkingSpace::Srgb, 1.0);
    assert!(recoloured.pixels != plain.pixels && recoloured.pixel(60, 30) == [20, 180, 90, 255]);
    assert_eq!(compositor.sent, sent, "the palette is looked up on the card");
    // The field is on four tiles. Painted on in one of them, that one is sent.
    fn field(project: &mut Project) -> &mut iskra_drawing::Drawing {
        let shot = Arc::make_mut(&mut project.shots[0]);
        let layer = Arc::make_mut(&mut shot.layers[2]);
        Arc::make_mut(&mut layer.drawings[0].drawing)
    }
    if let ArtContent::Indexed(surface) = &mut field(&mut project).art_layers[1].content {
        surface.paint(Rect::new(20, 20, 70, 70), &vec![1.0; 2500], SwatchId(3));
    }
    let kept = compositor.kept();
    let painted = same(&mut compositor, "painted on", &project, WorkingSpace::Srgb, 1.0);
    assert!(painted.pixels != recoloured.pixels);
    assert_eq!((compositor.sent, compositor.kept()), (sent + 1, kept), "one tile, into the texture that was there");
    // Free colour under it, half clear, on a tile that had nothing: its colours are as they are.
    let mut free = iskra_drawing::free_colour();
    free.fill_rect(Rect::new(-200, -100, -40, 40), &[0.9, 0.6, 0.1, 0.5]);
    field(&mut project).art_layers[0].content = ArtContent::Rgba(free);
    let mixed = same(&mut compositor, "with free colour", &project, WorkingSpace::Srgb, 1.0);
    assert!(mixed.pixels == painted.pixels, "the field covers it");
    if let ArtContent::Indexed(surface) = &mut field(&mut project).art_layers[1].content {
        surface.erase(Rect::new(-240, -120, 0, 0), &vec![1.0; 240 * 120]);
    }
    let through = same(&mut compositor, "free colour where the field is rubbed out", &project, WorkingSpace::Linear, 1.0);
    assert!(through.pixels != mixed.pixels);
    same(&mut compositor, "and small", &project, WorkingSpace::Srgb, 0.5);
}

#[test]
fn the_window_fades_layers_and_draws_tinted_ghosts_under_them() {
    use iskra_drawing::{Affine, Rect};
    use iskra_gpu::{Ghost, Look, Looks};
    use iskra_model::{ArtContent, SwatchId};
    let Some(mut compositor) = compositor() else { return };
    let (project, ball, _) = scene();
    let (shot, palette) = (&project.shots[0], &project.palette);
    let view = View::frame(&project.settings, 1.0);
    let looked = |compositor: &mut Compositor, looks: &Looks, space: WorkingSpace| {
        let frame = compositor.render_looked(shot, palette, space, 0, &view, looks);
        compositor.read(&frame).expect("the frame is read back")
    };
    let near = |a: [u8; 4], b: [f32; 4]| a.iter().zip(b).all(|(a, b)| (f32::from(*a) - b).abs() <= 2.0);
    let plain = same(&mut compositor, "as the shot shows it", &project, WorkingSpace::Srgb, 1.0);
    assert_eq!(looked(&mut compositor, &Looks::default(), WorkingSpace::Srgb).pixels, plain.pixels, "with nothing to look at otherwise, it is the render");

    // The field faded to half: where it alone is, half of it is there, and the ball over it
    // is as it was.
    let mut looks = Looks::default();
    looks.layers.insert(shot.layers[2].id, Look { opacity: 0.5, tint: None });
    let faded = looked(&mut compositor, &looks, WorkingSpace::Srgb);
    assert_eq!(plain.pixel(60, 30), [38, 110, 210, 255]);
    assert!(near(faded.pixel(60, 30), [38.0, 110.0, 210.0, 127.5]), "{:?}", faded.pixel(60, 30));
    assert_eq!(faded.pixel(200, 78), plain.pixel(200, 78));

    // A ghost of the ball's drawing, green and six tenths there, ninety pixels to the left of
    // the ball and under it: over the field there, and nothing of it over the ball.
    let drawing = shot.layer(ball).unwrap().drawings[0].drawing.clone();
    let placed = iskra_render::seen(shot, 0).into_iter().find(|seen| Arc::ptr_eq(&seen.drawing, &drawing)).expect("the ball is seen").to_frame;
    let ghost = |drawing: &Arc<iskra_drawing::Drawing>, lines_only: bool| Ghost {
        under: ball,
        drawing: drawing.clone(),
        to_frame: placed.then(&Affine::translate(-90.0, 0.0)),
        warp: None,
        look: Look { opacity: 0.6, tint: Some([0.2, 0.8, 0.3]) },
        lines_only,
    };
    let with = |ghost: Ghost| Looks { ghosts: vec![ghost], ..Looks::default() };
    let ghosted = looked(&mut compositor, &with(ghost(&drawing, false)), WorkingSpace::Srgb);
    let under = plain.pixel(110, 78);
    let mixed = [0, 1, 2].map(|c| 0.6 * [51.0, 204.0, 76.5][c] + 0.4 * f32::from(under[c]));
    assert!(near(ghosted.pixel(110, 78), [mixed[0], mixed[1], mixed[2], 255.0]), "{:?} over {under:?}", ghosted.pixel(110, 78));
    assert_eq!((ghosted.pixel(200, 78), ghosted.pixel(60, 30)), (plain.pixel(200, 78), plain.pixel(60, 30)));
    assert!(looked(&mut compositor, &with(ghost(&drawing, false)), WorkingSpace::Linear).pixel(110, 78) != plain.pixel(110, 78));
    // A ghost is what the drawing covers: the ball's swatch made half clear, its ghost is as
    // strong as it was, as the window's tiles have it.
    let mut clear = project.palette.as_ref().clone();
    let red = clear.swatches.iter().position(|swatch| swatch.id == SwatchId(3)).expect("the ball's swatch");
    clear.swatches[red].color[3] = 128;
    let clear = Arc::new(clear);
    let frame = compositor.render_looked(shot, &clear, WorkingSpace::Srgb, 0, &view, &with(ghost(&drawing, false)));
    let half = compositor.read(&frame).expect("the frame is read back");
    assert!(near(half.pixel(110, 78), [mixed[0], mixed[1], mixed[2], 255.0]), "{:?}", half.pixel(110, 78));

    // Of a drawing with colour art too, a ghost of its lines alone is the ghost of the lines.
    let mut coloured = iskra_drawing::Drawing::clone(&drawing);
    if let ArtContent::Indexed(surface) = &mut coloured.art_layers[0].content {
        surface.paint(Rect::new(-200, -200, 200, 200), &vec![1.0; 400 * 400], SwatchId(5));
    }
    let coloured = Arc::new(coloured);
    assert_eq!(looked(&mut compositor, &with(ghost(&coloured, true)), WorkingSpace::Srgb).pixels, ghosted.pixels);
    assert!(looked(&mut compositor, &with(ghost(&coloured, false)), WorkingSpace::Srgb).pixels != ghosted.pixels);
}

#[test]
fn a_shot_shown_in_a_palette_style_has_its_colours_both_ways() {
    use iskra_model::{Style, StyleId, SwatchId};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ..) = scene();
    let plain = same(&mut compositor, "the palette's own colours", &project, WorkingSpace::Srgb, 1.0);
    // Every swatch another colour in the style, and the shot shown in it.
    let colors = project.palette.swatches.iter().map(|swatch| (swatch.id, [swatch.color[2], swatch.color[0], 200, 255])).collect();
    Arc::make_mut(&mut project.palette).styles.push(Style {
        id: StyleId(1),
        name: "Night".to_string(),
        colors,
        set: Default::default(),
        kinds: Default::default(),
    });
    assert_eq!(same(&mut compositor, "a style no shot is shown in", &project, WorkingSpace::Srgb, 1.0).pixels, plain.pixels);
    Arc::make_mut(&mut project.shots[0]).styles = vec![StyleId(1)];
    let night = same(&mut compositor, "in a style", &project, WorkingSpace::Srgb, 1.0);
    assert!(night.pixels != plain.pixels && night.pixels.iter().any(|pixel| pixel[2] == 200 && pixel[3] == 255), "the style's colours are drawn");
    assert_eq!(project.palette.color(SwatchId(1)), [0, 0, 0, 255], "and the palette's own are as they were");
}

#[test]
fn a_style_node_is_drawn_as_the_reference_draws_it() {
    use iskra_model::{MATTE, STYLE, STYLE_PALETTE, Source, Style, StyleId, Value};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ball, _) = scene();
    let colors = project.palette.swatches.iter().map(|swatch| (swatch.id, [swatch.color[2], swatch.color[0], 200, 255])).collect();
    Arc::make_mut(&mut project.palette).styles.push(Style {
        id: StyleId(1),
        name: "Night".to_string(),
        colors,
        set: Default::default(),
        kinds: Default::default(),
    });
    // Over the whole shot, half in Night, where the ball is.
    let shown = project.shots[0].display.unwrap();
    let node = put_on(&mut project, STYLE, shown, &[(STYLE, Value::Text("Night".to_string())), ("amount", Value::Number(50.0))]);
    Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap().inputs.insert(MATTE.to_string(), Source::Layer(ball));
    // Made on the card, not by the reference.
    let by_reference = compositor.effects.1;
    same(&mut compositor, "a style by a matte", &project, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "a style by a matte, at half the size", &project, WorkingSpace::Linear, 0.5);
    assert_eq!(compositor.effects.1, by_reference, "the reference made none of it");
    // With no matte, the whole shot half in Night.
    Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap().inputs.remove(MATTE);
    same(&mut compositor, "a style over the whole shot", &project, WorkingSpace::Srgb, 1.0);
    // The shot shown in Night, and the node back to the palette's own colours.
    Arc::make_mut(&mut project.shots[0]).styles = vec![StyleId(1)];
    Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap().param_mut(STYLE).unwrap().value = Value::Text(String::new());
    same(&mut compositor, "the palette's own colours in a shot shown in a style", &project, WorkingSpace::Srgb, 1.0);
    // In Night of the first palette alone, with a swatch moved to another.
    Arc::make_mut(&mut project.shots[0]).styles.clear();
    let palette = Arc::make_mut(&mut project.palette);
    palette.swatches[0].set = palette.new_set("Other").unwrap();
    let node = Arc::make_mut(&mut project.shots[0]).effect_mut(node).unwrap();
    node.param_mut(STYLE).unwrap().value = Value::Text("Night".to_string());
    node.param_mut(STYLE_PALETTE).unwrap().value = Value::Text("Default".to_string());
    same(&mut compositor, "a style of one palette", &project, WorkingSpace::Srgb, 1.0);
}

#[test]
fn the_card_draws_a_bent_drawing_as_the_reference_does() {
    use iskra_model::{Bone, Deformer, DeformerId};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ball, _) = scene();
    // Two bones along the mist's band, the second turned; and two across the ball, which is
    // on a peg that turns and stretches it.
    let (band, blob) = (DeformerId(project.new_id()), DeformerId(project.new_id()));
    let chain = |id, name: &str, origin, length: f64, turn| {
        let bones = vec![Bone::at_rest(0.0, length), Bone { turn, ..Bone::at_rest(0.0, length) }];
        Arc::new(Deformer::new(id, name, origin, bones))
    };
    let shot = Arc::make_mut(&mut project.shots[0]);
    shot.deformers.extend([chain(band, "Band", [-150.0, 0.0], 150.0, 60.0), chain(blob, "Blob", [-30.0, 0.0], 30.0, -45.0)]);
    for layer in &mut shot.layers {
        let deformer = if layer.id == ball { Some(blob) } else { (layer.name == "Mist").then_some(band) };
        Arc::make_mut(layer).deformer = deformer;
    }
    let bent = same(&mut compositor, "bent by bones", &project, WorkingSpace::Srgb, 1.0);
    assert!(bent.pixel(180, 125) != [38, 110, 210, 255], "the band hangs where its bone was turned to");
    assert_eq!(compositor.bends, (2, 0), "both are bent on the card, from the drawings kept as they were painted");
    same(&mut compositor, "bent, in linear light and small", &project, WorkingSpace::Linear, 0.5);
    // The ball's swatch of another colour: bent again on the card, and nothing is sent.
    let sent = compositor.sent;
    let red = project.palette.swatches.iter().position(|swatch| swatch.id == iskra_model::SwatchId(3)).expect("the ball's swatch");
    Arc::make_mut(&mut project.palette).swatches[red].color = [20, 180, 90, 255];
    let recoloured = same(&mut compositor, "bent, a swatch changed", &project, WorkingSpace::Srgb, 1.0);
    assert!(recoloured.pixels != bent.pixels && compositor.sent == sent, "the palette is looked up on the card");
    // Painted on in one tile of the ball, that tile is sent, and the ball is bent again.
    let shot = Arc::make_mut(&mut project.shots[0]);
    let drawing = Arc::make_mut(&mut Arc::make_mut(&mut shot.layers[0]).drawings[0].drawing);
    if let iskra_model::ArtContent::Indexed(surface) = &mut drawing.art_layers[1].content {
        surface.paint(iskra_drawing::Rect::new(10, 10, 30, 30), &[1.0; 400], iskra_model::SwatchId(1));
    }
    let bends = compositor.bends.0;
    let painted = same(&mut compositor, "bent, painted on", &project, WorkingSpace::Srgb, 1.0);
    assert!(painted.pixels != recoloured.pixels);
    assert_eq!((compositor.sent, compositor.bends.0), (sent + 1, bends + 1), "one tile, and one bend of the ball");
    // Past what a texture holds, it is bent on the processor as before.
    compositor.limit_textures(100);
    same(&mut compositor, "bent, larger than a texture", &project, WorkingSpace::Srgb, 1.0);
    assert!(compositor.bends.1 > 0);
    compositor.limit_textures(u32::MAX);
    // Bent and leant back at once: the bent drawing is a card like any other.
    Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).pegs[0]).transform.tilt = [40.0, 15.0];
    same(&mut compositor, "bent and tilted", &project, WorkingSpace::Srgb, 1.0);
}

#[test]
fn the_card_leaves_out_the_art_layers_switched_off_as_the_reference_does() {
    use iskra_model::ArtLayerKind;
    use iskra_model::{Bone, Deformer, DeformerId};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ball, _) = scene();
    // The ball has colour art too, under its lines and off to one side.
    let shot = Arc::make_mut(&mut project.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
    if let iskra_model::ArtContent::Indexed(surface) = &mut drawing.art_layers[0].content {
        surface.paint(iskra_drawing::Rect::new(0, 0, 100, 80), &[1.0; 8000], iskra_model::SwatchId(2));
    }
    let off = |project: &mut Project, off: &[ArtLayerKind]| {
        let shot = Arc::make_mut(&mut project.shots[0]);
        Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap()).art_off = off.to_vec();
    };
    let whole = same(&mut compositor, "both art layers", &project, WorkingSpace::Srgb, 1.0);
    for (kinds, what) in [(&[ArtLayerKind::ColourArt][..], "the colour art off"), (&[ArtLayerKind::LineArt][..], "the line art off")] {
        off(&mut project, kinds);
        let made = same(&mut compositor, what, &project, WorkingSpace::Srgb, 1.0);
        assert!(made.pixels != whole.pixels, "{what}: something is left out");
        // Kept as it was painted in parts, and bent.
        compositor.limit_textures(100);
        same(&mut compositor, &format!("{what}, in parts"), &project, WorkingSpace::Srgb, 1.0);
        compositor.limit_textures(u32::MAX);
    }
    let blob = DeformerId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let bones = vec![Bone::at_rest(0.0, 30.0), Bone { turn: -45.0, ..Bone::at_rest(0.0, 30.0) }];
    shot.deformers.push(Arc::new(Deformer::new(blob, "Blob", [-30.0, 0.0], bones)));
    Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap()).deformer = Some(blob);
    for kinds in [ArtLayerKind::ColourArt, ArtLayerKind::LineArt] {
        off(&mut project, &[kinds]);
        same(&mut compositor, &format!("bent, {kinds:?} off"), &project, WorkingSpace::Srgb, 1.0);
    }
}

#[test]
fn the_card_draws_an_art_layers_output_and_the_overart_as_the_reference_does() {
    use iskra_model::{ArtLayerKind, Source};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ball, _) = scene();
    // The ball has colour art under its lines and overart over them, each off to one side.
    let shot = Arc::make_mut(&mut project.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
    for (at, rect, swatch) in [(0, iskra_drawing::Rect::new(0, 0, 100, 80), 2), (2, iskra_drawing::Rect::new(-90, -70, -20, 0), 4)] {
        if let iskra_model::ArtContent::Indexed(surface) = &mut drawing.art_layers[at].content {
            surface.paint(rect, &vec![1.0; (rect.width() * rect.height()) as usize], iskra_model::SwatchId(swatch));
        }
    }
    let whole = same(&mut compositor, "all three art layers", &project, WorkingSpace::Srgb, 1.0);
    let wire = |project: &mut Project, to: Vec<Source>| {
        let stack = Arc::make_mut(&mut Arc::make_mut(&mut project.shots[0]).composites[0]);
        let (at, after) =
            (stack.inputs.iter().position(|i| i.layer() == Some(ball)).unwrap(), stack.inputs.iter().rposition(|i| i.layer() == Some(ball)).unwrap());
        stack.inputs.splice(at..=after, to);
    };
    for kind in ArtLayerKind::ALL {
        wire(&mut project, vec![Source::Art(ball, kind)]);
        let made = same(&mut compositor, &format!("{kind:?} alone"), &project, WorkingSpace::Srgb, 1.0);
        assert!(made.pixels != whole.pixels, "{kind:?}: the others are left out");
    }
    let all = ArtLayerKind::ALL.into_iter().rev().map(|kind| Source::Art(ball, kind)).collect();
    wire(&mut project, all);
    // Each output is laid on the frame and then over the others, where the flat output's art
    // layers are over each other first: the two differ a little along the soft edges alone.
    let each = same(&mut compositor, "each from its own output", &project, WorkingSpace::Srgb, 1.0);
    let apart = each.pixels.iter().zip(&whole.pixels).filter(|(a, b)| a != b).count();
    assert!(apart < 60, "{apart} pixels apart");
}

#[test]
fn a_ghost_of_a_bent_drawing_is_what_it_covers() {
    use iskra_drawing::Affine;
    use iskra_gpu::{Ghost, Look, Looks};
    use iskra_model::{Bone, Deformer, DeformerId, SwatchId};
    let Some(mut compositor) = compositor() else { return };
    let (project, ball, _) = scene();
    let shot = &project.shots[0];
    let layer = shot.layer(ball).unwrap();
    let bones = vec![Bone::at_rest(0.0, 30.0), Bone { turn: -45.0, ..Bone::at_rest(0.0, 30.0) }];
    let warp = Deformer::new(DeformerId(9000), "Blob", [-30.0, 0.0], bones).warp(0, layer.density);
    let drawing = layer.drawings[0].drawing.clone();
    let placed = iskra_render::seen(shot, 0).into_iter().find(|seen| Arc::ptr_eq(&seen.drawing, &drawing)).expect("the ball is seen").to_frame;
    let looks = Looks {
        ghosts: vec![Ghost {
            under: ball,
            drawing,
            to_frame: placed.then(&Affine::translate(-90.0, 0.0)),
            warp: Some(warp),
            look: Look { opacity: 0.6, tint: Some([0.2, 0.8, 0.3]) },
            lines_only: false,
        }],
        ..Looks::default()
    };
    let view = View::frame(&project.settings, 1.0);
    let mut clear = project.palette.as_ref().clone();
    let red = clear.swatches.iter().position(|swatch| swatch.id == SwatchId(3)).expect("the ball's swatch");
    clear.swatches[red].color[3] = 128;
    let clear = Arc::new(clear);
    let looked = |compositor: &mut Compositor, palette: &Arc<iskra_model::Palette>| {
        let frame = compositor.render_looked(shot, palette, WorkingSpace::Srgb, 0, &view, &looks);
        compositor.read(&frame).expect("the frame is read back")
    };
    // Bent on the card, and on the processor where it is larger than a texture may be: as
    // strong with the ball's swatch half clear as with it whole.
    let whole = looked(&mut compositor, &project.palette);
    assert_eq!(looked(&mut compositor, &clear).pixel(110, 78), whole.pixel(110, 78), "bent on the card");
    compositor.limit_textures(100);
    let whole = looked(&mut compositor, &project.palette);
    assert_eq!(looked(&mut compositor, &clear).pixel(110, 78), whole.pixel(110, 78), "bent on the processor");
    assert!(compositor.bends.1 > 0);
}

#[test]
fn what_the_reference_makes_of_an_effect_is_faded_as_its_layers_are() {
    use iskra_gpu::{Look, Looks};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ball, _) = scene();
    // A tone wider than the card takes is the reference's.
    put_on(&mut project, "tone", iskra_model::Source::Layer(ball), &[("width", iskra_model::Value::Number(200.0))]);
    // The ball alone, so that what is put in is seen by its alpha.
    let shot = Arc::make_mut(&mut project.shots[0]);
    let others: Vec<u64> = shot.layers.iter().filter(|layer| layer.id != ball).map(|layer| layer.id.0).collect();
    shot.hidden.extend(others);
    let view = View::frame(&project.settings, 1.0);
    let looked = |compositor: &mut Compositor, looks: &Looks| {
        let frame = compositor.render_looked(&project.shots[0], &project.palette, WorkingSpace::Srgb, 0, &view, looks);
        compositor.read(&frame).expect("the frame is read back")
    };
    let plain = looked(&mut compositor, &Looks::default());
    let made = compositor.effects.1;
    assert!(made > 0 && plain.pixel(200, 78)[3] == 255, "the reference made it: {:?}", plain.pixel(200, 78));
    // Faded to half, on the light table say, half of what the reference made is put in.
    let mut looks = Looks::default();
    looks.layers.insert(ball, Look { opacity: 0.5, tint: None });
    let faded = looked(&mut compositor, &looks);
    assert!(compositor.effects.1 > made, "the reference made it again");
    assert!(faded.pixel(200, 78)[3].abs_diff(128) <= 2, "{:?}", faded.pixel(200, 78));
}

/// Put a new effect node of `kind` on the wire that leaves `on` in the scene's shot, with
/// its parameters set as `set` says.
fn put_on(project: &mut Project, kind: &str, on: iskra_model::Source, set: &[(&str, iskra_model::Value)]) -> iskra_model::EffectId {
    use iskra_model::{Effect, EffectId, PICTURE, Source, effect_kind};
    let id = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mut node = Effect::new(id, kind, effect_kind(kind).expect("the kind is known"));
    node.inputs.insert(PICTURE.to_string(), on);
    for (param, value) in set {
        node.param_mut(param).expect("the kind has the parameter").value = value.clone();
    }
    // What took it takes it through the node, the display too.
    shot.rewire(on, Some(Source::Effect(id)));
    shot.effects.push(Arc::new(node));
    id
}

#[test]
fn a_colour_card_is_drawn_as_the_reference_draws_it() {
    use iskra_model::{COLOUR_CARD, DEPTH, Effect, EffectId, Source, Value, effect_kind};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ..) = scene();
    let id = EffectId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let mut node = Effect::new(id, "Card", effect_kind(COLOUR_CARD).unwrap());
    node.param_mut("colour").unwrap().value = Value::Colour([0.2, 0.6, 0.3, 0.8]);
    node.param_mut(DEPTH).unwrap().value = Value::Number(-1.0);
    let main = shot.main_composite().unwrap();
    Arc::make_mut(shot.composites.iter_mut().find(|composite| composite.id == main).unwrap()).inputs.push(Source::Effect(id));
    shot.effects.push(Arc::new(node));
    let made = same(&mut compositor, "a colour card", &project, WorkingSpace::Srgb, 1.0);
    assert!(made.pixel(2, 2)[3] > 0, "the card is behind all of the frame");
    same(&mut compositor, "a colour card, in linear light and small", &project, WorkingSpace::Linear, 0.5);
    assert_eq!(compositor.effects.1, 0, "made on the card");
    // Rain, snow and speed lines over it, at another size too.
    for kind in ["Rain", "Snow", "Wind", "Burst", "Speed Lines"] {
        let mut falling = project.clone();
        let id = EffectId(falling.new_id());
        let shot = Arc::make_mut(&mut falling.shots[0]);
        let mut node = Effect::new(id, kind, effect_kind(iskra_model::PARTICLES).unwrap());
        node.param_mut("kind").unwrap().value = Value::Choice(kind.to_string());
        Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(id));
        shot.effects.push(Arc::new(node));
        let by_reference = compositor.effects.1;
        same(&mut compositor, kind, &falling, WorkingSpace::Srgb, 1.0);
        same(&mut compositor, kind, &falling, WorkingSpace::Linear, 0.5);
        // The Colour Card under them is made on the card too.
        assert_eq!(compositor.effects.1, by_reference, "{kind} is laid on the card");
        // With the ball's drawing wired into its sprite port, turned and spinning.
        let ball = falling.shots[0].layers.iter().find(|layer| layer.name == "Ball").unwrap().id;
        let shot = Arc::make_mut(&mut falling.shots[0]);
        let node = Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == id).unwrap());
        node.inputs.insert(iskra_model::SPRITE.to_string(), Source::Layer(ball));
        for (param, value) in [("spriteScale", 30.0), ("turn", 100.0), ("spin", 10.0), ("tint", 50.0)] {
            node.param_mut(param).unwrap().value = Value::Number(value);
        }
        same(&mut compositor, &format!("{kind} of sprites"), &falling, WorkingSpace::Srgb, 1.0);
        assert_eq!(compositor.effects.1, by_reference, "{kind} of sprites is laid on the card");
    }
    // Rain spread in depth, seen through a camera that is panned and rolled.
    let mut filmed = with_camera();
    let id = EffectId(filmed.new_id());
    let shot = Arc::make_mut(&mut filmed.shots[0]);
    let mut node = Effect::new(id, "Rain", effect_kind(iskra_model::PARTICLES).unwrap());
    node.param_mut("depthRange").unwrap().value = Value::Number(300.0);
    node.param_mut("splash").unwrap().value = Value::Switch(true);
    node.param_mut("ground").unwrap().value = Value::Number(30.0);
    Arc::make_mut(&mut shot.composites[0]).inputs.insert(0, Source::Effect(id));
    shot.effects.push(Arc::new(node));
    same(&mut compositor, "rain in depth through the camera, splashing", &filmed, WorkingSpace::Srgb, 1.0);
    // Its drops and its splashes drawn as sprites, two pictures on the card side by side.
    let mut sprites = filmed.clone();
    let ball = sprites.shots[0].layers.iter().find(|layer| layer.name == "Ball").unwrap().id;
    let shot = Arc::make_mut(&mut sprites.shots[0]);
    let node = Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == id).unwrap());
    node.inputs.insert(iskra_model::SPRITE.to_string(), Source::Layer(ball));
    node.inputs.insert(iskra_model::SPLASH_SPRITE.to_string(), Source::Layer(ball));
    node.param_mut("spriteScale").unwrap().value = Value::Number(20.0);
    same(&mut compositor, "rain and its splashes as sprites", &sprites, WorkingSpace::Srgb, 1.0);
    // Out of an emitter over the frame, from frame 0.
    let shot = Arc::make_mut(&mut filmed.shots[0]);
    let node = Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == id).unwrap());
    node.param_mut("emitter").unwrap().value = Value::Choice("Area".to_string());
    node.param_mut("emitterY").unwrap().value = Value::Number(-60.0);
    same(&mut compositor, "rain out of an emitter", &filmed, WorkingSpace::Srgb, 1.0);
    // In two sheets, the field between them.
    let shot = Arc::make_mut(&mut filmed.shots[0]);
    let node = Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == id).unwrap());
    node.param_mut("planes").unwrap().value = Value::Number(2.0);
    same(&mut compositor, "rain in two sheets", &filmed, WorkingSpace::Srgb, 1.0);
}

#[test]
fn the_card_blends_as_the_reference_does() {
    use iskra_model::{Source, Value};
    let Some(mut compositor) = compositor() else { return };
    let (plain, ball, _) = scene();
    let untouched = same(&mut compositor, "no blend", &plain, WorkingSpace::Srgb, 1.0);
    // The ball laid onto the field and the mist by each mode, at all of it and at a part.
    let mut seen = Vec::new();
    for mode in iskra_model::BLEND_MODES {
        for (amount, space) in [(100.0, WorkingSpace::Srgb), (60.0, WorkingSpace::Linear)] {
            let mut project = plain.clone();
            put_on(&mut project, iskra_model::BLEND, Source::Layer(ball), &[("mode", Value::Choice(mode.to_string())), ("amount", Value::Number(amount))]);
            let laid = same(&mut compositor, &format!("{mode} at {amount}"), &project, space, 1.0);
            if amount == 100.0 {
                seen.push((mode, laid.pixel(200, 78)));
            }
        }
    }
    // Normal is as if there were no Blend; the others are not, over the blue field.
    assert_eq!(seen[0].1, untouched.pixel(200, 78));
    assert!(seen[1..].iter().all(|(_, pixel)| *pixel != untouched.pixel(200, 78)), "{seen:?}");
    let field = [38u8, 110, 210];
    let (add, multiply) = (seen[1].1, seen[3].1);
    assert!(add[2] >= field[2] && multiply[2] <= field[2], "{seen:?}");
}

#[test]
fn the_card_blurs_and_glows_as_the_reference_does() {
    use iskra_model::{Source, Value};
    let Some(mut compositor) = compositor() else { return };
    let number = Value::Number;
    let (plain, ball, _) = scene();
    let field = Source::Layer(plain.shots[0].layers[2].id);
    let with = |kind: &str, on: Source, set: &[(&str, Value)]| {
        let mut project = plain.clone();
        put_on(&mut project, kind, on, set);
        project
    };
    let untouched = same(&mut compositor, "no effect", &plain, WorkingSpace::Srgb, 1.0);

    // A blur of one layer, under the others: past the field's edge the picture is soft, and
    // the ball over it is as sharp as it was.
    let blurred = with("blur", field, &[("radius", number(8.0))]);
    let soft = same(&mut compositor, "a blurred layer", &blurred, WorkingSpace::Srgb, 1.0);
    assert!(soft.pixel(36, 30)[3] > 0 && untouched.pixel(36, 30)[3] == 0 && soft.pixel(200, 78) == untouched.pixel(200, 78));
    // The blurred layer's card alone, as the window shows it while the shot stands still.
    let (shot, view) = (&blurred.shots[0], View::frame(&blurred.settings, 1.0));
    let cards = iskra_eval::evaluate(shot, 0).cards;
    let card = cards.iter().flat_map(iskra_eval::Card::leaves).find(|card| matches!(card, iskra_eval::Card::Through { .. })).expect("the blur's card");
    let reference = iskra_render::card(shot, &blurred.palette, WorkingSpace::Srgb, 0, &view, card);
    let alone = compositor.card(shot, &blurred.palette, WorkingSpace::Srgb, 0, &view, card);
    let alone = compositor.read(&alone).expect("the card is read back");
    let apart = |a: &[u8; 4], b: &[u8; 4]| if a[3] <= 1 && b[3] <= 1 { 0 } else { a.iter().zip(b).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0) };
    assert_eq!(alone.pixels.iter().zip(&reference.pixels).filter(|(a, b)| apart(a, b) > 1).count(), 0, "a card alone");
    assert!(alone.pixel(36, 30)[3] > 0 && alone.pixel(200, 78) != soft.pixel(200, 78), "the field's blur, without the ball over it");
    same(&mut compositor, "a blurred layer, in linear light", &blurred, WorkingSpace::Linear, 1.0);
    same(&mut compositor, "a blurred layer, at half the size", &blurred, WorkingSpace::Srgb, 0.5);
    same(&mut compositor, "a blurred layer, at twice the size", &blurred, WorkingSpace::Linear, 2.0);
    same(&mut compositor, "a blur of nothing", &with("blur", field, &[("radius", number(0.0))]), WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "a faint blur", &with("blur", field, &[("radius", number(1.0))]), WorkingSpace::Srgb, 1.0);
    // All that the display shows, through one blur: it reaches the frame's edges.
    let shown = plain.shots[0].display.unwrap();
    same(&mut compositor, "the whole picture blurred", &with("blur", shown, &[("radius", number(5.0))]), WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.effects, (8, 0), "all of them were made on the card");
    same(&mut compositor, "a wide blur", &with("blur", field, &[("radius", number(120.0))]), WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.effects, (9, 0));
    // A blur wider than the card makes is the reference's own, sent to the card.
    same(&mut compositor, "a very wide blur", &with("blur", field, &[("radius", number(400.0))]), WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.effects, (9, 1));

    // A glow round the ball, in its colour.
    let glowing = with("glow", Source::Layer(ball), &[("radius", number(10.0)), ("colour", Value::Colour([0.2, 1.0, 0.4, 0.8]))]);
    let lit = same(&mut compositor, "a glow", &glowing, WorkingSpace::Srgb, 1.0);
    assert!(lit != untouched && lit.pixel(200, 78) == untouched.pixel(200, 78));
    same(&mut compositor, "a glow, in linear light and small", &glowing, WorkingSpace::Linear, 0.5);
    same(
        &mut compositor,
        "a strong glow",
        &with("glow", Source::Layer(ball), &[("radius", number(4.0)), ("strength", number(300.0))]),
        WorkingSpace::Srgb,
        1.5,
    );
    same(&mut compositor, "a glow with no radius", &with("glow", Source::Layer(ball), &[("radius", number(0.0))]), WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.effects, (13, 1));

    // One effect on what another gave, both on the card; a layer made less solid, which the
    // card makes too, alone and under a blur; and a kind that the card does not make, over one.
    let mut both = glowing.clone();
    let glow = Source::Effect(both.shots[0].effects[0].id);
    put_on(&mut both, "blur", glow, &[("radius", number(3.0))]);
    same(&mut compositor, "a glow, blurred", &both, WorkingSpace::Srgb, 1.0);
    let faded = with("transparency", Source::Layer(ball), &[("amount", number(40.0))]);
    assert_eq!(same(&mut compositor, "a layer less solid", &faded, WorkingSpace::Srgb, 1.0).pixel(60, 30), untouched.pixel(60, 30));
    let mut under = faded.clone();
    let fade = Source::Effect(under.shots[0].effects[0].id);
    put_on(&mut under, "blur", fade, &[("radius", number(4.0))]);
    same(&mut compositor, "less solid, then blurred", &under, WorkingSpace::Linear, 1.0);
    let mut over = blurred.clone();
    let blur = Source::Effect(over.shots[0].effects[0].id);
    put_on(&mut over, "tone", blur, &[("width", number(200.0))]);
    same(&mut compositor, "blurred, then the reference's", &over, WorkingSpace::Srgb, 1.0);
    // A tone by a matte, with its matte.
    let mut toned = plain.clone();
    let tone = put_on(&mut toned, "tone", Source::Layer(ball), &[("width", number(0.0))]);
    Arc::make_mut(&mut toned.shots[0]).effect_mut(tone).unwrap().inputs.insert(iskra_model::MATTE.to_string(), field);
    same(&mut compositor, "a tone by a matte", &toned, WorkingSpace::Srgb, 1.0);
    // A cutter is made only round the ball, and put where it is; at half the size too, and
    // with the ball cut away whole, nothing at all.
    let mut cut = plain.clone();
    let cutter =
        put_on(&mut cut, "cutter", Source::Layer(ball), &[("grow", number(-6.0)), ("blur", number(4.0)), ("inverted", iskra_model::Value::Switch(true))]);
    Arc::make_mut(&mut cut.shots[0]).effect_mut(cutter).unwrap().inputs.insert(iskra_model::MATTE.to_string(), Source::Layer(ball));
    same(&mut compositor, "a cutter, softened", &cut, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "a cutter, softened, at half the size", &cut, WorkingSpace::Linear, 0.5);
    let effect = Arc::make_mut(&mut cut.shots[0]).effect_mut(cutter).unwrap();
    (effect.param_mut("grow").unwrap().value, effect.param_mut("blur").unwrap().value) = (number(0.0), number(0.0));
    effect.param_mut("inverted").unwrap().value = iskra_model::Value::Switch(false);
    let gone = same(&mut compositor, "a cutter that cuts all away", &cut, WorkingSpace::Srgb, 1.0);
    assert_eq!(gone.pixel(60, 30), untouched.pixel(60, 30));
    // Switched off, an effect is not there.
    let mut off = blurred;
    let node = off.shots[0].effects[0].id;
    Arc::make_mut(&mut off.shots[0]).effect_mut(node).unwrap().enabled = false;
    assert_eq!(same(&mut compositor, "an effect switched off", &off, WorkingSpace::Srgb, 1.0).pixels, untouched.pixels);
}

#[test]
fn the_card_corrects_colours_as_the_reference_does() {
    use iskra_model::{Curves, Source, Value};
    let Some(mut compositor) = compositor() else { return };
    let (plain, ball, _) = scene();
    let field = Source::Layer(plain.shots[0].layers[2].id);
    let with = |on: Source, set: &[(&str, Value)]| {
        let mut project = plain.clone();
        put_on(&mut project, "colourCorrection", on, set);
        project
    };
    let number = Value::Number;
    let all = [
        ("black", number(8.0)),
        ("white", number(92.0)),
        ("middle", number(1.4)),
        ("hue", number(120.0)),
        ("saturation", number(-40.0)),
        ("lightness", number(15.0)),
        ("tint", Value::Colour([1.0, 0.6, 0.2, 1.0])),
        ("tintAmount", number(30.0)),
    ];
    let untouched = same(&mut compositor, "no effect", &plain, WorkingSpace::Srgb, 1.0);
    let made = compositor.effects;
    let corrected = same(&mut compositor, "a colour correction", &with(field, &all), WorkingSpace::Srgb, 1.0);
    assert_ne!(corrected, untouched);
    same(&mut compositor, "a colour correction, in linear light and small", &with(field, &all), WorkingSpace::Linear, 0.5);
    same(&mut compositor, "darker, of the ball", &with(Source::Layer(ball), &[("lightness", number(-30.0))]), WorkingSpace::Srgb, 1.0);
    // By curves, the master's and each colour's own.
    let mut curves = Curves::default();
    *curves.points_mut(0) = vec![[0.0, 0.1], [0.4, 0.6], [1.0, 0.9]];
    *curves.points_mut(2) = vec![[0.0, 0.0], [0.5, 0.3], [1.0, 1.0]];
    let curved = with(field, &[("curves", Value::Curves(curves))]);
    same(&mut compositor, "by curves", &curved, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "by curves, in linear light", &curved, WorkingSpace::Linear, 1.0);
    assert_eq!(compositor.effects, (made.0 + 5, made.1), "all made on the card");
    // Drawn again, it is found made.
    let kept = compositor.effects_kept;
    same(&mut compositor, "by curves again", &curved, WorkingSpace::Linear, 1.0);
    assert_eq!((compositor.effects.0, compositor.effects_kept), (made.0 + 5, kept + 1));
}

#[test]
fn the_card_tones_and_highlights_as_the_reference_does() {
    use iskra_model::{MATTE, Source, Value};
    let Some(mut compositor) = compositor() else { return };
    let (plain, ball, _) = scene();
    let field = Source::Layer(plain.shots[0].layers[2].id);
    let number = Value::Number;
    // On the field: a rim inside its edge, or where the ball is, as a matte.
    let with = |kind: &str, set: &[(&str, Value)], matte: bool| {
        let mut project = plain.clone();
        let id = put_on(&mut project, kind, field, set);
        if matte {
            let shot = Arc::make_mut(&mut project.shots[0]);
            shot.effect_mut(id).unwrap().inputs.insert(MATTE.to_string(), Source::Layer(ball));
        }
        project
    };
    let untouched = same(&mut compositor, "no effect", &plain, WorkingSpace::Srgb, 1.0);
    let made = compositor.effects;
    let rim = [("width", number(6.0)), ("softness", number(3.0)), ("amount", number(80.0)), ("colour", Value::Colour([0.1, 0.1, 0.4, 0.9]))];
    let toned = same(&mut compositor, "a tone, a rim", &with("tone", &rim, false), WorkingSpace::Srgb, 1.0);
    assert_ne!(toned, untouched);
    same(&mut compositor, "a highlight, a rim, in linear light and small", &with("highlight", &rim, false), WorkingSpace::Linear, 0.5);
    same(&mut compositor, "a tone, a rim, sharp", &with("tone", &[("width", number(4.0)), ("softness", number(0.0))], false), WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "a tone by a matte made wider", &with("tone", &rim, true), WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "a highlight by a matte, at twice the size", &with("highlight", &rim, true), WorkingSpace::Linear, 2.0);
    assert_eq!(compositor.effects, (made.0 + 5, made.1), "all made on the card");
    // Wider than the card takes, it is the reference's.
    same(&mut compositor, "a tone, a wide rim", &with("tone", &[("width", number(200.0))], false), WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.effects, (made.0 + 5, made.1 + 1));
}

#[test]
fn the_card_casts_a_shadow_as_the_reference_does() {
    use iskra_model::{Source, Value};
    let Some(mut compositor) = compositor() else { return };
    let (plain, ball, _) = scene();
    let number = Value::Number;
    let with = |set: &[(&str, Value)]| {
        let mut project = plain.clone();
        put_on(&mut project, "dropShadow", Source::Layer(ball), set);
        project
    };
    let untouched = same(&mut compositor, "no effect", &plain, WorkingSpace::Srgb, 1.0);
    let made = compositor.effects;
    let shadowed = same(&mut compositor, "a drop shadow", &with(&[]), WorkingSpace::Srgb, 1.0);
    assert_ne!(shadowed, untouched);
    let aside = [("offsetX", number(-12.0)), ("offsetY", number(7.5)), ("softness", number(3.0)), ("colour", Value::Colour([0.3, 0.0, 0.5, 0.8]))];
    same(&mut compositor, "a shadow up and to the left, coloured", &with(&aside), WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "a shadow, in linear light and small", &with(&aside), WorkingSpace::Linear, 0.5);
    same(&mut compositor, "a sharp shadow, at twice the size", &with(&[("softness", number(0.0)), ("opacity", number(100.0))]), WorkingSpace::Srgb, 2.0);
    assert_eq!(compositor.effects, (made.0 + 4, made.1), "all made on the card");
}

#[test]
fn the_card_blurs_along_a_line_and_round_a_centre_as_the_reference_does() {
    use iskra_model::{Source, Value};
    let Some(mut compositor) = compositor() else { return };
    let (plain, ball, _) = scene();
    let number = Value::Number;
    let with = |kind: &str, on: Source, set: &[(&str, Value)]| {
        let mut project = plain.clone();
        put_on(&mut project, kind, on, set);
        project
    };
    let shown = plain.shots[0].display.unwrap();
    let untouched = same(&mut compositor, "no effect", &plain, WorkingSpace::Srgb, 1.0);
    let made = compositor.effects;
    let along = with("directionalBlur", Source::Layer(ball), &[("radius", number(6.0)), ("angle", number(30.0))]);
    assert_ne!(same(&mut compositor, "a directional blur", &along, WorkingSpace::Srgb, 1.0), untouched);
    same(&mut compositor, "a directional blur, in linear light and small", &along, WorkingSpace::Linear, 0.5);
    same(&mut compositor, "a long directional blur of all", &with("directionalBlur", shown, &[("radius", number(40.0))]), WorkingSpace::Srgb, 1.0);
    let spun = with("radialBlur", shown, &[("amount", number(12.0)), ("centreX", number(20.0)), ("centreY", number(-10.0))]);
    same(&mut compositor, "a spin", &spun, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "a spin, in linear light and large", &spun, WorkingSpace::Linear, 1.5);
    let zoom = [("kind", Value::Choice("Zoom".to_string())), ("amount", number(20.0))];
    same(&mut compositor, "a zoom of the ball", &with("radialBlur", Source::Layer(ball), &zoom), WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.effects, (made.0 + 6, made.1), "all made on the card");
}

#[test]
fn the_card_cuts_as_the_reference_does() {
    use iskra_model::{MATTE, Source, Value};
    let Some(mut compositor) = compositor() else { return };
    let (plain, ball, _) = scene();
    let field = Source::Layer(plain.shots[0].layers[2].id);
    // The field cut by the ball, which is drawn over it too.
    let cut = |set: &[(&str, Value)]| {
        let mut project = plain.clone();
        let id = put_on(&mut project, "cutter", field, set);
        let shot = Arc::make_mut(&mut project.shots[0]);
        let node = Arc::make_mut(shot.effects.iter_mut().find(|effect| effect.id == id).unwrap());
        node.inputs.insert(MATTE.to_string(), Source::Layer(ball));
        project
    };
    let untouched = same(&mut compositor, "no effect", &plain, WorkingSpace::Srgb, 1.0);
    let made = compositor.effects;
    let cut_out = same(&mut compositor, "a cutter", &cut(&[]), WorkingSpace::Srgb, 1.0);
    assert_ne!(cut_out, untouched);
    same(&mut compositor, "an inverted cutter", &cut(&[("inverted", Value::Switch(true))]), WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "an inverted cutter, in linear light and small", &cut(&[("inverted", Value::Switch(true))]), WorkingSpace::Linear, 0.5);
    assert_eq!(compositor.effects, (made.0 + 3, made.1), "cut on the card");
    // With its matte grown, narrowed or softened, on the card too.
    same(&mut compositor, "a cutter with its matte grown", &cut(&[("grow", Value::Number(6.0))]), WorkingSpace::Srgb, 1.0);
    assert_eq!(compositor.effects, (made.0 + 4, made.1));
    // Drawn again, it is found made; and so with a layer outside it hidden, but not at
    // another size.
    let (mut again, kept) = (cut(&[]), compositor.effects_kept);
    same(&mut compositor, "a cutter again", &again, WorkingSpace::Srgb, 1.0);
    let outside = again.shots[0].layers.iter().map(|layer| layer.id).find(|id| Source::Layer(*id) != field && *id != ball);
    let shot = Arc::make_mut(&mut again.shots[0]);
    shot.hidden.extend(outside.map(|id| id.0));
    same(&mut compositor, "a cutter, with another layer hidden", &again, WorkingSpace::Srgb, 1.0);
    assert_eq!((compositor.effects.0, compositor.effects_kept), (made.0 + 4, kept + 2), "found made");
    same(&mut compositor, "a cutter inverted again", &cut(&[("inverted", Value::Switch(true))]), WorkingSpace::Srgb, 0.75);
    assert_eq!(compositor.effects.0, made.0 + 5, "made again at another size");
}

#[test]
fn the_card_draws_vector_art_over_bitmap_art_as_the_reference_does() {
    use iskra_model::{ArtContent, ShapeId, ShapeKind, SwatchId, VectorArt, VectorFill, VectorShape, VectorStroke, kurbo::BezPath};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ball, _) = scene();
    // The ball's colour art painted, and its line art made vector: a curve across the fill,
    // its width going from thin to thick, over a fill of its own.
    let shot = Arc::make_mut(&mut project.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
    if let ArtContent::Indexed(surface) = &mut drawing.art_layers[0].content {
        surface.paint(iskra_drawing::Rect::new(-40, -30, 60, 50), &[1.0; 8000], SwatchId(2));
    }
    let mut curve = BezPath::new();
    curve.move_to((-80.0, 20.0));
    curve.curve_to((-30.0, -60.0), (30.0, 60.0), (80.0, -10.0));
    let mut patch = BezPath::new();
    patch.move_to((-20.0, -20.0));
    patch.quad_to((40.0, -40.0), (30.0, 20.0));
    patch.close_path();
    let shapes = vec![
        VectorShape { id: ShapeId(1), swatch: SwatchId(4), kind: ShapeKind::Fill(VectorFill::plain(patch)) },
        // Fainter at its start, its colour drifted further along it.
        VectorShape {
            id: ShapeId(2),
            swatch: SwatchId(3),
            kind: ShapeKind::Stroke(VectorStroke {
                path: curve,
                widths: vec![1.5, 12.0],
                opacities: vec![0.3, 1.0],
                drifts: vec![0.0, 1.0],
                ..Default::default()
            }),
        },
    ];
    drawing.art_layers[1].content = ArtContent::Vector(VectorArt::new(shapes));
    Arc::make_mut(&mut project.palette).swatches.iter_mut().find(|swatch| swatch.id == SwatchId(3)).unwrap().drift = Some([120, 0, -30]);
    same(&mut compositor, "vector line art over bitmap colour art", &project, WorkingSpace::Srgb, 1.0);
    same(&mut compositor, "and with the camera close", &project, WorkingSpace::Srgb, 2.5);
}

#[test]
fn a_drawing_of_vector_art_is_bent_by_its_points_and_drawn_alike_by_both() {
    use iskra_model::{ArtContent, Bone, Deformer, DeformerId, ShapeId, ShapeKind, SwatchId, VectorArt, VectorShape, VectorStroke, kurbo::BezPath};
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ball, _) = scene();
    // The ball's drawing made all vector art: a straight line along the two bones, nothing else.
    let blob = DeformerId(project.new_id());
    let shot = Arc::make_mut(&mut project.shots[0]);
    let bones = vec![Bone::at_rest(0.0, 40.0), Bone { turn: -60.0, ..Bone::at_rest(0.0, 40.0) }];
    shot.deformers.push(Arc::new(Deformer::new(blob, "Blob", [-40.0, 0.0], bones)));
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    layer.deformer = Some(blob);
    let density = layer.density;
    let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
    let mut line = BezPath::new();
    line.move_to((-40.0 * density, 0.0));
    line.line_to((40.0 * density, 0.0));
    let stroke = VectorShape { id: ShapeId(1), swatch: SwatchId(1), kind: ShapeKind::Stroke(VectorStroke::even(line, 6.0)) };
    drawing.art_layers[0].content = ArtContent::Vector(VectorArt::default());
    drawing.art_layers[1].content = ArtContent::Vector(VectorArt::new(vec![stroke]));

    // Seen bent by its points: a copy, with nothing left for the mesh, and the same copy again.
    let seen = |project: &Project| {
        let found = iskra_render::seen(&project.shots[0], 0).into_iter().find(|seen| seen.drawing.art_layers[1].content.is_vector());
        found.map(|seen| (seen.drawing, seen.warp)).expect("the ball")
    };
    let (first, again) = (seen(&project), seen(&project));
    assert!(first.1.is_none() && Arc::ptr_eq(&first.0, &again.0));
    let ArtContent::Vector(bent) = &first.0.art_layers[1].content else { panic!("vector art") };
    let ShapeKind::Stroke(bent) = &bent.shapes()[0].kind else { panic!("a stroke") };
    let end = iskra_model::kurbo::ParamCurve::end(&bent.path.segments().last().unwrap());
    assert!(end.y.abs() > 20.0 * density, "the line's end is turned with the second bone: {end:?}");
    same(&mut compositor, "vector art bent by its points", &project, WorkingSpace::Srgb, 1.0);

    // With pixels on its colour art too, it is bent through the mesh, vector art and all.
    let shot = Arc::make_mut(&mut project.shots[0]);
    let layer = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.id == ball).unwrap());
    let drawing = Arc::make_mut(&mut layer.drawings[0].drawing);
    drawing.art_layers[0].content = ArtContent::Indexed(Default::default());
    if let ArtContent::Indexed(surface) = &mut drawing.art_layers[0].content {
        surface.paint(iskra_drawing::Rect::new(-20, -20, 20, 20), &[1.0; 1600], SwatchId(2));
    }
    assert!(seen(&project).1.is_some(), "a mixed drawing keeps its warp");
    same(&mut compositor, "mixed, bent through the mesh", &project, WorkingSpace::Srgb, 1.0);
}

#[test]
fn the_card_draws_gradients_and_pictures_as_the_reference_does() {
    let Some(mut compositor) = compositor() else { return };
    let (mut project, ..) = scene();
    common::shaded(&mut project);
    for space in [WorkingSpace::Srgb, WorkingSpace::Linear] {
        same(&mut compositor, &format!("shaded, {space:?}"), &project, space, 1.0);
    }
    // Made solid again, the same drawings are drawn as swatches, in their one colour.
    let palette = Arc::make_mut(&mut project.palette);
    palette.swatches.iter_mut().for_each(|swatch| swatch.kind = iskra_model::SwatchKind::Solid);
    let plain = same(&mut compositor, "solid again", &project, WorkingSpace::Srgb, 1.0);
    assert_eq!(plain.pixel(60, 31), [38, 110, 210, 255]);
}
