//! The scene that the compositors are tried on: the reference against its golden pictures,
//! the GPU compositor against the reference.

use std::sync::Arc;

use iskra_drawing::{BrushSettings, Rect, Stroke, StrokePoint, paint_stroke};
use iskra_model::{Animation, ArtContent, DrawingId, IndexedSurface, LayerId, Peg, PegId, PoolDrawing, Project, Role, SwatchId, Transform, WorkingSpace};
use iskra_render::{Image, View, render};

/// A shot of 320 × 180 with drawings at twice that: a blue field, over it a `Mist` layer with
/// a band of white that half covers, and on top a `Ball` layer on the peg `Bounce`: a red blob
/// with a black stroke through it.
pub fn scene() -> (Project, LayerId, PegId) {
    let mut project = Project::default();
    (project.settings.width, project.settings.height) = (320, 180);
    let mut mist = project.new_layer("Mist", Role::Animation);
    let mut ball = project.new_layer("Ball", Role::Animation);
    let band = DrawingId(project.new_id());
    let (field, blob, peg) = (DrawingId(project.new_id()), DrawingId(project.new_id()), PegId(project.new_id()));
    let painted = |paint: &dyn Fn(&mut IndexedSurface)| {
        let mut drawing = iskra_model::Drawing::default();
        if let ArtContent::Indexed(surface) = &mut drawing.art_layers[1].content {
            paint(surface);
        }
        Arc::new(drawing)
    };
    let fill = |surface: &mut IndexedSurface, rect: Rect, coverage: f32, swatch: u16| {
        surface.paint(rect, &vec![coverage; rect.width() as usize * rect.height() as usize], SwatchId(swatch));
    };
    let sky = painted(&|surface| fill(surface, Rect::new(-240, -120, 240, 120), 1.0, 5));
    mist.drawings.push(PoolDrawing { id: band, name: "1".to_string(), drawing: painted(&|surface| fill(surface, Rect::new(-300, -20, 300, 20), 0.5, 2)) });
    mist.exposure.set(0, 48, Some(band));
    let shape = painted(&|surface| {
        fill(surface, Rect::new(-60, -60, 60, 60), 1.0, 3);
        let points = (0..=20).map(|i| StrokePoint::new(-90.0 + 9.0 * f64::from(i), 40.0 * (f64::from(i) * 0.5).sin(), 1.0)).collect();
        paint_stroke(surface, &Stroke { brush: BrushSettings { size: 14.0, ..BrushSettings::default() }, points }, SwatchId(1));
    });
    ball.parent = Some(peg);
    ball.drawings.push(PoolDrawing { id: blob, name: "1".to_string(), drawing: shape });
    ball.exposure.set(0, 48, Some(blob));
    let ball_id = ball.id;
    let shot = Arc::make_mut(&mut project.shots[0]);
    let back = Arc::make_mut(&mut shot.layers[0]);
    back.drawings.push(PoolDrawing { id: field, name: "1".to_string(), drawing: sky });
    back.exposure.set(0, 48, Some(field));
    shot.connect_on_top(mist.id);
    shot.connect_on_top(ball_id);
    shot.layers.insert(0, Arc::new(mist));
    shot.layers.insert(0, Arc::new(ball));
    let transform = Transform { position: [40.0, 10.0], rotation: 30.0, scale: [1.5, 0.75], ..Transform::default() };
    shot.pegs.push(Arc::new(Peg {
        id: peg,
        name: "Bounce".to_string(),
        parent: None,
        transform,
        animation: Animation::default(),
        template: None,
        follows: None,
        inherit: Default::default(),
        matches: Default::default(),
    }));
    (project, ball_id, peg)
}

pub fn shoot(project: &Project, space: WorkingSpace, scale: f64) -> Image {
    render(&project.shots[0], &project.palette, space, 0, &View::frame(&project.settings, scale))
}

/// The scene's swatches made gradients and a picture (plan §4.20): the field's blue a gradient
/// from orange at its top to blue at its bottom, placed so on the field; the mist's white a
/// round one, white in its middle and clear at its edge; and the blob's red a red and yellow
/// checker, tiled, eight drawing pixels to one of its own.
#[allow(dead_code)]
pub fn shaded(project: &mut Project) {
    use iskra_model::{ArtLayerKind, Gradient, GradientShape, GradientStop, MediaId, Picture, Placement, SwatchKind, SwatchTexture};
    let palette = Arc::make_mut(&mut project.palette);
    let mut kind = |id: u16, kind: SwatchKind| palette.swatches.iter_mut().find(|swatch| swatch.id == SwatchId(id)).unwrap().kind = kind;
    let stop = |at, color| GradientStop { at, color };
    kind(5, SwatchKind::Gradient(Gradient { shape: GradientShape::Linear, stops: vec![stop(0.0, [250, 160, 40, 255]), stop(1.0, [38, 110, 210, 255])] }));
    kind(2, SwatchKind::Gradient(Gradient { shape: GradientShape::Radial, stops: vec![stop(0.0, [255, 255, 255, 255]), stop(1.0, [255, 255, 255, 0])] }));
    kind(3, SwatchKind::Texture(SwatchTexture { media: MediaId(77), tile: true, scale: 8.0, once: false }));
    let (red, yellow) = ([220, 50, 47, 255], [250, 220, 60, 255]);
    palette.pictures.insert(MediaId(77), Arc::new(Picture { width: 2, height: 2, pixels: vec![red, yellow, yellow, red] }));
    let shot = Arc::make_mut(&mut project.shots[0]);
    let back = Arc::make_mut(shot.layers.iter_mut().find(|layer| layer.name != "Mist" && layer.name != "Ball").unwrap());
    let field = Arc::make_mut(&mut back.drawings[0].drawing);
    field.placements.push(Placement {
        art: ArtLayerKind::LineArt,
        swatch: SwatchId(5),
        at: [0, 0],
        start: [0.0, -120.0],
        end: [0.0, 120.0],
        across: None,
        focus: [0.0; 2],
    });
}
