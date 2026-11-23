//! The playback cache (plan §4.3, M4): whole frames of the shot, made by the GPU compositor
//! and kept on the card, so that a shot plays at its frame rate however many layers it has.
//! While the shot plays, the viewport shows these and makes the ones just ahead of the
//! playhead; a frame that was made once is shown again without being made again.
//!
//! The frames are of one shot as it is, under one palette, at one size. When any of those
//! changes they are all let go: an edit makes a new shot, and its frames are others.

use std::collections::HashMap;
use std::sync::Arc;

use egui::TextureId;
use egui::mutex::RwLock;
use iskra_gpu::Compositor;
use iskra_model::{Palette, Settings, Shot, WorkingSpace};
use iskra_render::View;

/// How many bytes of frames may be kept until the preferences say: at HD, about ninety frames.
const BUDGET: usize = 768 << 20;

/// What the frames that are kept are of.
#[derive(Clone, PartialEq)]
struct Of {
    shot: Arc<Shot>,
    palette: Arc<Palette>,
    space: WorkingSpace,
    /// The frame's size in the shot's own pixels, and the size the frames are made at.
    resolution: (u32, u32),
    size: (u32, u32),
    /// How opaque the boards are made, where they are faded under the animation as reference.
    boards: Option<u32>,
}

/// What the viewport's picture is of: while it stays the same, the picture does.
#[derive(Clone, PartialEq)]
struct Window {
    shot: Shot,
    palette: Arc<Palette>,
    space: WorkingSpace,
    frame: u32,
    view: View,
    looks: iskra_gpu::Looks,
}

struct Kept {
    id: TextureId,
    /// Held for as long as the window may draw it.
    _texture: wgpu::Texture,
    /// The count of the frame of the screen it was last shown on.
    used: u64,
}

/// What an effect node's picture on the card is kept by: the node, and of a Particles node
/// in sheets, which sheet of how many.
pub type CardKey = (iskra_model::EffectId, Option<(u32, u32)>);

pub struct FrameCache {
    compositor: Compositor,
    /// The window's own renderer, which draws the frames as pictures of the interface.
    renderer: Arc<RwLock<egui_wgpu::Renderer>>,
    of: Option<Of>,
    frames: HashMap<u32, Kept>,
    /// The pictures of what goes through effect nodes while the shot stands still, by the
    /// node: the viewport says what each is of, and which are still wanted.
    cards: HashMap<CardKey, Kept>,
    /// The picture of the viewport as it stands, where it is drawn on the card: the last made,
    /// with what it was made of.
    window: Option<Kept>,
    window_of: Option<Window>,
    /// How many pictures of the viewport have been made, as against shown again.
    pub windows_made: u64,
    shown: u64,
    /// How many frames have been made, as against found already made.
    pub made: u64,
    pub budget: usize,
    /// How opaque the frames made from now on show the boards: faded as reference in their
    /// place among the layers, where the boards are seen with the animation. `None` for as
    /// they are. The viewport says so.
    pub boards: Option<f32>,
}

impl FrameCache {
    /// A cache whose frames are made by `compositor` and drawn by `renderer`, which are on
    /// the same device.
    pub fn new(compositor: Compositor, renderer: Arc<RwLock<egui_wgpu::Renderer>>) -> Self {
        Self {
            compositor,
            renderer,
            of: None,
            frames: HashMap::new(),
            cards: HashMap::new(),
            window: None,
            window_of: None,
            windows_made: 0,
            shown: 0,
            made: 0,
            budget: BUDGET,
            boards: None,
        }
    }

    /// How large frames are made for a view that shows a pixel of the shot over `across`
    /// pixels of the screen: as large as they are seen, in steps of an eighth of the shot's
    /// size so that a small change of zoom keeps them, and never larger than the shot.
    pub fn scale_for(across: f32) -> f64 {
        (f64::from(across) * 8.0).ceil().clamp(1.0, 8.0) / 8.0
    }

