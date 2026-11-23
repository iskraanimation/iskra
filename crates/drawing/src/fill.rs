//! The paint bucket (plan §4.8): fills an area of colour art with a swatch, out to the lines
//! of the line art.
//!
//! The area is what can be reached from the click without crossing a line or paint of another
//! swatch. Gaps in the lines are closed by filling only where a square as wide as the gap
//! fits, and then letting the fill spread back to the lines; the fill then runs a little way
//! under the lines, so that no light shows between a line's soft edge and its fill.

use std::collections::VecDeque;

use photocraft_geom::Rect;

use crate::{IndexedPixel, IndexedSurface, SwatchId};

/// A pixel covered at least this much is a line, or is painted: half.
const HALF: u16 = 32768;

/// How the paint bucket fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    /// Gaps in the lines up to this many pixels wide are closed.
    pub gap: u32,
    /// How many pixels the fill runs under the lines around it.
    pub under: u32,
}

/// Why nothing was filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillError {
    /// The click is on a line.
    OnLine,
    /// The lines around the click don't close: the fill would run out over the whole drawing.
    Open,
    /// The click is on paint of a swatch that is protected.
    Protected,
    /// The click, with the bucket on lines, is on no line.
    NoLine,
}

impl Default for Fill {
    fn default() -> Self {
        Self { gap: 0, under: 2 }
    }
}

impl std::fmt::Display for FillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FillError::OnLine => "that is on a line: click inside the area to fill",
            FillError::Open => "the lines around that don't close: draw the gap shut, or close wider gaps",
            FillError::Protected => "that is painted with a swatch that is protected",
            FillError::NoLine => "there is no line there: click on a line to give it the swatch",
        })
    }
}

/// The swatch a pixel of colour art counts as painted with, or none.
fn painted(pixel: IndexedPixel) -> SwatchId {
    if pixel.coverage >= HALF { pixel.swatch } else { SwatchId::NONE }
}

/// Fill the area of `colour` around `at` with `swatch`, bounded by the lines of `line`.
/// [`SwatchId::NONE`] takes the paint of the area away. Returns the rectangle that changed,
/// which is empty where the area had that swatch already.
pub fn fill(colour: &mut IndexedSurface, line: &IndexedSurface, at: (i32, i32), swatch: SwatchId, options: &Fill) -> Result<Rect, FillError> {
    fill_protecting(colour, line, at, swatch, options, &[])
}

/// [`fill`], refusing an area that is painted with one of the `protected` swatches. Paint of
/// another swatch bounds a fill anyway, so nothing else of theirs is ever filled over.
pub fn fill_protecting(
    colour: &mut IndexedSurface,
    line: &IndexedSurface,
    at: (i32, i32),
    swatch: SwatchId,
    options: &Fill,
    protected: &[SwatchId],
) -> Result<Rect, FillError> {
    let reach = options.gap.div_ceil(2) as i32;
    let drawn = line.bounds().union(&colour.bounds());
    if !drawn.contains(at.0, at.1) {
        return if swatch == SwatchId::NONE { Ok(Rect::EMPTY) } else { Err(FillError::Open) };
    }
    // A margin of pixels with nothing on them, wide enough that a fill that gets out of the
    // lines reaches its edge even where only the widest square fits.
    let margin = reach + 2;
    let area = Rect::new(drawn.x0 - margin, drawn.y0 - margin, drawn.x1 + margin, drawn.y1 + margin);
    let (width, height) = (area.width() as usize, area.height() as usize);
    let index = |x: i32, y: i32| (y - area.y0) as usize * width + (x - area.x0) as usize;

    let lines = line.pixels(area);
    let paint = colour.pixels(area);
    let seed = index(at.0, at.1);
    if lines[seed].coverage >= HALF {
        return Err(FillError::OnLine);
    }
    let before = painted(paint[seed]);
    if protected.contains(&before) {
        return Err(FillError::Protected);
    }
    if before == swatch {
        return Ok(Rect::EMPTY);
    }
    // Where the fill may go: not onto a line, and only over what the click was on.
    let free: Vec<bool> = lines.iter().zip(&paint).map(|(line, paint)| line.coverage < HALF && painted(*paint) == before).collect();

    let grid = Grid { width, height };
    let region = match reach {
        0 => grid.flood(&free, seed).ok_or(FillError::Open)?,
        _ => grid.closing_gaps(&free, seed, reach as usize).ok_or(FillError::Open)?,
    };
    let region = grid.spread(region, options.under as usize, |i| lines[i].coverage >= HALF);
    let region = grid.climb(region, |i| lines[i].coverage);

    let changed = bounds_of(&region, width, area);
    let pixel = if swatch == SwatchId::NONE { IndexedPixel::default() } else { IndexedPixel { swatch, coverage: u16::MAX } };
    colour.set_where(area, &region, pixel);
    if swatch == SwatchId::NONE {
        colour.prune();
    }
    Ok(changed)
}