    /// Whether the frames that are kept are of this: the shot and the palette as they are,
    /// at this size.
    fn is_of(&self, shot: &Arc<Shot>, palette: &Arc<Palette>, settings: &Settings, scale: f64) -> bool {
        let view = View::frame(settings, scale);
        self.of.as_ref().is_some_and(|of| {
            Arc::ptr_eq(&of.shot, shot)
                && Arc::ptr_eq(&of.palette, palette)
                && (of.space, of.resolution, of.size) == (settings.working_space, (settings.width, settings.height), (view.width, view.height))
                && of.boards == self.boards.map(f32::to_bits)
        })
    }

    /// The frames of this that are made, in their order: none if what is kept is of
    /// something else.
    pub fn made_of(&self, shot: &Arc<Shot>, palette: &Arc<Palette>, settings: &Settings, scale: f64) -> Vec<u32> {
        if !self.is_of(shot, palette, settings, scale) {
            return Vec::new();
        }
        let mut frames: Vec<u32> = self.frames.keys().copied().collect();
        frames.sort_unstable();
        frames
    }

    /// Whether one more frame at this size would be past the budget, so that making it would
    /// let another go.
    pub fn full(&self, settings: &Settings, scale: f64) -> bool {
        let view = View::frame(settings, scale);
        (self.frames.len() + 1) * view.width as usize * view.height as usize * 4 > self.budget
    }

    /// Whether `frame` of what the cache holds is made.
    pub fn has(&self, frame: u32) -> bool {
        self.frames.contains_key(&frame)
    }

    /// How many frames are kept.
    #[cfg(test)]
    pub fn kept(&self) -> usize {
        self.frames.len()
    }

    /// Let every frame go.
    pub fn clear(&mut self) {
        let mut renderer = self.renderer.write();
        for (_, kept) in self.frames.drain() {
            renderer.free_texture(&kept.id);
        }
        self.of = None;
    }

    /// The picture of `frame` of `shot`, at `scale` times the shot's size, to draw over the
    /// camera's frame: found if it was made, else made now. What was kept of another shot,
    /// palette or size is let go first.
    pub fn frame(&mut self, shot: &Arc<Shot>, palette: &Arc<Palette>, settings: &Settings, scale: f64, frame: u32) -> TextureId {
        let view = View::frame(settings, scale);
        if !self.is_of(shot, palette, settings, scale) {
            self.clear();
            let resolution = (settings.width, settings.height);
            let boards = self.boards.map(f32::to_bits);
            self.of =
                Some(Of { shot: shot.clone(), palette: palette.clone(), space: settings.working_space, resolution, size: (view.width, view.height), boards });
        }
        self.shown += 1;
        if let Some(kept) = self.frames.get_mut(&frame) {
            kept.used = self.shown;
            return kept.id;
        }
        self.made += 1;
        // The boards faded where they lie among the layers, over or under the animation.
        let made = match self.boards {
            Some(opacity) => {
                let mut looks = iskra_gpu::Looks::default();
                for layer in shot.layers.iter().filter(|layer| shot.is_board(layer.id)) {
                    looks.layers.insert(layer.id, iskra_gpu::Look { opacity, tint: None });
                }
                self.compositor.render_looked(shot, palette, settings.working_space, frame, &view, &looks)
            }
            None => self.compositor.render(shot, palette, settings.working_space, frame, &view),
        };
        let texture = self.compositor.shown(&made);
        let id = self.renderer.write().register_native_texture(
            self.compositor.device(),
            &texture.create_view(&wgpu::TextureViewDescriptor::default()),
            wgpu::FilterMode::Linear,
        );
        self.frames.insert(frame, Kept { id, _texture: texture, used: self.shown });
        // Past the budget, the frames shown longest ago go.
        let each = view.width as usize * view.height as usize * 4;
        while self.frames.len() > 1 && self.frames.len() * each > self.budget {
            let Some(oldest) = self.frames.iter().filter(|(at, _)| **at != frame).min_by_key(|(_, kept)| kept.used).map(|(at, _)| *at) else { break };
            if let Some(kept) = self.frames.remove(&oldest) {
                self.renderer.write().free_texture(&kept.id);
            }
        }
        id
    }
}