/// How far apart two colours of free colour may be, in each of red, green, blue and alpha,
/// to be one area to the paint bucket: the soft edge of a stroke is of its stroke.
const ALIKE: u8 = 24;

/// Whether two pixels of free colour are of one area: both nothing, or near enough alike.
fn alike(a: [u8; 4], b: [u8; 4]) -> bool {
    (a[3] == 0 && b[3] == 0) || a.iter().zip(&b).all(|(a, b)| a.abs_diff(*b) <= ALIKE)
}

/// [`fill`] on colour art that is free colour: the area around `at` whose colour is like
/// the colour there, bounded by the lines of `line`, is painted `color` (sRGB, straight
/// alpha), or with no alpha is taken away. Returns the rectangle that changed, which is
/// empty where the area is of that colour already.
pub fn fill_free(colour: &mut photocraft_raster::Surface, line: &IndexedSurface, at: (i32, i32), color: [u8; 4], options: &Fill) -> Result<Rect, FillError> {
    fill_free_by(colour, line, at, color, options, None)
}

/// [`fill_free`] with a gradient or a picture (plan §4.20): the area is painted in the colours
/// `shade` has across it, fitted to it; `color` is its colour as one, by which an area that is
/// of it already is left as it is.
pub fn fill_free_shaded(
    colour: &mut photocraft_raster::Surface,
    line: &IndexedSurface,
    at: (i32, i32),
    color: [u8; 4],
    options: &Fill,
    shade: &dyn Fn(i32, i32, Rect) -> [u8; 4],
) -> Result<Rect, FillError> {
    fill_free_by(colour, line, at, color, options, Some(shade))
}

fn fill_free_by(
    colour: &mut photocraft_raster::Surface,
    line: &IndexedSurface,
    at: (i32, i32),
    color: [u8; 4],
    options: &Fill,
    shade: Option<&dyn Fn(i32, i32, Rect) -> [u8; 4]>,
) -> Result<Rect, FillError> {
    let color = if color[3] == 0 { [0; 4] } else { color };
    let reach = options.gap.div_ceil(2) as i32;
    let drawn = line.bounds().union(&colour.content_bounds());
    if !drawn.contains(at.0, at.1) {
        return if color[3] == 0 { Ok(Rect::EMPTY) } else { Err(FillError::Open) };
    }
    let margin = reach + 2;
    let area = Rect::new(drawn.x0 - margin, drawn.y0 - margin, drawn.x1 + margin, drawn.y1 + margin);
    let (width, height) = (area.width() as usize, area.height() as usize);
    let lines = line.pixels(area);
    let mut paint = vec![[0u8; 4]; width * height];
    colour.read_rgba8_into(area, &mut paint);
    let seed = (at.1 - area.y0) as usize * width + (at.0 - area.x0) as usize;
    if lines[seed].coverage >= HALF {
        return Err(FillError::OnLine);
    }
    let before = paint[seed];
    if alike(before, color) {
        return Ok(Rect::EMPTY);
    }
    let free: Vec<bool> = lines.iter().zip(&paint).map(|(line, paint)| line.coverage < HALF && alike(*paint, before)).collect();
    let grid = Grid { width, height };
    let region = match reach {
        0 => grid.flood(&free, seed).ok_or(FillError::Open)?,
        _ => grid.closing_gaps(&free, seed, reach as usize).ok_or(FillError::Open)?,
    };
    let region = grid.spread(region, options.under as usize, |i| lines[i].coverage >= HALF);
    let region = grid.climb(region, |i| lines[i].coverage);
    let changed = bounds_of(&region, width, area);
    for (i, _) in region.iter().enumerate().filter(|(_, inside)| **inside) {
        let (x, y) = (area.x0 + (i % width) as i32, area.y0 + (i / width) as i32);
        paint[i] = shade.map_or(color, |shade| shade(x, y, changed));
    }
    // Only what changed is written, so that no tile is made where nothing is.
    if !changed.is_empty() {
        let rows = (changed.y0..changed.y1).flat_map(|y| {
            let start = (y - area.y0) as usize * width + (changed.x0 - area.x0) as usize;
            paint[start..start + changed.width() as usize].iter().copied()
        });
        colour.write_interleaved(changed, rows.collect::<Vec<[u8; 4]>>().as_flattened());
        colour.prune();
    }
    Ok(changed)
}

/// Give the line of `line` under `at` the swatch `swatch` (the bucket on line art): the
/// pixels painted with the swatch found there that touch one another, up, down, sideways
/// and corner to corner, each with the coverage it has, so the line keeps its soft edge.
/// [`SwatchId::NONE`] takes the line away. Returns the rectangle that changed, which is empty
/// where the line had that swatch already.
pub fn recolour_line(line: &mut IndexedSurface, at: (i32, i32), swatch: SwatchId, protected: &[SwatchId]) -> Result<Rect, FillError> {
    let area = line.bounds();
    if !area.contains(at.0, at.1) {
        return Err(FillError::NoLine);
    }
    let (width, height) = (area.width() as usize, area.height() as usize);
    let pixels = line.pixels(area);
    let seed = (at.1 - area.y0) as usize * width + (at.0 - area.x0) as usize;
    let before = pixels[seed].swatch;
    if pixels[seed].coverage == 0 {
        return Err(FillError::NoLine);
    }
    if protected.contains(&before) {
        return Err(FillError::Protected);
    }
    if before == swatch {
        return Ok(Rect::EMPTY);
    }
    let of_line = |i: usize| pixels[i].coverage > 0 && pixels[i].swatch == before;
    let mut region = vec![false; pixels.len()];
    let mut queue = VecDeque::from([seed]);
    region[seed] = true;
    let mut changed = Rect::EMPTY;
    while let Some(i) = queue.pop_front() {
        let (x, y) = (i % width, i / width);
        changed = changed.union(&Rect::new(area.x0 + x as i32, area.y0 + y as i32, area.x0 + x as i32 + 1, area.y0 + y as i32 + 1));
        for (nx, ny) in (y.saturating_sub(1)..=(y + 1).min(height - 1)).flat_map(|ny| (x.saturating_sub(1)..=(x + 1).min(width - 1)).map(move |nx| (nx, ny))) {
            let next = ny * width + nx;
            if !region[next] && of_line(next) {
                region[next] = true;
                queue.push_back(next);
            }
        }
    }
    line.swatch_where(area, &region, swatch);
    if swatch == SwatchId::NONE {
        line.prune();
    }
    Ok(changed)
}

/// The bounds of the pixels of `region`, which is `width` pixels a row and lies at `area`.
fn bounds_of(region: &[bool], width: usize, area: Rect) -> Rect {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (y, row) in region.chunks(width).enumerate() {
        if let (Some(first), Some(last)) = (row.iter().position(|inside| *inside), row.iter().rposition(|inside| *inside)) {
            (x0, y0, x1, y1) = (x0.min(first), y0.min(y), x1.max(last), y1.max(y));
        }
    }
    if x0 == usize::MAX {
        return Rect::EMPTY;
    }
    Rect::new(area.x0 + x0 as i32, area.y0 + y0 as i32, area.x0 + x1 as i32 + 1, area.y0 + y1 as i32 + 1)
}

/// The pixels of the area a fill looks at, in rows.
struct Grid {
    width: usize,
    height: usize,
}

impl Grid {
    /// The pixels where a square reaching `reach` pixels each way is free all over. What is
    /// past the edge counts as free.
    fn erode(&self, free: &[bool], reach: usize) -> Vec<bool> {
        if reach == 0 {
            return free.to_vec();
        }
        let walls: Vec<bool> = free.iter().map(|free| !free).collect();
        self.near(&walls, reach).into_iter().map(|near| !near).collect()
    }

    /// The pixels within `reach` of one of `set`, across, down or corner to corner: the
    /// squares of that reach round each.
    fn dilate(&self, set: &[bool], reach: usize) -> Vec<bool> {
        self.near(set, reach)
    }

    /// Whether one of `set` is within `reach` of each pixel, across and down at once (the
    /// square round it): along the rows and then down the columns, each way, a row at a time
    /// in the order the pixels lie in, keeping only whether one was met within reach.
    fn near(&self, set: &[bool], reach: usize) -> Vec<bool> {
        let (width, height) = (self.width, self.height);
        let mut rows = vec![false; set.len()];
        for y in 0..height {
            let (from, out) = (&set[y * width..(y + 1) * width], &mut rows[y * width..(y + 1) * width]);
            let mut since = usize::MAX;
            for x in 0..width {
                since = if from[x] { 0 } else { since.saturating_add(1) };
                out[x] = since <= reach;
            }
            since = usize::MAX;
            for x in (0..width).rev() {
                since = if from[x] { 0 } else { since.saturating_add(1) };
                out[x] |= since <= reach;
            }
        }
        let mut out = vec![false; set.len()];
        let mut since = vec![usize::MAX; width];
        for y in 0..height {
            for x in 0..width {
                since[x] = if rows[y * width + x] { 0 } else { since[x].saturating_add(1) };
                out[y * width + x] = since[x] <= reach;
            }
        }
        since.fill(usize::MAX);
        for y in (0..height).rev() {
            for x in 0..width {
                since[x] = if rows[y * width + x] { 0 } else { since[x].saturating_add(1) };
                out[y * width + x] |= since[x] <= reach;
            }
        }
        out
    }