impl FrameCache {
    /// The picture of what goes through the effect node `effect` on `frame` of `shot`, which
    /// is `card` of what the shot shows there, as `view` sees it: made now, on the card, in
    /// place of the one that was kept of that node.
    pub fn card(
        &mut self,
        effect: CardKey,
        shot: &Shot,
        palette: &Arc<Palette>,
        space: WorkingSpace,
        frame: u32,
        view: &View,
        card: &iskra_eval::Card,
    ) -> TextureId {
        let made = self.compositor.card(shot, palette, space, frame, view, card);
        let texture = self.compositor.shown(&made);
        let mut renderer = self.renderer.write();
        let id =
            renderer.register_native_texture(self.compositor.device(), &texture.create_view(&wgpu::TextureViewDescriptor::default()), wgpu::FilterMode::Linear);
        if let Some(before) = self.cards.insert(effect, Kept { id, _texture: texture, used: 0 }) {
            renderer.free_texture(&before.id);
        }
        id
    }

    /// The picture of the viewport while the shot stands still: what `shot` shows on `frame`
    /// as `view` sees it, with what only the window shows, `looks`. Made now, in place of
    /// the one made before, unless that one is of the same: then it is shown again.
    pub fn window(&mut self, shot: &Shot, palette: &Arc<Palette>, space: WorkingSpace, frame: u32, view: &View, looks: &iskra_gpu::Looks) -> TextureId {
        let of = Window { shot: shot.clone(), palette: palette.clone(), space, frame, view: *view, looks: looks.clone() };
        if let (Some(kept), Some(was)) = (&self.window, &self.window_of)
            && was.shot == of.shot
            && Arc::ptr_eq(&was.palette, &of.palette)
            && (was.space, was.frame, was.view) == (of.space, of.frame, of.view)
            && was.looks == of.looks
        {
            return kept.id;
        }
        self.window_of = Some(of);
        self.windows_made += 1;
        let made = self.compositor.render_looked(shot, palette, space, frame, view, looks);
        let texture = self.compositor.shown(&made);
        let mut renderer = self.renderer.write();
        let id =
            renderer.register_native_texture(self.compositor.device(), &texture.create_view(&wgpu::TextureViewDescriptor::default()), wgpu::FilterMode::Linear);
        if let Some(before) = self.window.replace(Kept { id, _texture: texture, used: 0 }) {
            renderer.free_texture(&before.id);
        }
        id
    }

    /// The viewport's picture as it was last made: its pixels, in rows from the top.
    #[cfg(test)]
    pub fn window_pixels(&self) -> Option<Vec<[u8; 4]>> {
        self.compositor.read_shown(&self.window.as_ref()?._texture)
    }

    /// How many tiles of drawings the compositor has sent to the card.
    #[cfg(test)]
    pub fn tiles_sent(&self) -> u64 {
        self.compositor.sent
    }

    /// Let go the pictures of the effect nodes that `wanted` does not name.
    pub fn keep_cards(&mut self, wanted: impl Fn(CardKey) -> bool) {
        let mut renderer = self.renderer.write();
        self.cards.retain(|effect, kept| {
            let keep = wanted(*effect);
            if !keep {
                renderer.free_texture(&kept.id);
            }
            keep
        });
    }
}

impl Drop for FrameCache {
    fn drop(&mut self) {
        self.keep_cards(|_| false);
        if let Some(window) = self.window.take() {
            self.renderer.write().free_texture(&window.id);
        }
        self.clear();
    }
}