    /// The pixels of `free` that can be reached from `seed`, going up, down and sideways.
    /// `None` when the edge of the area is reached: the fill got out.
    fn flood(&self, free: &[bool], seed: usize) -> Option<Vec<bool>> {
        let mut region = vec![false; free.len()];
        if !free[seed] {
            return Some(region);
        }
        let mut stack = vec![seed];
        region[seed] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % self.width, i / self.width);
            if x == 0 || y == 0 || x + 1 == self.width || y + 1 == self.height {
                return None;
            }
            for next in [i - 1, i + 1, i - self.width, i + self.width] {
                if free[next] && !region[next] {
                    region[next] = true;
                    stack.push(next);
                }
            }
        }
        Some(region)
    }

    /// The pixels of `free` that can be reached from `seed` with gaps closed: all that the
    /// squares reaching `reach` pixels each way cover where they fit, and the places too
    /// narrow for the square that lead nowhere else, a pointed end or a corner. A narrow place
    /// that also meets what such a square covers from another side is a gap, or leads to one:
    /// of it only the gap is filled, out to the line's far edge, and the fill neither bulges
    /// through it nor runs along a corridor beyond it.
    /// `None` when the fill gets out.
    fn closing_gaps(&self, free: &[bool], seed: usize, reach: usize) -> Option<Vec<bool>> {
        // What is closed in without gaps closed is the most a fill can be with them: worked
        // out within its bounds alone, where nothing else is free, it is quicker on a large
        // drawing, and what lies across a line can't take a narrow place for a gap.
        let Some(reached) = self.flood(free, seed) else { return self.closing_gaps_in(free, seed, reach) };
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for i in (0..free.len()).filter(|&i| reached[i]) {
            let (x, y) = (i % self.width, i / self.width);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        // With a margin of nothing free, so that the edge counts as a line.
        let margin = reach + 2;
        let inner = Grid { width: x1 - x0 + 1 + 2 * margin, height: y1 - y0 + 1 + 2 * margin };
        let outer = |x: usize, y: usize| (y + y0 - margin) * self.width + x + x0 - margin;
        let mut part = vec![false; inner.width * inner.height];
        for y in margin..inner.height - margin {
            for x in margin..inner.width - margin {
                part[y * inner.width + x] = reached[outer(x, y)];
            }
        }
        let (sx, sy) = (seed % self.width, seed / self.width);
        let filled = inner.closing_gaps_in(&part, (sy - y0 + margin) * inner.width + sx - x0 + margin, reach)?;
        let mut region = vec![false; free.len()];
        for (i, _) in filled.iter().enumerate().filter(|(_, filled)| **filled) {
            region[outer(i % inner.width, i / inner.width)] = true;
        }
        Some(region)
    }

    /// [`Grid::closing_gaps`] over the whole of the grid.
    fn closing_gaps_in(&self, free: &[bool], seed: usize, reach: usize) -> Option<Vec<bool>> {
        let wide = self.erode(free, reach);
        // Where the square fits around the click; from a click in a narrow place, the nearest
        // such part that is closed in.
        let core = if wide[seed] {
            self.flood(&wide, seed)?
        } else {
            let thin: Vec<bool> = free.iter().zip(&wide).map(|(free, wide)| *free && !*wide).collect();
            let around = self.flood_within(&thin, seed);
            let beside = (0..free.len()).filter(|&i| wide[i] && self.neighbours(i).any(|next| around[next]));
            let mut tried = vec![false; free.len()];
            let mut found = None;
            for start in beside {
                if tried[start] {
                    continue;
                }
                match self.flood(&wide, start) {
                    Some(part) => {
                        found = Some(part);
                        break;
                    }
                    // What is open is the outside, however often it is met.
                    None => self.flood_within(&wide, start).iter().zip(&mut tried).for_each(|(part, tried)| *tried |= *part),
                }
            }
            // No wide part at all: the narrow place by itself, if it is closed.
            match found {
                Some(part) => part,
                None => return self.flood(free, seed),
            }
        };
        // What the squares cover where they fit, here and elsewhere: the far side of any gap
        // is what they cover from elsewhere. A square that fits is free all over.
        let elsewhere: Vec<bool> = wide.iter().zip(&core).map(|(wide, core)| *wide && !*core).collect();
        let mut filled = self.dilate(&core, reach);
        let beyond = self.dilate(&elsewhere, reach);
        // The narrow places, each taken whole where it meets the fill and nothing beyond.
        let narrow: Vec<bool> = (0..free.len()).map(|i| free[i] && !filled[i] && !beyond[i]).collect();
        let mut seen = vec![false; free.len()];
        for start in 0..free.len() {
            if !narrow[start] || seen[start] {
                continue;
            }
            seen[start] = true;
            let (mut part, mut stack) = (Vec::new(), vec![start]);
            let (mut meets, mut leads_on) = (false, false);
            while let Some(i) = stack.pop() {
                part.push(i);
                for next in self.neighbours(i) {
                    if narrow[next] {
                        if !seen[next] {
                            seen[next] = true;
                            stack.push(next);
                        }
                    } else {
                        meets |= filled[next];
                        leads_on |= beyond[next];
                    }
                }
            }
            if meets && !leads_on {
                part.iter().for_each(|&i| filled[i] = true);
            }
        }
        // And the gap itself, out to the line's far edge: the narrow pixels a line crosses on
        // both sides within the gap's width, taken from the fill a step at a time up, down and
        // sideways, so that nothing past the line is.
        let mut stack: Vec<usize> = (0..free.len()).filter(|&i| filled[i] && self.neighbours(i).any(|next| narrow[next] && !filled[next])).collect();
        while let Some(i) = stack.pop() {
            for next in self.neighbours(i) {
                if narrow[next] && !filled[next] && self.bridged(free, next, 2 * reach) {
                    filled[next] = true;
                    stack.push(next);
                }
            }
        }
        Some(filled)
    }

    /// Whether a line lies on both sides of a pixel, across or down, with no more than `span`
    /// free pixels between: whether it is in a gap of a line that wide. Not corner to corner,
    /// where a line's end and another line beside it would bridge the way out.
    fn bridged(&self, free: &[bool], i: usize, span: usize) -> bool {
        let (x, y) = ((i % self.width) as i64, (i / self.width) as i64);
        // How many steps from the pixel the first one that is not free is, that way.
        let wall = |dx: i64, dy: i64| {
            (1..=span as i64 + 1).find(|step| {
                let (nx, ny) = (x + dx * step, y + dy * step);
                nx >= 0 && ny >= 0 && nx < self.width as i64 && ny < self.height as i64 && !free[ny as usize * self.width + nx as usize]
            })
        };
        [(1, 0), (0, 1)]
            .into_iter()
            .any(|(dx, dy)| matches!((wall(dx, dy), wall(-dx, -dy)), (Some(ahead), Some(behind)) if (ahead + behind - 1) as usize <= span))
    }

    /// The pixels up, down and to the sides of one, where the area has them.
    fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> {
        let (x, y, width, height) = (i % self.width, i / self.width, self.width, self.height);
        let sides = [(x > 0).then(|| i - 1), (x + 1 < width).then(|| i + 1), (y > 0).then(|| i - width), (y + 1 < height).then(|| i + width)];
        sides.into_iter().flatten()
    }

    /// [`Grid::flood`], stopping at the edge of the area and not minding it.
    fn flood_within(&self, free: &[bool], seed: usize) -> Vec<bool> {
        let mut region = vec![false; free.len()];
        if !free[seed] {
            return region;
        }
        region[seed] = true;
        let mut stack = vec![seed];
        while let Some(i) = stack.pop() {
            for next in self.neighbours(i) {
                if free[next] && !region[next] {
                    region[next] = true;
                    stack.push(next);
                }
            }
        }
        region
    }

    /// The pixels of `region` that lie on its edge, which are all that can grow it.
    fn edge(&self, region: &[bool]) -> Vec<usize> {
        let width = self.width;
        let on_edge = |i: usize| {
            let (x, y) = (i % width, i / width);
            x == 0 || y == 0 || x + 1 == width || y + 1 == self.height || !(region[i - 1] && region[i + 1] && region[i - width] && region[i + width])
        };
        (0..region.len()).filter(|&i| region[i] && on_edge(i)).collect()
    }

    /// The pixels around `i`, corners too, that are on the grid.
    fn around(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let (x, y) = ((i % self.width) as i64, (i / self.width) as i64);
        [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)]
            .into_iter()
            .map(move |(dx, dy)| (x + dx, y + dy))
            .filter(|&(nx, ny)| nx >= 0 && ny >= 0 && nx < self.width as i64 && ny < self.height as i64)
            .map(|(nx, ny)| ny as usize * self.width + nx as usize)
    }

    /// `region` grown by `steps` pixels, corners too, into the pixels `allowed` lets it into.
    fn spread(&self, mut region: Vec<bool>, steps: usize, allowed: impl Fn(usize) -> bool) -> Vec<bool> {
        if steps == 0 {
            return region;
        }
        let mut edge: VecDeque<(usize, usize)> = self.edge(&region).into_iter().map(|i| (i, 0)).collect();
        while let Some((i, step)) = edge.pop_front() {
            if step == steps {
                continue;
            }
            for next in self.around(i) {
                if !region[next] && allowed(next) {
                    region[next] = true;
                    edge.push_back((next, step + 1));
                }
            }
        }
        region
    }

    /// `region` grown up the lines around it, from each pixel to those beside it that the
    /// lines cover more, so that it runs under a line to its middle, where its coverage stops
    /// rising, however soft the line's edge. It goes no further than the middle, so a fill
    /// on each side of a line meets the other under it.
    fn climb(&self, mut region: Vec<bool>, coverage: impl Fn(usize) -> u16) -> Vec<bool> {
        let mut edge: VecDeque<usize> = self.edge(&region).into();
        while let Some(i) = edge.pop_front() {
            let here = coverage(i);
            for next in self.around(i) {
                if !region[next] && coverage(next) > here {
                    region[next] = true;
                    edge.push_back(next);
                }
            }
        }
        region
    }
}

#[cfg(test)]
mod line_tests {
    use photocraft_paint::{BrushSettings, Stroke, StrokePoint};

    use super::*;
    use crate::paint_stroke;

    #[test]
    fn the_bucket_fills_free_colour_inside_the_lines_and_inside_a_colour() {
        let brush = BrushSettings { size: 6.0, pressure_size: false, ..Default::default() };
        let mut line = IndexedSurface::new();
        paint_stroke(
            &mut line,
            &crate::Shape::Rectangle.stroke(&brush, crate::Point::new(100.0, 100.0), crate::Point::new(300.0, 200.0), Default::default()),
            SwatchId(1),
        );
        let mut colour = crate::free_colour();
        let pixel = |colour: &photocraft_raster::Surface, x: i32, y: i32| {
            let mut out = [[0u8; 4]];
            colour.read_rgba8_into(Rect::new(x, y, x + 1, y + 1), &mut out);
            out[0]
        };
        // Inside the lines it is filled, out to them and a little under; outside it runs out.
        let red = [220, 50, 47, 255];
        let changed = fill_free(&mut colour, &line, (200, 150), red, &Fill::default()).unwrap();
        assert_eq!((changed, pixel(&colour, 105, 105), pixel(&colour, 50, 150)), (Rect::new(101, 101, 299, 199), red, [0, 0, 0, 0]));
        assert_eq!(fill_free(&mut colour, &line, (50, 150), red, &Fill::default()), Err(FillError::Open));
        assert_eq!(fill_free(&mut colour, &line, (100, 150), red, &Fill::default()), Err(FillError::OnLine));
        assert_eq!(fill_free(&mut colour, &line, (200, 150), [222, 52, 45, 255], &Fill::default()), Ok(Rect::EMPTY), "it is that colour already, near enough");
        // A patch of another colour in it is an area of its own, and bounds the red around it.
        crate::fill_rgba(&mut colour, Rect::new(150, 120, 200, 170), &[1.0; 2500], [38, 110, 210, 255]);
        let green = [64, 160, 43, 255];
        fill_free(&mut colour, &line, (160, 130), green, &Fill::default()).unwrap();
        assert_eq!((pixel(&colour, 199, 169), pixel(&colour, 201, 150), pixel(&colour, 149, 150)), (green, red, red));
        // With no alpha, the area is taken away.
        fill_free(&mut colour, &line, (250, 150), [9, 9, 9, 0], &Fill::default()).unwrap();
        assert_eq!((pixel(&colour, 250, 150), pixel(&colour, 160, 130)), ([0, 0, 0, 0], green));
    }

    #[test]
    fn the_bucket_on_line_art_gives_one_line_another_swatch() {
        let brush = BrushSettings { size: 6.0, pressure_size: false, ..Default::default() };
        let stroke = |from: (f64, f64), to: (f64, f64)| {
            let point = |at: (f64, f64)| StrokePoint { x: at.0, y: at.1, pressure: 1.0, ..Default::default() };
            Stroke { brush: brush.clone(), points: vec![point(from), point(to)] }
        };
        // A cross of two lines of one swatch, a third of another over its end, and a line apart.
        let mut line = IndexedSurface::new();
        paint_stroke(&mut line, &stroke((0.0, 50.0), (100.0, 50.0)), SwatchId(1));
        paint_stroke(&mut line, &stroke((50.0, 0.0), (50.0, 100.0)), SwatchId(1));
        paint_stroke(&mut line, &stroke((100.0, 30.0), (100.0, 70.0)), SwatchId(2));
        paint_stroke(&mut line, &stroke((0.0, 200.0), (100.0, 200.0)), SwatchId(1));
        let soft = line.pixel(20, 47).coverage;
        let swatch = |line: &IndexedSurface, x, y| line.pixel(x, y).swatch.0;

        let changed = recolour_line(&mut line, (20, 50), SwatchId(3), &[]).unwrap();
        assert_eq!((swatch(&line, 20, 50), swatch(&line, 50, 90), swatch(&line, 100, 60), swatch(&line, 50, 200)), (3, 3, 2, 1), "what touches, of its swatch");
        assert_eq!((line.pixel(20, 47).coverage, changed.x0, changed.y1), (soft, -5, 105), "the soft edge is as it was");
        assert_eq!(recolour_line(&mut line, (20, 50), SwatchId(3), &[]), Ok(Rect::EMPTY));
        assert_eq!(recolour_line(&mut line, (20, 80), SwatchId(4), &[]), Err(FillError::NoLine));
        assert_eq!(recolour_line(&mut line, (500, 500), SwatchId(4), &[]), Err(FillError::NoLine));
        assert_eq!(recolour_line(&mut line, (100, 60), SwatchId(4), &[SwatchId(2)]), Err(FillError::Protected));
        // Unpainted, the line is gone, and the others are there.
        recolour_line(&mut line, (50, 200), SwatchId::NONE, &[]).unwrap();
        assert_eq!((line.pixel(50, 200).coverage, swatch(&line, 20, 50), line.bounds().y1), (0, 3, 105));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: SwatchId = SwatchId(1);
    const SKIN: SwatchId = SwatchId(2);
    const SHIRT: SwatchId = SwatchId(3);

    /// A box of lines `thick` pixels wide, its outside from (0, 0) to (`size`, `size`).
    fn boxed(size: i32, thick: i32) -> IndexedSurface {
        let mut line = IndexedSurface::new();
        let mut bar = |rect: Rect| line.paint(rect, &vec![1.0; (rect.width() * rect.height()) as usize], LINE);
        bar(Rect::new(0, 0, size, thick));
        bar(Rect::new(0, size - thick, size, size));
        bar(Rect::new(0, 0, thick, size));
        bar(Rect::new(size - thick, 0, size, size));
        line
    }

    fn solid(surface: &IndexedSurface, x: i32, y: i32) -> SwatchId {
        painted(surface.pixel(x, y))
    }

    #[test]
    fn a_closed_area_is_filled_out_to_its_lines_and_a_little_under_them() {
        let line = boxed(40, 4);
        let mut colour = IndexedSurface::new();
        let changed = fill(&mut colour, &line, (20, 20), SKIN, &Fill::default()).unwrap();
        assert_eq!(changed, Rect::new(2, 2, 38, 38), "the inside, and two pixels under the lines");
        assert_eq!((solid(&colour, 4, 4), solid(&colour, 35, 35), solid(&colour, 2, 20)), (SKIN, SKIN, SKIN));
        assert_eq!((solid(&colour, 1, 20), solid(&colour, 45, 20)), (SwatchId::NONE, SwatchId::NONE), "not past the lines");

        // Filled again with the same swatch, nothing changes; with another, all of it does.
        assert_eq!(fill(&mut colour, &line, (20, 20), SKIN, &Fill::default()), Ok(Rect::EMPTY));
        assert_eq!(fill(&mut colour, &line, (10, 30), SHIRT, &Fill::default()), Ok(changed));
        assert_eq!(colour.swatches().into_iter().collect::<Vec<_>>(), [SHIRT]);

        // No swatch takes the paint away.
        assert_eq!(fill(&mut colour, &line, (20, 20), SwatchId::NONE, &Fill::default()), Ok(changed));
        assert!(colour.is_empty() && colour.tiles().tile_count() == 0);
    }

    #[test]
    fn a_fill_runs_under_a_soft_line_to_its_middle() {
        // A box of soft lines 21 pixels wide, each covered most at its middle and less and
        // less out to its edges, so that half coverage is five pixels from the middle.
        let mut line = IndexedSurface::new();
        let soft: Vec<f32> = (0..21).map(|x| 1.0 - (x as f32 - 10.0).abs() / 10.5).collect();
        let across = |length: i32| soft.iter().flat_map(|c| std::iter::repeat_n(*c, length as usize)).collect::<Vec<_>>();
        let along = |length: i32| (0..length).flat_map(|_| soft.iter().copied()).collect::<Vec<_>>();
        line.paint(Rect::new(0, 0, 21, 100), &along(100), LINE);
        line.paint(Rect::new(79, 0, 100, 100), &along(100), LINE);
        line.paint(Rect::new(0, 0, 100, 21), &across(100), LINE);
        line.paint(Rect::new(0, 79, 100, 100), &across(100), LINE);
        let mut colour = IndexedSurface::new();
        fill(&mut colour, &line, (50, 50), SKIN, &Fill::default()).unwrap();
        assert_eq!((solid(&colour, 10, 50), solid(&colour, 89, 50), solid(&colour, 50, 10)), (SKIN, SKIN, SKIN), "to the middle of the lines");
        assert_eq!((solid(&colour, 9, 50), solid(&colour, 90, 50), solid(&colour, 50, 9)), (SwatchId::NONE, SwatchId::NONE, SwatchId::NONE), "and not past it");
    }

    #[test]
    fn a_click_on_a_line_or_outside_the_lines_fills_nothing() {
        let line = boxed(40, 4);
        let mut colour = IndexedSurface::new();
        assert_eq!(fill(&mut colour, &line, (1, 20), SKIN, &Fill::default()), Err(FillError::OnLine));
        assert_eq!(fill(&mut colour, &line, (500, 500), SKIN, &Fill::default()), Err(FillError::Open), "far from any drawing");
        assert!(colour.is_empty());
    }

    #[test]
    fn a_gap_in_the_lines_lets_the_fill_out_unless_it_is_closed() {
        let mut line = boxed(60, 4);
        // A gap six pixels wide in the top line.
        line.clear(Rect::new(27, 0, 33, 4));
        let mut colour = IndexedSurface::new();
        assert_eq!(fill(&mut colour, &line, (30, 30), SKIN, &Fill::default()), Err(FillError::Open));
        assert_eq!(fill(&mut colour, &line, (30, 30), SKIN, &Fill { gap: 4, under: 2 }), Err(FillError::Open), "the gap is wider than what is closed");
        assert!(colour.is_empty(), "a fill that gets out paints nothing");

        fill(&mut colour, &line, (30, 30), SKIN, &Fill { gap: 8, under: 2 }).unwrap();
        assert_eq!((solid(&colour, 5, 5), solid(&colour, 54, 54), solid(&colour, 30, 5)), (SKIN, SKIN, SKIN), "filled into the corners and up to the gap");
        assert_eq!(solid(&colour, 30, -6), SwatchId::NONE, "and not out through it");
        assert_eq!(solid(&colour, 80, 30), SwatchId::NONE);
    }

    #[test]
    fn a_closed_gap_lets_the_fill_neither_bulge_through_nor_run_along_a_corridor() {
        // A box of thin lines with a gap six pixels wide in its top, and above it a second
        // line that makes a corridor two pixels high, narrower than the gap, running away to
        // both sides past the box.
        let mut line = boxed(60, 1);
        line.clear(Rect::new(27, 0, 33, 1));
        line.paint(Rect::new(-40, -3, 100, -2), &vec![1.0; 140], LINE);
        let mut colour = IndexedSurface::new();
        fill(&mut colour, &line, (30, 30), SKIN, &Fill { gap: 8, under: 0 }).unwrap();
        assert_eq!((solid(&colour, 30, 1), solid(&colour, 2, 2), solid(&colour, 57, 57)), (SKIN, SKIN, SKIN), "up to the line inside");
        assert_eq!((solid(&colour, 27, 0), solid(&colour, 30, 0), solid(&colour, 32, 0)), (SKIN, SKIN, SKIN), "the gap is filled, with no notch");
        assert_eq!(solid(&colour, 30, -1), SwatchId::NONE, "but nothing past it");
        assert_eq!((solid(&colour, 10, -1), solid(&colour, -20, -1)), (SwatchId::NONE, SwatchId::NONE), "nor along the corridor");
    }

    /// How many pixels inside a flat ellipse's line are left unfilled, for each gap.
    #[test]
    fn a_flat_ellipse_is_filled_into_its_ends() {
        use crate::{BrushSettings, Point, Shape, paint_stroke};
        for hardness in [1.0, 0.5] {
            let mut line = IndexedSurface::new();
            let brush = BrushSettings { size: 8.0, hardness, pressure_size: false, ..Default::default() };
            paint_stroke(&mut line, &Shape::Ellipse.stroke(&brush, Point::new(0.0, 0.0), Point::new(600.0, 120.0), Default::default()), LINE);
            for gap in [0, 6, 20] {
                let mut colour = IndexedSurface::new();
                fill(&mut colour, &line, (300, 60), SKIN, &Fill { gap, under: 2 }).unwrap();
                // Every pixel inside the line that is no line is painted.
                let inside = |x: f64, y: f64| ((x - 300.0) / 296.0).powi(2) + ((y - 60.0) / 56.0).powi(2) < 1.0;
                let bare = (0..600).flat_map(|x| (0..120).map(move |y| (x, y))).filter(|&(x, y)| inside(f64::from(x) + 0.5, f64::from(y) + 0.5));
                let bare: Vec<_> = bare.filter(|&(x, y)| line.pixel(x, y).coverage < HALF && solid(&colour, x, y) != SKIN).collect();
                assert!(bare.is_empty(), "hardness {hardness}, gap {gap}: {} pixels bare, the first at {:?}", bare.len(), bare.first());
                assert_eq!(solid(&colour, -20, 60), SwatchId::NONE);
                // A click in the pointed end fills the same area.
                let mut from_end = IndexedSurface::new();
                fill(&mut from_end, &line, (8, 60), SKIN, &Fill { gap, under: 2 }).unwrap();
                assert!(from_end == colour, "hardness {hardness}, gap {gap}: filled from its end");
            }
        }
    }

    #[test]
    fn paint_of_another_swatch_bounds_a_fill_as_a_line_does() {
        let line = boxed(40, 4);
        let mut colour = IndexedSurface::new();
        // The left half painted by hand, with no line between the halves.
        colour.paint(Rect::new(4, 4, 20, 36), &vec![1.0; 16 * 32], SHIRT);
        fill(&mut colour, &line, (30, 20), SKIN, &Fill::default()).unwrap();
        assert_eq!((solid(&colour, 19, 20), solid(&colour, 20, 20)), (SHIRT, SKIN));
        assert_eq!(fill_protecting(&mut colour, &line, (10, 20), LINE, &Fill::default(), &[SHIRT]), Err(FillError::Protected));
        // And the hand-painted half is an area of its own.
        fill(&mut colour, &line, (10, 20), LINE, &Fill::default()).unwrap();
        assert_eq!((solid(&colour, 19, 20), solid(&colour, 20, 20)), (LINE, SKIN));
    }
}
