//! Brush model and stroke engine — pixel brushes, Bresenham stamps.
//!
//! Every footprint is centered on the stamp origin (0,0): a `size`-wide axis
//! spans `-(size/2) ..= (size-1)/2` (integer division), so even sizes straddle
//! the pixel boundary at 0 and odd sizes have their middle pixel at 0.
//! Round stamp rule (the classic pixel-circle chart): odd `N` rasterizes
//! `dx²+dy² <= ((N-1)/2)²` about the pixel center; even `N` rasterizes
//! `(dx+0.5)²+(dy+0.5)² <= (N/2)²` about the pixel boundary — size 1 is 1 px,
//! size 2 the full 2×2 block (4 px), size 3 the 5-px plus shape, size 4 the
//! 4×4 block minus its four corners (12 px). Pixel counts for 1..=7 are
//! 1, 4, 5, 12, 13, 32, 29. The footprint shape never depends on size
//! parity: a Round brush stays round (4-fold symmetric) at every size.
//! Bresenham lines include both endpoints, so fast drags leave no gaps.
//!
//! The Draw tool owns the shared painting infrastructure: a [`DrawTool`] is a
//! [`BrushSpec`] plus a [`DrawMode`] plus the primary color. Pen is Aseprite's
//! "Simple Ink" source-over blend ([`blend_over_pixel`]); Eraser subtracts the
//! primary color's alpha from the destination ([`subtract_alpha`]).

use crate::core::buffer::PixelBuffer;
use crate::core::clip::PixelClip;
use crate::core::color::Color;
use crate::core::math::Rect2i;

/// Brush footprint shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BrushShape {
    Square,
    Round,
}

impl BrushShape {
    /// The shape a Ctrl+scroll gesture selects, directionally and
    /// idempotently: scrolling up (positive `steps`) always selects
    /// [`Self::Round`], scrolling down (negative `steps`) always selects
    /// [`Self::Square`], and a zero net step count changes nothing (`None`).
    ///
    /// The sign of the net steps decides, so up-then-down in one frame cancels
    /// out, and repeating the same direction never toggles back.
    pub const fn from_scroll_steps(steps: i32) -> Option<Self> {
        if steps > 0 {
            Some(Self::Round)
        } else if steps < 0 {
            Some(Self::Square)
        } else {
            None
        }
    }
}

/// Stamp-scatter spread region: the shape around the cursor a scattered stamp
/// may land in. Every variant is sampled area-uniformly over the integer
/// lattice points of its region.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScatterShape {
    /// Uniform over the square `[-s, s]²` (the original jitter box).
    #[default]
    Square,
    /// Uniform over the disc `dx² + dy² <= s²`.
    Circle,
    /// Uniform over the diamond `|dx| + |dy| <= s`.
    Diamond,
}

/// A pixel brush: size in pixels (1–64) plus footprint shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BrushSpec {
    pub size: u8,
    pub shape: BrushShape,
}

impl BrushSpec {
    /// Smallest brush size in pixels.
    pub const MIN_SIZE: u8 = 1;
    /// Largest brush size in pixels.
    pub const MAX_SIZE: u8 = 64;

    /// The default 1 px pencil.
    pub const PENCIL_1PX: BrushSpec = BrushSpec {
        size: 1,
        shape: BrushShape::Square,
    };

    /// Panics when `size` is not in `1..=64` (clamping is NOT acceptable for a
    /// spec constructor; use [`Self::sanitize`] for UI input).
    pub fn new(size: u8, shape: BrushShape) -> Self {
        assert!(
            (1..=64).contains(&size),
            "BrushSpec::new: size must be in 1..=64, got {size}"
        );
        Self { size, shape }
    }

    /// Clamps `size` into `MIN_SIZE..=MAX_SIZE` (for UI sliders).
    pub fn sanitize(size: u8, shape: BrushShape) -> Self {
        Self {
            size: size.clamp(Self::MIN_SIZE, Self::MAX_SIZE),
            shape,
        }
    }

    /// Local offsets (relative to the stamp center) covered by this brush.
    ///
    /// The footprint's bounding box is centered on the stamp origin (0,0) for
    /// every size: a `size`-wide axis spans `-(size/2) ..= (size-1)/2` (integer
    /// division), so even sizes straddle the pixel boundary at 0 and odd sizes
    /// have their middle pixel at 0. Square: the full `size×size` block.
    /// Round (classic pixel-circle chart): odd `N` rasterizes the circle
    /// `dx²+dy² <= ((N-1)/2)²` about the pixel center (0,0); even `N`
    /// rasterizes `(dx+0.5)²+(dy+0.5)² <= (N/2)²` about the pixel boundary —
    /// 4-fold symmetric about the matching center and round at every size.
    pub fn stamp_offsets(&self) -> Vec<(i32, i32)> {
        let size = self.size as i32;
        let k = size / 2;
        // Centered inclusive range for a `size`-wide axis: `a..=b` with
        // `b - a = size - 1` and `a + b = -1` (even) / `a + b = 0` (odd).
        let (min, max) = (-k, (size - 1) / 2);
        let mut out = Vec::with_capacity((size * size) as usize);
        let full_block = self.shape == BrushShape::Square;
        for dy in min..=max {
            for dx in min..=max {
                let inside = if full_block {
                    true
                } else if size % 2 == 0 {
                    // (dx+0.5)² + (dy+0.5)² <= (N/2)², scaled ×4 to integers:
                    // (2dx+1)² + (2dy+1)² <= N².
                    let ex = 2 * dx + 1;
                    let ey = 2 * dy + 1;
                    ex * ex + ey * ey <= size * size
                } else {
                    // dx² + dy² <= ((N-1)/2)² about the pixel center.
                    dx * dx + dy * dy <= k * k
                };
                if inside {
                    out.push((dx, dy));
                }
            }
        }
        out
    }
}

/// Draw-tool paint mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DrawMode {
    /// Aseprite "Simple Ink" source-over: opaque replaces, alpha 0 erases.
    Pen,
    /// Subtracts the primary color's alpha from the destination alpha.
    Eraser,
}

/// A draw tool: brush spec + mode + primary color + stamp scatter + stepped
/// tail. Pen and Eraser share the same spec (size + shape), color, scatter and
/// tail; only the per-pixel operation differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DrawTool {
    pub spec: BrushSpec,
    pub mode: DrawMode,
    pub color: Color,
    /// Per-stamp jitter radius in pixels (`0` = off), clamped at
    /// [`Self::MAX_SCATTER`] via [`Stroke::with_scatter`].
    pub scatter: u8,
    /// Scatter spread region sampled by [`StampJitter::next_offset`].
    pub scatter_shape: ScatterShape,
    /// Stepped size tail (`-100..=100`, `0` = off), clamped at
    /// [`Self::MAX_TAIL`] via [`Stroke::with_tail`]. The magnitude is the
    /// number of canvas pixels of stroke travel that change the brush size by
    /// one pixel; the sign is the direction (`+` grows, `-` shrinks). `0` is
    /// byte-identical to no tail.
    pub tail: i8,
}

impl DrawTool {
    /// Largest per-stamp jitter radius in pixels.
    pub const MAX_SCATTER: u8 = 32;
    /// Largest magnitude of the stepped size tail (either sign): at `+100` the
    /// brush grows one pixel per 100 px of travel; at `-100` it shrinks one
    /// pixel per 100 px.
    pub const MAX_TAIL: i8 = 100;
}

/// Applies the stepped size tail: every `|tail|` canvas pixels of stroke
/// travel changes the brush size by one pixel, in the sign's direction
/// (`+` grows, `-` shrinks), so the effective size is an exact integer at
/// every stamp — no fractional-size rounding.
///
/// `traveled` is the accumulated path length of the current stroke in canvas
/// pixels; it is floored to whole pixels and divided with integer division, so
/// a partial step contributes nothing. `tail == 0` returns `base` unchanged
/// (byte-identical to no tail); the result is clamped to
/// `BrushSpec::MIN_SIZE..=BrushSpec::MAX_SIZE`.
pub fn tail_size(base: u8, tail: i8, traveled: f32) -> u8 {
    if tail == 0 {
        return base;
    }
    let step = i32::from(tail).abs();
    let steps = (traveled.max(0.0) as i32) / step;
    let delta = if tail > 0 { steps } else { -steps };
    let size = i32::from(base) + delta;
    size.clamp(
        i32::from(BrushSpec::MIN_SIZE),
        i32::from(BrushSpec::MAX_SIZE),
    ) as u8
}

/// Deterministic per-stamp jitter source (LCG). One stream per stroke: seeded
/// at the first stamp via [`Self::for_stroke`] (or pinned via
/// [`Stroke::with_jitter_seed`] for tests) and advanced exactly once per
/// stamp, so segment restarts never reseed and a pinned seed reproduces a
/// stroke byte-exactly.
pub struct StampJitter {
    state: u32,
}

impl StampJitter {
    /// The stream starting at `seed`.
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// Seed for a real stroke: mixes the start point with a monotonically
    /// increasing stroke counter so two strokes through the same point still
    /// diverge.
    pub fn for_stroke(x: i32, y: i32) -> Self {
        static STROKE_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        let counter = STROKE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let seed = (x as u32).wrapping_mul(0x9E37_79B9)
            ^ (y as u32).wrapping_mul(0x85EB_CA6B)
            ^ counter.wrapping_mul(0xC2B2_AE35);
        Self::new(seed)
    }

    /// Next independent-uniform `(jx, jy)` in `[-scatter, scatter]²` filtered
    /// by `shape`. `scatter == 0` yields `(0, 0)` for every shape without
    /// advancing the stream. `Square` samples the box directly; `Circle` and
    /// `Diamond` rejection-sample the box so accepted points are area-uniform
    /// over their region.
    pub fn next_offset(&mut self, scatter: u8, shape: ScatterShape) -> (i32, i32) {
        let radius = i32::from(scatter);
        if radius == 0 {
            return (0, 0);
        }
        match shape {
            ScatterShape::Square => {
                let span = 2 * radius + 1;
                let jx = self.below(span) - radius;
                let jy = self.below(span) - radius;
                (jx, jy)
            }
            ScatterShape::Circle => {
                let limit = radius * radius;
                self.rejection_sample(radius, |dx, dy| dx * dx + dy * dy <= limit)
            }
            ScatterShape::Diamond => {
                self.rejection_sample(radius, |dx, dy| dx.abs() + dy.abs() <= radius)
            }
        }
    }

    /// Rejection-samples the `[-radius, radius]²` box until `accept` holds,
    /// giving an area-uniform point of the target region. Bounded to
    /// [`Self::MAX_REJECTION_TRIES`] attempts; falls back to `(0, 0)` (always
    /// inside the region for `radius >= 0`) rather than looping forever.
    fn rejection_sample(&mut self, radius: i32, accept: impl Fn(i32, i32) -> bool) -> (i32, i32) {
        let span = 2 * radius + 1;
        for _ in 0..Self::MAX_REJECTION_TRIES {
            let dx = self.below(span) - radius;
            let dy = self.below(span) - radius;
            if accept(dx, dy) {
                return (dx, dy);
            }
        }
        (0, 0)
    }

    /// Bounded rejection attempts before [`Self::rejection_sample`] falls back
    /// to the center. The disc accepts ~79% and the diamond ~50% of the box,
    /// so exhausting 32 tries has probability well under 2⁻³⁰.
    const MAX_REJECTION_TRIES: u32 = 32;

    /// Next LCG word reduced uniformly to `0..span` (rejection sampling —
    /// keeps the draw unbiased).
    fn below(&mut self, span: i32) -> i32 {
        let span = u64::from(span as u32);
        let limit = (1u64 << 32) - ((1u64 << 32) % span);
        loop {
            self.state = self
                .state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            let word = u64::from(self.state);
            if word < limit {
                return (word % span) as i32;
            }
        }
    }
}

/// Aseprite `rgba_blender_normal` (Simple Ink source-over), non-premultiplied.
///
/// - `src.a == 255` → the destination becomes exactly `src` (opaque replace);
/// - `src.a == 0` → the pixel is erased (fully transparent, RGB zeroed);
/// - `dst.a == 0` (and `0 < src.a < 255`) → the destination becomes `src`;
/// - otherwise: `Ra = Sa + Ba - round(Ba*Sa/255)` and
///   `Rc = Bc + round((Sc-Bc)*Sa/Ra)` per channel.
pub fn blend_over_pixel(dst: Color, src: Color) -> Color {
    let sa = src.a as u16;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return Color::TRANSPARENT;
    }
    if dst.a == 0 {
        return src;
    }
    let ba = dst.a as u16;
    let ra = sa + ba - ((ba * sa) as f32 / 255.0).round() as u16;
    let blend = |sc: u8, bc: u8| {
        let sc = sc as f32;
        let bc = bc as f32;
        (bc + (sc - bc) * sa as f32 / ra as f32).round() as u8
    };
    Color::rgba(
        blend(src.r, dst.r),
        blend(src.g, dst.g),
        blend(src.b, dst.b),
        ra as u8,
    )
}

/// Eraser: subtract `amount` from the destination alpha, RGB untouched. When
/// the resulting alpha is 0 the RGB is zeroed too (fully clean pixel).
pub fn subtract_alpha(dst: Color, amount: u8) -> Color {
    let a = dst.a.saturating_sub(amount);
    if a == 0 {
        Color::TRANSPARENT
    } else {
        Color::rgba(dst.r, dst.g, dst.b, a)
    }
}

/// Applies the tool's mode to one destination pixel (the shared stamp helper).
fn apply_tool(dst: Color, tool: DrawTool) -> Color {
    match tool.mode {
        DrawMode::Pen => blend_over_pixel(dst, tool.color),
        DrawMode::Eraser => subtract_alpha(dst, tool.color.a),
    }
}

/// Writes every stamp pixel `(cx+dx, cy+dy)` that is in bounds.
/// Out-of-bounds offsets (negative or past the buffer edge) are skipped.
pub fn stamp_at(buf: &mut PixelBuffer, tool: DrawTool, cx: i32, cy: i32) {
    for (dx, dy) in tool.spec.stamp_offsets() {
        let px = cx + dx;
        let py = cy + dy;
        if px >= 0 && py >= 0 {
            stamp_pixel(buf, px as usize, py as usize, tool);
        }
    }
}

/// In-bounds-only variant; the caller guarantees every offset lands in bounds
/// (used by the fast stroke path after rect-clipping the line).
pub fn stamp_at_unchecked(buf: &mut PixelBuffer, tool: DrawTool, cx: i32, cy: i32) {
    for (dx, dy) in tool.spec.stamp_offsets() {
        stamp_pixel_unchecked(buf, (cx + dx) as usize, (cy + dy) as usize, tool);
    }
}

fn stamp_pixel(buf: &mut PixelBuffer, x: usize, y: usize, tool: DrawTool) {
    if let Some(dst) = buf.get_pixel(x, y) {
        buf.set_pixel(x, y, apply_tool(dst, tool));
    }
}

fn stamp_pixel_unchecked(buf: &mut PixelBuffer, x: usize, y: usize, tool: DrawTool) {
    let dst = buf.get_pixel_unchecked(x, y);
    buf.set_pixel_unchecked(x, y, apply_tool(dst, tool));
}

/// Single-point eraser convenience (stamp only, no line interpolation).
pub fn erase_to(buf: &mut PixelBuffer, spec: BrushSpec, x: i32, y: i32) {
    stamp_at(
        buf,
        DrawTool {
            spec,
            mode: DrawMode::Eraser,
            color: Color::BLACK,
            scatter: 0,
            scatter_shape: ScatterShape::Square,
            tail: 0,
        },
        x,
        y,
    );
}

/// Single-point draw convenience (stamp only, no line interpolation).
pub fn draw_to(buf: &mut PixelBuffer, spec: BrushSpec, x: i32, y: i32, color: Color) {
    stamp_at(
        buf,
        DrawTool {
            spec,
            mode: DrawMode::Pen,
            color,
            scatter: 0,
            scatter_shape: ScatterShape::Square,
            tail: 0,
        },
        x,
        y,
    );
}

/// Integer Bresenham line from `a` to `b`, both endpoints inclusive.
pub(crate) fn bresenham_line(a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let (mut x, mut y) = a;
    let (x1, y1) = b;
    let dx = (x1 - x).abs();
    let dy = -(y1 - y).abs();
    let sx = if x < x1 { 1 } else { -1 };
    let sy = if y < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut points = Vec::new();
    loop {
        points.push((x, y));
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
    points
}

/// Snap the `anchor → end` vector to the nearest multiple of 22.5° while
/// keeping the pointer distance, for Ctrl-held LINE mode.
///
/// Only the direction is quantized: the radius from `anchor` to `end` is
/// preserved and the rotated offset is rounded back to integer pixels. A
/// degenerate vector (`end == anchor`) is returned unchanged.
pub fn snap_line_angle(anchor: (i32, i32), end: (i32, i32)) -> (i32, i32) {
    let dx = f64::from(end.0 - anchor.0);
    let dy = f64::from(end.1 - anchor.1);
    let radius = (dx * dx + dy * dy).sqrt();
    if radius == 0.0 {
        return end;
    }
    const STEP: f64 = std::f64::consts::PI / 8.0; // 22.5°
    let snapped = (dy.atan2(dx) / STEP).round() * STEP;
    (
        anchor.0 + (radius * snapped.cos()).round() as i32,
        anchor.1 + (radius * snapped.sin()).round() as i32,
    )
}

/// Canvas-sized bitmask of the cells a single stroke has already painted.
///
/// Memory is one bit per canvas pixel, allocated lazily on the first in-bounds
/// stamp and dropped with the stroke, so it is bounded by the canvas rather
/// than the stroke length: a 4096² canvas costs 2 MiB however far the stroke
/// travels, where a `HashSet<(i32, i32)>` costs ~16–32 bytes per touched cell
/// and can exceed the canvas on a long scattered stroke. Tests and sets are
/// O(1) with no hashing.
struct TouchedCells {
    width: usize,
    height: usize,
    words_per_row: usize,
    bits: Vec<u64>,
}

impl TouchedCells {
    fn new(width: usize, height: usize) -> Self {
        let words_per_row = width.div_ceil(64);
        Self {
            width,
            height,
            words_per_row,
            bits: vec![0; words_per_row * height],
        }
    }

    /// Marks `(x, y)` and reports whether it was previously clear. The caller
    /// guarantees `x < width` and `y < height`.
    fn mark(&mut self, x: usize, y: usize) -> bool {
        let word = y * self.words_per_row + x / 64;
        let mask = 1u64 << (x % 64);
        let fresh = self.bits[word] & mask == 0;
        self.bits[word] |= mask;
        fresh
    }
}

/// An in-progress stroke: interpolates stamps between input samples so fast
/// drags leave no dotted gaps. Each stamp may be displaced by a per-stamp
/// jitter of up to `tool.scatter` pixels (deterministic stream, seeded once)
/// and its size steps with `tool.tail` as the stroke travels.
pub struct Stroke {
    tool: DrawTool,
    clip: Option<PixelClip>,
    last: (i32, i32),
    started: bool,
    bounds: Rect2i,
    /// Union of every stamped stamp's scatter-expanded `size×size` square
    /// centered on the cursor point (clamped to `bounds`); `None` until
    /// something lands.
    bbox: Option<Rect2i>,
    /// Jitter stream: seeded once at the first stamp, or pinned up front by
    /// [`Self::with_jitter_seed`].
    jitter: Option<StampJitter>,
    /// Accumulated path length of the interpolated stamps, in canvas pixels,
    /// from the stroke start. Drives the stepped tail and never resets (a
    /// mid-stroke segment restart keeps accumulating).
    travel: f32,
    /// Cursor point of the previous stamp, used to accumulate [`Self::travel`].
    travel_anchor: (i32, i32),
    /// Cells already painted by this stroke; a later stamp skips them so a
    /// pixel receives the tool's blend at most once per stroke. `None` until
    /// the first in-bounds stamp; a fresh `Stroke` starts clear, while a
    /// segment restart on the same stroke keeps the record.
    touched: Option<TouchedCells>,
}

impl Stroke {
    /// Bounds default to `Rect2i::ZERO` (unbounded: `stamp_at` clips to the
    /// buffer); call [`Self::set_bounds`] to clamp to a canvas/layer rect.
    pub fn new(spec: BrushSpec, mode: DrawMode, color: Color) -> Self {
        Self {
            tool: DrawTool {
                spec,
                mode,
                color,
                scatter: 0,
                scatter_shape: ScatterShape::Square,
                tail: 0,
            },
            clip: None,
            last: (0, 0),
            started: false,
            bounds: Rect2i::ZERO,
            bbox: None,
            jitter: None,
            travel: 0.0,
            travel_anchor: (0, 0),
            touched: None,
        }
    }

    /// Per-stamp jitter radius in pixels, clamped to `0..=MAX_SCATTER`
    /// (`0` disables scatter).
    pub fn with_scatter(mut self, scatter: u8) -> Self {
        self.tool.scatter = scatter.min(DrawTool::MAX_SCATTER);
        self
    }

    /// Scatter spread region sampled when scatter is enabled.
    pub fn with_scatter_shape(mut self, shape: ScatterShape) -> Self {
        self.tool.scatter_shape = shape;
        self
    }

    /// Stepped size tail along the stroke, clamped to
    /// `-MAX_TAIL..=MAX_TAIL` (`0` = constant size). The magnitude is the
    /// canvas pixels of travel per one-pixel size change; the sign is the
    /// direction.
    pub fn with_tail(mut self, tail: i8) -> Self {
        self.tool.tail = tail.clamp(-DrawTool::MAX_TAIL, DrawTool::MAX_TAIL);
        self
    }

    /// Pins the jitter stream to `seed` so the stroke reproduces byte-exactly
    /// (tests).
    pub fn with_jitter_seed(mut self, seed: u32) -> Self {
        self.jitter = Some(StampJitter::new(seed));
        self
    }

    /// Sets the optional pixel clip for this stroke.
    pub fn with_clip(mut self, clip: Option<PixelClip>) -> Self {
        self.clip = clip;
        self
    }

    /// Clamp the stroke to a canvas/layer rect (clipped stamps).
    pub fn set_bounds(&mut self, bounds: Rect2i) {
        self.bounds = bounds;
    }

    /// Stamps at `(x, y)` and records it as the stroke origin.
    /// Out-of-bounds stamps are skipped, but `last` still updates.
    pub fn start(&mut self, buf: &mut PixelBuffer, x: i32, y: i32) {
        self.stamp(buf, x, y);
        self.last = (x, y);
        self.started = true;
    }

    /// Extends the stroke to `(x, y)`, Bresenham-interpolating from the last
    /// sample so every line pixel is stamped (no dotted gaps). When the stroke
    /// has not started, delegates to [`Self::start`].
    pub fn continue_to(&mut self, buf: &mut PixelBuffer, x: i32, y: i32) {
        if !self.started {
            self.start(buf, x, y);
            return;
        }
        for (px, py) in bresenham_line(self.last, (x, y)) {
            self.stamp(buf, px, py);
        }
        self.last = (x, y);
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn last_pos(&self) -> (i32, i32) {
        self.last
    }

    /// Union of every stamped stamp's scatter-expanded `size×size` bbox
    /// (clamped to bounds); `Rect2i::ZERO` when nothing was stamped. The
    /// expansion covers any jitter and a per-stamp tapered size, so the region
    /// contains every touched pixel.
    pub fn bounding_box(&self) -> Rect2i {
        self.bbox.unwrap_or(Rect2i::ZERO)
    }

    fn stamp(&mut self, buf: &mut PixelBuffer, x: i32, y: i32) {
        if self.started {
            let dx = x - self.travel_anchor.0;
            let dy = y - self.travel_anchor.1;
            self.travel += ((dx * dx + dy * dy) as f32).sqrt();
        }
        self.travel_anchor = (x, y);
        let spec = BrushSpec {
            size: tail_size(self.tool.spec.size, self.tool.tail, self.travel),
            shape: self.tool.spec.shape,
        };
        let scatter = self.tool.scatter;
        let shape = self.tool.scatter_shape;
        let (jx, jy) = self
            .jitter
            .get_or_insert_with(|| StampJitter::for_stroke(x, y))
            .next_offset(scatter, shape);
        let (cx, cy) = (x + jx, y + jy);
        // Clip the jittered footprint to the stroke bounds.
        let mut stamp_bounds = stamp_bbox(spec, cx, cy);
        if !self.bounds.is_empty() {
            stamp_bounds = stamp_bounds.clamp_to(self.bounds);
        }
        if stamp_bounds.is_empty() {
            return;
        }
        let touched = self
            .touched
            .get_or_insert_with(|| TouchedCells::new(buf.width(), buf.height()));
        if touched.width != buf.width() || touched.height != buf.height() {
            *touched = TouchedCells::new(buf.width(), buf.height());
        }
        let mut written = 0;
        for (dx, dy) in spec.stamp_offsets() {
            let px = cx + dx;
            let py = cy + dy;
            if let Some(pixel_clip) = &self.clip {
                if !pixel_clip.contains(px, py) {
                    continue;
                }
            }
            if stamp_bounds.contains(px, py) && px >= 0 && py >= 0 {
                let (ux, uy) = (px as usize, py as usize);
                if let Some(dst) = buf.get_pixel(ux, uy) {
                    if touched.mark(ux, uy) {
                        buf.set_pixel_unchecked(ux, uy, apply_tool(dst, self.tool));
                        written += 1;
                    }
                }
            }
        }
        if written == 0 {
            return;
        }
        // Undo-bbox contribution: the effective footprint at the cursor point
        // expanded by the scatter radius on every side — a deterministic
        // superset that covers whatever jitter this stamp drew.
        let base = stamp_bbox(spec, x, y);
        let margin = i32::from(scatter);
        let mut bbox = Rect2i::new(
            base.x - margin,
            base.y - margin,
            base.w + 2 * margin,
            base.h + 2 * margin,
        );
        if !self.bounds.is_empty() {
            bbox = bbox.clamp_to(self.bounds);
        }
        if !bbox.is_empty() {
            self.bbox = Some(match self.bbox {
                Some(b) => b.union(bbox),
                None => bbox,
            });
        }
    }
}

/// The `size×size` square centered on the stamp center — the bounding box of a
/// stamp's footprint. Used both to clip stamps to the stroke bounds and as the
/// stroke's bounding-box contribution (matches the documented bbox semantics).
fn stamp_bbox(spec: BrushSpec, cx: i32, cy: i32) -> Rect2i {
    let size = spec.size as i32;
    let half = size / 2;
    Rect2i::new(cx - half, cy - half, size, size)
}

/// Delta-capture seam: the caller turns this into an undo command via
/// `ReverseDeltaCommand`. Kept dependency-free (no undo imports) by design.
pub struct StrokeRecord {
    pub region: Rect2i,
    pub before: Vec<u8>,
}

impl StrokeRecord {
    /// Captures the region's RGBA8 bytes before a stroke touches it.
    /// `None` when the region is out of bounds.
    pub fn capture_region(buf: &PixelBuffer, region: Rect2i) -> Option<StrokeRecord> {
        let before = buf.export_region(region, None)?;
        Some(StrokeRecord { region, before })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::clip::PixelClip;
    use crate::core::select::Selection;

    fn spec(size: u8, shape: BrushShape) -> BrushSpec {
        BrushSpec::new(size, shape)
    }

    fn pen(spec: BrushSpec, color: Color) -> DrawTool {
        DrawTool {
            spec,
            mode: DrawMode::Pen,
            color,
            scatter: 0,
            scatter_shape: ScatterShape::Square,
            tail: 0,
        }
    }

    fn eraser(spec: BrushSpec, color: Color) -> DrawTool {
        DrawTool {
            spec,
            mode: DrawMode::Eraser,
            color,
            scatter: 0,
            scatter_shape: ScatterShape::Square,
            tail: 0,
        }
    }

    #[test]
    fn square_stamp_offset_counts() {
        assert_eq!(spec(1, BrushShape::Square).stamp_offsets().len(), 1);
        assert_eq!(spec(2, BrushShape::Square).stamp_offsets().len(), 4);
        assert_eq!(spec(3, BrushShape::Square).stamp_offsets().len(), 9);
        assert_eq!(spec(4, BrushShape::Square).stamp_offsets().len(), 16);
    }

    #[test]
    fn square_stamp_offsets_are_row_major() {
        assert_eq!(
            spec(2, BrushShape::Square).stamp_offsets(),
            vec![(-1, -1), (0, -1), (-1, 0), (0, 0)]
        );
    }

    #[test]
    fn round_stamp_offsets_odd_sizes_are_symmetric_and_centered() {
        for size in [1u8, 3u8] {
            let offsets = spec(size, BrushShape::Round).stamp_offsets();
            assert!(offsets.contains(&(0, 0)), "size {size} must contain (0,0)");
            for &(dx, dy) in &offsets {
                assert!(
                    offsets.contains(&(-dx, -dy)),
                    "size {size}: ({dx},{dy}) lacks mirror (-{dx},-{dy})"
                );
            }
        }
    }

    #[test]
    fn round_stamp_offset_sets() {
        // Size 1: single center pixel.
        assert_eq!(spec(1, BrushShape::Round).stamp_offsets(), vec![(0, 0)]);
        // Size 2: the full 2×2 block (the boundary-centered circle covers all 4 px).
        assert_eq!(
            spec(2, BrushShape::Round).stamp_offsets(),
            vec![(-1, -1), (0, -1), (-1, 0), (0, 0)]
        );
        // Size 3: plus shape (circle of radius 1 about the pixel center).
        assert_eq!(
            spec(3, BrushShape::Round).stamp_offsets(),
            vec![(0, -1), (-1, 0), (0, 0), (1, 0), (0, 1)]
        );
        // Size 4: the 4×4 block with the four corners cut (12 px).
        let offsets = spec(4, BrushShape::Round).stamp_offsets();
        assert_eq!(offsets.len(), 12);
        for dy in -2..=1 {
            for dx in -2..=1 {
                let corner = (dx == -2 || dx == 1) && (dy == -2 || dy == 1);
                if corner {
                    assert!(
                        !offsets.contains(&(dx, dy)),
                        "corner ({dx},{dy}) must be cut"
                    );
                } else {
                    assert!(offsets.contains(&(dx, dy)), "missing ({dx},{dy})");
                }
            }
        }
    }

    #[test]
    fn round_footprint_matches_the_classic_pixel_circle_chart() {
        let expected_counts = [1usize, 4, 5, 12, 13, 32, 29];
        for (size, expected) in (1u8..=7).zip(expected_counts) {
            let offsets = spec(size, BrushShape::Round).stamp_offsets();
            assert_eq!(offsets.len(), expected, "size {size} footprint count");
            for &(dx, dy) in &offsets {
                // 4-fold rotation about the footprint center: the pixel center
                // (0,0) for odd N, the pixel boundary (-0.5,-0.5) for even N.
                let quarter_turn = |(px, py): (i32, i32)| -> (i32, i32) {
                    if size % 2 == 0 {
                        (-1 - py, px)
                    } else {
                        (-py, px)
                    }
                };
                let mut rotated = (dx, dy);
                for turn in 0..4 {
                    rotated = quarter_turn(rotated);
                    assert!(
                        offsets.contains(&rotated),
                        "size {size}: ({dx},{dy}) rotated {turn} quarter-turns to {rotated:?} is missing"
                    );
                }
            }
        }
    }

    #[test]
    fn stamp_offsets_are_centered_for_every_size() {
        for shape in [BrushShape::Square, BrushShape::Round] {
            for size in 1u8..=64 {
                let offsets = spec(size, shape).stamp_offsets();
                let min_x = offsets.iter().map(|&(x, _)| x).min().unwrap();
                let max_x = offsets.iter().map(|&(x, _)| x).max().unwrap();
                let min_y = offsets.iter().map(|&(_, y)| y).min().unwrap();
                let max_y = offsets.iter().map(|&(_, y)| y).max().unwrap();
                // Even sizes straddle the pixel boundary at 0; odd sizes have
                // their middle pixel at 0. Either way the footprint is centered
                // on the stamp origin.
                if size % 2 == 0 {
                    assert_eq!(
                        (min_x + max_x + 1) as f32 / 2.0,
                        0.0,
                        "size {size} {shape:?} x-axis not centered"
                    );
                    assert_eq!(
                        (min_y + max_y + 1) as f32 / 2.0,
                        0.0,
                        "size {size} {shape:?} y-axis not centered"
                    );
                } else {
                    assert_eq!(
                        min_x + max_x,
                        0,
                        "size {size} {shape:?} x-axis not centered"
                    );
                    assert_eq!(
                        min_y + max_y,
                        0,
                        "size {size} {shape:?} y-axis not centered"
                    );
                }
            }
        }
    }

    #[test]
    fn size2_square_stamp_is_centered_on_the_cursor() {
        let mut buf = PixelBuffer::new(8, 8);
        let color = Color::rgb(11, 22, 33);
        stamp_at(&mut buf, pen(spec(2, BrushShape::Square), color), 4, 4);
        // Centered: the four pixels (x-1,y-1)…(x,y), not (x,y)…(x+1,y+1).
        for (x, y) in [(3, 3), (4, 3), (3, 4), (4, 4)] {
            assert_eq!(
                buf.get_pixel(x, y),
                Some(color),
                "({x},{y}) must be painted"
            );
        }
        for (x, y) in [(5, 4), (4, 5), (5, 5), (2, 4), (4, 2)] {
            assert_eq!(
                buf.get_pixel(x, y),
                Some(Color::TRANSPARENT),
                "({x},{y}) must stay clear"
            );
        }
    }

    #[test]
    fn new_panics_on_invalid_size() {
        assert!(std::panic::catch_unwind(|| BrushSpec::new(0, BrushShape::Square)).is_err());
        assert!(std::panic::catch_unwind(|| BrushSpec::new(65, BrushShape::Round)).is_err());
        assert!(std::panic::catch_unwind(|| BrushSpec::new(255, BrushShape::Square)).is_err());
    }

    #[test]
    fn sanitize_clamps_size() {
        assert_eq!(BrushSpec::sanitize(0, BrushShape::Square).size, 1);
        assert_eq!(BrushSpec::sanitize(100, BrushShape::Round).size, 64);
        assert_eq!(BrushSpec::sanitize(3, BrushShape::Square).size, 3);
        assert_eq!(BrushSpec::PENCIL_1PX, BrushSpec::new(1, BrushShape::Square));
    }

    #[test]
    fn stamp_at_colors_exact_block() {
        let mut buf = PixelBuffer::new(8, 8);
        let color = Color::rgb(10, 20, 30);
        stamp_at(&mut buf, pen(spec(2, BrushShape::Square), color), 2, 2);
        for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
            assert_eq!(buf.get_pixel(x, y), Some(color));
        }
        // Just outside the block stays transparent.
        assert_eq!(buf.get_pixel(3, 2), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(2, 3), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(0, 2), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(2, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn stamp_at_clips_at_edges() {
        let mut buf = PixelBuffer::new(8, 8);
        let color = Color::rgb(1, 2, 3);
        stamp_at(&mut buf, pen(spec(3, BrushShape::Square), color), 0, 0);
        assert_eq!(buf.get_pixel(0, 0), Some(color));
        assert_eq!(buf.get_pixel(1, 0), Some(color));
        assert_eq!(buf.get_pixel(0, 1), Some(color));
        assert_eq!(buf.get_pixel(1, 1), Some(color));
        assert_eq!(buf.get_pixel(2, 0), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(0, 2), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(2, 2), Some(Color::TRANSPARENT));
    }

    #[test]
    fn stamp_at_negative_offsets_clip_without_panic() {
        let mut buf = PixelBuffer::new(8, 8);
        let color = Color::rgb(5, 6, 7);
        // Round size 3 has negative offsets; stamping at the origin must skip
        // out-of-bounds pixels without panicking.
        stamp_at(&mut buf, pen(spec(3, BrushShape::Round), color), 0, 0);
        assert_eq!(buf.get_pixel(0, 0), Some(color));
        assert_eq!(buf.get_pixel(1, 0), Some(color));
        assert_eq!(buf.get_pixel(0, 1), Some(color));
        assert_eq!(buf.get_pixel(1, 1), Some(Color::TRANSPARENT));
    }

    #[test]
    fn stamp_at_unchecked_matches_checked_in_bounds() {
        let mut a = PixelBuffer::new(8, 8);
        let mut b = PixelBuffer::new(8, 8);
        let color = Color::rgb(9, 8, 7);
        stamp_at(&mut a, pen(spec(2, BrushShape::Round), color), 3, 3);
        stamp_at_unchecked(&mut b, pen(spec(2, BrushShape::Round), color), 3, 3);
        assert_eq!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn round_size4_stamp_covers_centered_block() {
        let mut buf = PixelBuffer::new(16, 16);
        let color = Color::rgb(7, 8, 9);
        stamp_at(&mut buf, pen(spec(4, BrushShape::Round), color), 4, 4);
        // Boundary-centered circle within the 4×4 block: the four corners stay
        // clear, the other 12 pixels are painted.
        for dy in -2..=1 {
            for dx in -2..=1 {
                let corner = (dx == -2 || dx == 1) && (dy == -2 || dy == 1);
                let expected = if corner { Color::TRANSPARENT } else { color };
                assert_eq!(
                    buf.get_pixel((4 + dx) as usize, (4 + dy) as usize),
                    Some(expected),
                    "({dx},{dy})"
                );
            }
        }
        assert_eq!(buf.get_pixel(1, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(6, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(4, 1), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(4, 6), Some(Color::TRANSPARENT));
    }

    #[test]
    fn bresenham_horizontal() {
        let pts = bresenham_line((0, 0), (4, 2));
        assert_eq!(pts.len(), 5);
        assert_eq!(pts.first(), Some(&(0, 0)));
        assert_eq!(pts.last(), Some(&(4, 2)));
        let mut prev = None;
        for &(x, y) in &pts {
            if let Some((px, _)) = prev {
                assert!(x > px, "x must be monotonic");
            }
            prev = Some((x, y));
        }
        let mut sorted = pts.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), pts.len(), "no duplicates");
    }

    #[test]
    fn bresenham_vertical() {
        let pts = bresenham_line((3, 1), (3, 7));
        assert_eq!(pts.len(), 7);
        assert_eq!(pts.first(), Some(&(3, 1)));
        assert_eq!(pts.last(), Some(&(3, 7)));
        for (i, &(x, y)) in pts.iter().enumerate() {
            assert_eq!(x, 3);
            assert_eq!(y, 1 + i as i32);
        }
    }

    #[test]
    fn bresenham_single_point() {
        assert_eq!(bresenham_line((0, 0), (0, 0)), vec![(0, 0)]);
    }

    #[test]
    fn bresenham_diagonal() {
        assert_eq!(
            bresenham_line((0, 0), (3, 3)),
            vec![(0, 0), (1, 1), (2, 2), (3, 3)]
        );
    }

    #[test]
    fn bresenham_reverse_has_same_extent() {
        let fwd = bresenham_line((0, 0), (4, 2));
        let rev = bresenham_line((4, 2), (0, 0));
        assert_eq!(fwd.len(), rev.len());
        assert_eq!(rev.first(), Some(&(4, 2)));
        assert_eq!(rev.last(), Some(&(0, 0)));
    }

    #[test]
    fn stroke_no_gap_guarantee() {
        let mut buf = PixelBuffer::new(16, 16);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        stroke.start(&mut buf, 0, 0);
        stroke.continue_to(&mut buf, 4, 2);
        stroke.continue_to(&mut buf, 4, 5);
        let line1 = bresenham_line((0, 0), (4, 2));
        let line2 = bresenham_line((4, 2), (4, 5));
        let expected = line1.len() + line2.len() - 1;
        let colored = buf
            .as_bytes()
            .chunks_exact(4)
            .filter(|px| px[3] != 0)
            .count();
        assert_eq!(colored, expected);
        for &(x, y) in line1.iter().chain(line2.iter()) {
            assert_eq!(buf.get_pixel(x as usize, y as usize), Some(Color::WHITE));
        }
    }

    #[test]
    fn stroke_fast_horizontal_drag_stamps_every_pixel() {
        let mut buf = PixelBuffer::new(32, 4);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        stroke.start(&mut buf, 0, 0);
        stroke.continue_to(&mut buf, 20, 0);
        let colored = buf
            .as_bytes()
            .chunks_exact(4)
            .filter(|px| px[3] != 0)
            .count();
        assert_eq!(colored, 21);
        for x in 0..=20 {
            assert_eq!(buf.get_pixel(x, 0), Some(Color::WHITE));
        }
        assert_eq!(buf.get_pixel(21, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn eraser_removes_to_transparent() {
        let mut buf = PixelBuffer::new(8, 8);
        buf.fill(Color::rgb(200, 100, 50));
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Eraser, Color::BLACK);
        stroke.start(&mut buf, 2, 2);
        stroke.continue_to(&mut buf, 5, 2);
        for x in 2..=5 {
            assert_eq!(buf.get_pixel(x, 2), Some(Color::TRANSPARENT));
        }
        assert_eq!(buf.get_pixel(1, 2), Some(Color::rgb(200, 100, 50)));
        assert_eq!(buf.get_pixel(2, 1), Some(Color::rgb(200, 100, 50)));
        assert_eq!(buf.get_pixel(6, 2), Some(Color::rgb(200, 100, 50)));
        assert_eq!(buf.get_pixel(2, 3), Some(Color::rgb(200, 100, 50)));
    }

    #[test]
    fn masked_clip_limits_size_one_stroke_to_selected_cells() {
        let mut buffer = PixelBuffer::new(6, 3);
        buffer.fill(Color::WHITE);
        let rect = Rect2i::new(1, 1, 3, 1);
        let selection = Selection::capture_mask(&buffer, rect, vec![true, false, true]).unwrap();
        let clip = PixelClip::from_selection(&selection);
        let before = buffer.as_bytes().to_vec();
        let paint = Color::BLACK;
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, paint)
            .with_clip(Some(clip.clone()));

        stroke.start(&mut buffer, 0, 1);
        stroke.continue_to(&mut buffer, 4, 1);

        for x in 0usize..6 {
            let offset = (buffer.width() + x) * 4;
            if clip.contains(x as i32, 1) {
                assert_eq!(buffer.get_pixel(x, 1), Some(paint));
            } else {
                assert_eq!(
                    &buffer.as_bytes()[offset..offset + 4],
                    &before[offset..offset + 4]
                );
            }
        }
    }

    #[test]
    fn with_clip_none_preserves_unclipped_stroke_paint() {
        let mut buffer = PixelBuffer::new(6, 3);
        buffer.fill(Color::WHITE);
        let paint = Color::BLACK;
        let mut stroke =
            Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, paint).with_clip(None);

        stroke.start(&mut buffer, 0, 1);
        stroke.continue_to(&mut buffer, 4, 1);

        for x in 0..5 {
            assert_eq!(buffer.get_pixel(x, 1), Some(paint));
        }
        assert_eq!(buffer.get_pixel(5, 1), Some(Color::WHITE));
    }

    #[test]
    fn scatter_stroke_never_writes_outside_masked_clip() {
        let mut buffer = PixelBuffer::new(15, 15);
        buffer.fill(Color::WHITE);
        let rect = Rect2i::new(8, 11, 3, 3);
        let mut mask = vec![false; 9];
        mask[4] = true;
        let selection = Selection::capture_mask(&buffer, rect, mask).unwrap();
        let clip = PixelClip::from_selection(&selection);
        let before = buffer.as_bytes().to_vec();
        let paint = Color::BLACK;
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, paint)
            .with_scatter(8)
            .with_jitter_seed(7)
            .with_clip(Some(clip.clone()));

        stroke.start(&mut buffer, 4, 4);
        stroke.continue_to(&mut buffer, 4, 4);

        assert_eq!(buffer.get_pixel(9, 12), Some(paint));
        for y in 0usize..15 {
            for x in 0usize..15 {
                if !clip.contains(x as i32, y as i32) {
                    let offset = (y * buffer.width() + x) * 4;
                    assert_eq!(
                        &buffer.as_bytes()[offset..offset + 4],
                        &before[offset..offset + 4],
                        "({x},{y}) must remain byte-identical"
                    );
                }
            }
        }
    }

    #[test]
    fn stroke_starting_outside_clip_paints_only_crossing_inside_cells() {
        let mut buffer = PixelBuffer::new(6, 3);
        buffer.fill(Color::WHITE);
        let selection = Selection::capture(&buffer, Rect2i::new(2, 1, 3, 1)).unwrap();
        let clip = PixelClip::from_selection(&selection);
        let paint = Color::BLACK;
        let mut stroke =
            Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, paint).with_clip(Some(clip));

        stroke.start(&mut buffer, 0, 1);
        stroke.continue_to(&mut buffer, 5, 1);

        assert_eq!(buffer.get_pixel(0, 1), Some(Color::WHITE));
        assert_eq!(buffer.get_pixel(1, 1), Some(Color::WHITE));
        for x in 2..5 {
            assert_eq!(buffer.get_pixel(x, 1), Some(paint));
        }
        assert_eq!(buffer.get_pixel(5, 1), Some(Color::WHITE));
    }

    #[test]
    fn clipped_stroke_paints_each_cell_once_with_translucent_ink() {
        let mut buffer = PixelBuffer::new(4, 3);
        let selection = Selection::capture(&buffer, Rect2i::new(2, 1, 1, 1)).unwrap();
        let clip = PixelClip::from_selection(&selection);
        let paint = Color::rgba(0, 0, 0, 128);
        let mut stroke =
            Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, paint).with_clip(Some(clip));

        stroke.start(&mut buffer, 2, 1);
        stroke.continue_to(&mut buffer, 2, 1);

        assert_eq!(buffer.get_pixel(2, 1), Some(paint));
    }

    #[test]
    fn stroke_record_capture_matches_buffer_region() {
        let mut buf = PixelBuffer::new(8, 8);
        buf.fill(Color::rgb(30, 60, 90));
        buf.set_pixel(2, 2, Color::WHITE);
        let region = Rect2i::new(1, 1, 4, 4);
        let record = StrokeRecord::capture_region(&buf, region).unwrap();
        assert_eq!(record.region, region);
        assert_eq!(record.before, buf.export_region(region, None).unwrap());
    }

    #[test]
    fn stroke_record_undo_reversibility() {
        let mut buf = PixelBuffer::new(8, 8);
        buf.fill(Color::rgb(30, 60, 90));
        buf.set_pixel(4, 4, Color::rgb(1, 1, 1)); // pre-existing mark

        let mut stroke = Stroke::new(spec(3, BrushShape::Round), DrawMode::Pen, Color::WHITE);
        stroke.start(&mut buf, 3, 3);
        stroke.continue_to(&mut buf, 4, 3);
        let region = stroke.bounding_box();

        // Rebuild the pre-stroke state and capture the delta seam.
        let mut pristine = PixelBuffer::new(8, 8);
        pristine.fill(Color::rgb(30, 60, 90));
        pristine.set_pixel(4, 4, Color::rgb(1, 1, 1));
        let record = StrokeRecord::capture_region(&pristine, region).unwrap();

        // Redo the stroke on the pristine buffer.
        let mut stroke2 = Stroke::new(spec(3, BrushShape::Round), DrawMode::Pen, Color::WHITE);
        stroke2.start(&mut pristine, 3, 3);
        stroke2.continue_to(&mut pristine, 4, 3);
        assert_ne!(pristine.export_region(region, None).unwrap(), record.before);

        // Undo: re-blit the before-bytes → exact original restored.
        assert!(pristine.blit_region(record.region, &record.before));
        assert_eq!(pristine.export_region(region, None).unwrap(), record.before);
        assert_eq!(pristine.get_pixel(4, 4), Some(Color::rgb(1, 1, 1)));
    }

    #[test]
    fn stroke_record_out_of_bounds_is_none() {
        let buf = PixelBuffer::new(4, 4);
        assert!(StrokeRecord::capture_region(&buf, Rect2i::new(0, 0, 8, 8)).is_none());
        assert!(StrokeRecord::capture_region(&buf, Rect2i::new(-1, 0, 2, 2)).is_none());
    }

    #[test]
    fn bounding_box_size1() {
        let mut buf = PixelBuffer::new(8, 8);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        stroke.start(&mut buf, 1, 1);
        stroke.continue_to(&mut buf, 3, 1);
        assert_eq!(stroke.bounding_box(), Rect2i::new(1, 1, 3, 1));
    }

    #[test]
    fn bounding_box_size3_centered_square() {
        let mut buf = PixelBuffer::new(8, 8);
        let mut stroke = Stroke::new(spec(3, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        stroke.start(&mut buf, 1, 1);
        stroke.continue_to(&mut buf, 3, 1);
        assert_eq!(stroke.bounding_box(), Rect2i::new(0, 0, 5, 3));
    }

    #[test]
    fn bounding_box_clamped_to_bounds() {
        let mut buf = PixelBuffer::new(8, 8);
        let mut stroke = Stroke::new(spec(3, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        stroke.set_bounds(Rect2i::new(0, 0, 8, 8));
        stroke.start(&mut buf, 1, 1);
        stroke.continue_to(&mut buf, 3, 1);
        assert_eq!(stroke.bounding_box(), Rect2i::new(0, 0, 5, 3));
    }

    #[test]
    fn bounding_box_empty_before_start() {
        let stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        assert_eq!(stroke.bounding_box(), Rect2i::ZERO);
        assert!(!stroke.is_started());
        assert_eq!(stroke.last_pos(), (0, 0));
    }

    #[test]
    fn continue_to_before_start_delegates_to_start() {
        let mut buf = PixelBuffer::new(8, 8);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        stroke.continue_to(&mut buf, 2, 3);
        assert!(stroke.is_started());
        assert_eq!(stroke.last_pos(), (2, 3));
        assert_eq!(buf.get_pixel(2, 3), Some(Color::WHITE));
    }

    #[test]
    fn out_of_bounds_start_skips_stamp_but_updates_last() {
        let mut buf = PixelBuffer::new(4, 4);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        stroke.start(&mut buf, -5, -5);
        assert!(stroke.is_started());
        assert_eq!(stroke.last_pos(), (-5, -5));
        assert_eq!(stroke.bounding_box(), Rect2i::ZERO);
        // Continuing from an out-of-bounds point into bounds draws the line.
        stroke.continue_to(&mut buf, 0, 0);
        assert_eq!(buf.get_pixel(0, 0), Some(Color::WHITE));
    }

    #[test]
    fn draw_to_and_erase_to_single_point() {
        let mut buf = PixelBuffer::new(8, 8);
        draw_to(
            &mut buf,
            spec(2, BrushShape::Square),
            1,
            1,
            Color::rgb(1, 2, 3),
        );
        assert_eq!(buf.get_pixel(1, 1), Some(Color::rgb(1, 2, 3)));
        assert_eq!(buf.get_pixel(0, 0), Some(Color::rgb(1, 2, 3)));
        erase_to(&mut buf, spec(2, BrushShape::Square), 1, 1);
        assert_eq!(buf.get_pixel(1, 1), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn pen_opaque_replaces_destination() {
        let dst = Color::rgba(200, 100, 50, 128);
        let src = Color::rgba(10, 20, 30, 255);
        assert_eq!(blend_over_pixel(dst, src), src);
    }

    #[test]
    fn pen_half_alpha_over_opaque_matches_aseprite_mix() {
        let dst = Color::rgb(0, 0, 255);
        let src = Color::rgba(255, 0, 0, 128);
        assert_eq!(blend_over_pixel(dst, src), Color::rgba(128, 0, 127, 255));
    }

    #[test]
    fn pen_half_alpha_over_transparent_yields_src() {
        let src = Color::rgba(255, 0, 0, 128);
        assert_eq!(blend_over_pixel(Color::TRANSPARENT, src), src);
    }

    #[test]
    fn pen_zero_alpha_erases_pixel() {
        let dst = Color::rgb(200, 100, 50);
        let src = Color::rgba(255, 0, 0, 0);
        assert_eq!(blend_over_pixel(dst, src), Color::TRANSPARENT);
    }

    #[test]
    fn pen_repeated_strokes_accumulate_alpha() {
        let src = Color::rgba(255, 0, 0, 128);
        let once = blend_over_pixel(Color::TRANSPARENT, src);
        assert_eq!(once, src);
        let twice = blend_over_pixel(once, src);
        assert_eq!(twice, Color::rgba(255, 0, 0, 192));
        assert!(twice.a > once.a, "alpha must accumulate toward opaque");
    }

    #[test]
    fn eraser_full_alpha_zeroes_rgb() {
        assert_eq!(
            subtract_alpha(Color::rgb(200, 100, 50), 255),
            Color::TRANSPARENT
        );
    }

    #[test]
    fn eraser_half_alpha_halves_alpha_keeps_rgb() {
        assert_eq!(
            subtract_alpha(Color::rgb(200, 100, 50), 128),
            Color::rgba(200, 100, 50, 127)
        );
    }

    #[test]
    fn eraser_zero_alpha_is_noop() {
        assert_eq!(
            subtract_alpha(Color::rgb(200, 100, 50), 0),
            Color::rgb(200, 100, 50)
        );
    }

    #[test]
    fn eraser_clamps_alpha_to_zero() {
        assert_eq!(
            subtract_alpha(Color::rgba(200, 100, 50, 40), 255),
            Color::TRANSPARENT
        );
    }

    #[test]
    fn size_64_offsets_and_bbox_are_sane() {
        for shape in [BrushShape::Square, BrushShape::Round] {
            let offsets = spec(64, shape).stamp_offsets();
            let min_x = offsets.iter().map(|&(x, _)| x).min().unwrap();
            let max_x = offsets.iter().map(|&(x, _)| x).max().unwrap();
            let min_y = offsets.iter().map(|&(_, y)| y).min().unwrap();
            let max_y = offsets.iter().map(|&(_, y)| y).max().unwrap();
            assert_eq!((min_x, max_x), (-32, 31), "{shape:?} x span");
            assert_eq!((min_y, max_y), (-32, 31), "{shape:?} y span");
            assert_eq!(
                stamp_bbox(spec(64, shape), 0, 0),
                Rect2i::new(-32, -32, 64, 64)
            );
        }
        // Square size 64 covers the full block.
        assert_eq!(spec(64, BrushShape::Square).stamp_offsets().len(), 64 * 64);
        // Round size 64 is a real circle: strictly smaller than the block and
        // larger than half of it (the shape never snaps to a square at even
        // sizes).
        let offsets = spec(64, BrushShape::Round).stamp_offsets();
        assert!(offsets.contains(&(0, 0)));
        assert!(offsets.len() < 64 * 64);
        assert!(offsets.len() > 64 * 64 / 2);
        // Odd size 63 round: a centered circle, strictly smaller than the block.
        let offsets = spec(63, BrushShape::Round).stamp_offsets();
        assert!(offsets.contains(&(0, 0)));
        assert!(offsets.len() < 63 * 63);
        assert!(offsets.len() > 63 * 63 / 2);
        let min_x = offsets.iter().map(|&(x, _)| x).min().unwrap();
        let max_x = offsets.iter().map(|&(x, _)| x).max().unwrap();
        assert_eq!((min_x, max_x), (-31, 31));
    }

    #[test]
    fn stamp_at_pen_blends_over_existing_pixels() {
        let mut buf = PixelBuffer::new(4, 4);
        buf.fill(Color::rgb(0, 0, 255));
        stamp_at(
            &mut buf,
            pen(spec(1, BrushShape::Square), Color::rgba(255, 0, 0, 128)),
            2,
            2,
        );
        assert_eq!(buf.get_pixel(2, 2), Some(Color::rgba(128, 0, 127, 255)));
    }

    #[test]
    fn stamp_at_eraser_subtracts_alpha() {
        let mut buf = PixelBuffer::new(4, 4);
        buf.fill(Color::rgb(200, 100, 50));
        stamp_at(
            &mut buf,
            eraser(spec(1, BrushShape::Square), Color::rgba(0, 0, 0, 128)),
            2,
            2,
        );
        assert_eq!(buf.get_pixel(2, 2), Some(Color::rgba(200, 100, 50, 127)));
    }

    #[test]
    fn scatter_zero_reproduces_the_plain_stroke() {
        let brush = spec(3, BrushShape::Round);
        let mut plain = PixelBuffer::new(16, 16);
        let mut scattered = PixelBuffer::new(16, 16);
        let mut a = Stroke::new(brush, DrawMode::Pen, Color::WHITE);
        a.start(&mut plain, 2, 8);
        a.continue_to(&mut plain, 10, 8);
        a.continue_to(&mut plain, 12, 10);
        let mut b = Stroke::new(brush, DrawMode::Pen, Color::WHITE)
            .with_scatter(0)
            .with_jitter_seed(0xDEAD_BEEF);
        b.start(&mut scattered, 2, 8);
        b.continue_to(&mut scattered, 10, 8);
        b.continue_to(&mut scattered, 12, 10);
        assert_eq!(
            plain.as_bytes(),
            scattered.as_bytes(),
            "scatter 0 must be byte-identical to a plain stroke"
        );
        assert_eq!(a.bounding_box(), b.bounding_box());

        let mut expected = PixelBuffer::new(16, 16);
        for (px, py) in bresenham_line((2, 8), (10, 8))
            .into_iter()
            .chain(bresenham_line((10, 8), (12, 10)))
        {
            for (dx, dy) in brush.stamp_offsets() {
                expected.set_pixel((px + dx) as usize, (py + dy) as usize, Color::WHITE);
            }
        }
        assert_eq!(
            plain.as_bytes(),
            expected.as_bytes(),
            "scatter 0 must equal the un-jittered stamp union"
        );
    }

    #[test]
    fn scatter_stamps_stay_within_the_radius() {
        for seed in [0u32, 1, 7, 42, 12_345, 0xFFFF_FF00] {
            for scatter in [1u8, 4, 9, 32] {
                let brush = spec(3, BrushShape::Round);
                let mut buf = PixelBuffer::new(64, 64);
                let mut stroke = Stroke::new(brush, DrawMode::Pen, Color::WHITE)
                    .with_scatter(scatter)
                    .with_jitter_seed(seed);
                stroke.start(&mut buf, 8, 8);
                stroke.continue_to(&mut buf, 55, 10);
                stroke.continue_to(&mut buf, 58, 55);
                let path: Vec<(i32, i32)> = bresenham_line((8, 8), (55, 10))
                    .into_iter()
                    .chain(bresenham_line((55, 10), (58, 55)))
                    .collect();
                let reach = i32::from(scatter) + i32::from(brush.size) / 2;
                for y in 0..64usize {
                    for x in 0..64usize {
                        if buf.get_pixel(x, y) != Some(Color::TRANSPARENT) {
                            let near_path = path.iter().any(|&(cx, cy)| {
                                (x as i32 - cx).abs() <= reach && (y as i32 - cy).abs() <= reach
                            });
                            assert!(
                                near_path,
                                "seed {seed} scatter {scatter}: painted ({x},{y}) lies outside scatter+footprint of the cursor path"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn scatter_paints_off_the_centerline() {
        let brush = spec(1, BrushShape::Square);
        let mut a = PixelBuffer::new(32, 16);
        let mut first = Stroke::new(brush, DrawMode::Pen, Color::WHITE)
            .with_scatter(4)
            .with_jitter_seed(20_240_915);
        first.start(&mut a, 4, 8);
        first.continue_to(&mut a, 24, 8);
        let painted: Vec<(i32, i32)> = (0..32usize)
            .flat_map(|x| (0..16usize).map(move |y| (x, y)))
            .filter(|&(x, y)| a.get_pixel(x, y) == Some(Color::WHITE))
            .map(|(x, y)| (x as i32, y as i32))
            .collect();
        assert!(!painted.is_empty(), "the jittered stroke must paint");
        assert!(
            painted.iter().any(|&(_, y)| y != 8),
            "jitter must paint off the centerline y=8: {painted:?}"
        );

        let mut b = PixelBuffer::new(32, 16);
        let mut replay = Stroke::new(brush, DrawMode::Pen, Color::WHITE)
            .with_scatter(4)
            .with_jitter_seed(20_240_915);
        replay.start(&mut b, 4, 8);
        replay.continue_to(&mut b, 24, 8);
        assert_eq!(
            a.as_bytes(),
            b.as_bytes(),
            "a pinned seed must reproduce the stroke byte-exactly"
        );
    }

    #[test]
    fn scatter_undo_region_covers_jittered_pixels() {
        let bg = Color::rgb(3, 4, 5);
        for seed in [3u32, 77, 0xC0FFEE] {
            let brush = spec(3, BrushShape::Round);
            let mut buf = PixelBuffer::new(48, 48);
            buf.fill(bg);
            let mut stroke = Stroke::new(brush, DrawMode::Pen, Color::WHITE)
                .with_scatter(7)
                .with_jitter_seed(seed);
            stroke.start(&mut buf, 10, 24);
            stroke.continue_to(&mut buf, 38, 24);
            stroke.continue_to(&mut buf, 38, 38);
            let region = stroke.bounding_box();
            assert!(!region.is_empty(), "seed {seed}: the stroke must paint");
            for y in 0..48usize {
                for x in 0..48usize {
                    if buf.get_pixel(x, y) != Some(bg) {
                        assert!(
                            region.contains(x as i32, y as i32),
                            "seed {seed}: touched pixel ({x},{y}) lies outside the undo region {region:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn with_scatter_clamps_to_the_max_radius() {
        let clamped =
            Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE).with_scatter(200);
        assert_eq!(clamped.tool.scatter, DrawTool::MAX_SCATTER);
        let kept =
            Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::WHITE).with_scatter(7);
        assert_eq!(kept.tool.scatter, 7);
    }

    #[test]
    fn odd_brush_is_centered_on_the_cursor_pixel() {
        for shape in [BrushShape::Square, BrushShape::Round] {
            for size in [1u8, 3, 5, 7] {
                let offsets = spec(size, shape).stamp_offsets();
                assert!(
                    offsets.contains(&(0, 0)),
                    "size {size} {shape:?}: the cursor pixel (0,0) must be in the footprint"
                );
                for &(dx, dy) in &offsets {
                    assert!(
                        offsets.contains(&(-dx, -dy)),
                        "size {size} {shape:?}: ({dx},{dy}) lacks mirror (-{dx},-{dy})"
                    );
                }
            }
        }
    }

    #[test]
    fn even_brush_middle_four_are_centered_on_the_cursor() {
        let middle_four = [(-1, -1), (0, -1), (-1, 0), (0, 0)];
        for shape in [BrushShape::Square, BrushShape::Round] {
            for size in [2u8, 4, 6] {
                let offsets = spec(size, shape).stamp_offsets();
                for corner in middle_four {
                    assert!(
                        offsets.contains(&corner),
                        "size {size} {shape:?}: middle-four point {corner:?} must be present"
                    );
                }
                for &(dx, dy) in &offsets {
                    let point_mirror = (-1 - dx, -1 - dy);
                    assert!(
                        offsets.contains(&point_mirror),
                        "size {size} {shape:?}: ({dx},{dy}) lacks mirror {point_mirror:?}"
                    );
                    let axis_swap = (dy, dx);
                    assert!(
                        offsets.contains(&axis_swap),
                        "size {size} {shape:?}: ({dx},{dy}) lacks axis swap {axis_swap:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn even_brush_painted_block_is_centered_on_the_cursor_pixel() {
        // The painted block's geometric center (in pixel-edge coordinates) must
        // be the cursor pixel: the middle four pixels (-1,-1),(0,-1),(-1,0),(0,0)
        // straddle the lattice point (0,0), so their exact center coincides with
        // the cursor. Stamping at (5,5) paints a block spanning [4,6)² whose
        // center is (5,5).
        for shape in [BrushShape::Square, BrushShape::Round] {
            for size in [2u8, 4, 6] {
                let mut buf = PixelBuffer::new(16, 16);
                stamp_at(&mut buf, pen(spec(size, shape), Color::WHITE), 5, 5);
                let painted: Vec<(i32, i32)> = (0..16i32)
                    .flat_map(|x| (0..16i32).map(move |y| (x, y)))
                    .filter(|&(x, y)| buf.get_pixel(x as usize, y as usize) == Some(Color::WHITE))
                    .collect();
                assert!(!painted.is_empty(), "size {size} {shape:?} must paint");
                let min_x = painted.iter().map(|&(x, _)| x).min().unwrap();
                let max_x = painted.iter().map(|&(x, _)| x).max().unwrap();
                let min_y = painted.iter().map(|&(_, y)| y).min().unwrap();
                let max_y = painted.iter().map(|&(_, y)| y).max().unwrap();
                assert_eq!(
                    (min_x + max_x + 1) as f32 / 2.0,
                    5.0,
                    "size {size} {shape:?}: painted block x-center must be the cursor pixel 5"
                );
                assert_eq!(
                    (min_y + max_y + 1) as f32 / 2.0,
                    5.0,
                    "size {size} {shape:?}: painted block y-center must be the cursor pixel 5"
                );
            }
        }
    }

    #[test]
    fn scatter_circle_shape_stays_within_the_radius_and_is_area_uniform() {
        let scatter = 32u8;
        let radius = i32::from(scatter);
        let mut jitter = StampJitter::new(0x1234_5678);
        let samples = 20_000u32;
        let mut radius_sum = 0.0f64;
        let mut quadrants = [0u32; 4];
        for _ in 0..samples {
            let (dx, dy) = jitter.next_offset(scatter, ScatterShape::Circle);
            assert!(
                dx * dx + dy * dy <= radius * radius,
                "circle sample ({dx},{dy}) escapes the disc"
            );
            radius_sum += ((dx * dx + dy * dy) as f64).sqrt();
            let quadrant = usize::from(dx < 0) * 2 + usize::from(dy < 0);
            quadrants[quadrant] += 1;
        }
        let mean = radius_sum / f64::from(samples);
        let expected = 2.0 * f64::from(scatter) / 3.0;
        assert!(
            (mean - expected).abs() < 0.5,
            "circle mean radius {mean} must be near {expected}"
        );
        for (quadrant, count) in quadrants.iter().enumerate() {
            let share = f64::from(*count) / f64::from(samples);
            assert!(
                (share - 0.25).abs() < 0.03,
                "circle quadrant {quadrant} share {share} is unbalanced"
            );
        }
    }

    #[test]
    fn scatter_diamond_shape_stays_within_the_diamond() {
        let scatter = 32u8;
        let radius = i32::from(scatter);
        let mut jitter = StampJitter::new(0x1234_5678);
        let samples = 20_000u32;
        let mut distance_sum = 0.0f64;
        for _ in 0..samples {
            let (dx, dy) = jitter.next_offset(scatter, ScatterShape::Diamond);
            let distance = dx.abs() + dy.abs();
            assert!(
                distance <= radius,
                "diamond sample ({dx},{dy}) escapes |dx|+|dy| <= {radius}"
            );
            distance_sum += f64::from(distance);
        }
        let mean = distance_sum / f64::from(samples);
        let expected = 2.0 * f64::from(scatter) / 3.0;
        assert!(
            (mean - expected).abs() < 0.6,
            "diamond mean |dx|+|dy| {mean} must be near {expected}"
        );
    }

    #[test]
    fn scatter_square_shape_unchanged() {
        let mut jitter = StampJitter::new(0xDEAD_BEEF);
        let expected = [
            (3, -3),
            (1, -3),
            (-4, 4),
            (2, -1),
            (-4, -2),
            (-1, 0),
            (-1, -5),
            (2, 5),
        ];
        for (index, &(dx, dy)) in expected.iter().enumerate() {
            assert_eq!(
                jitter.next_offset(5, ScatterShape::Square),
                (dx, dy),
                "square sample {index} must match the original box sampling"
            );
        }
    }

    fn column_height(buf: &PixelBuffer, x: usize) -> usize {
        (0..buf.height())
            .filter(|&y| buf.get_pixel(x, y) != Some(Color::TRANSPARENT))
            .count()
    }

    #[test]
    fn tail_zero_is_byte_identical() {
        let brush = spec(9, BrushShape::Round);
        let mut plain = PixelBuffer::new(160, 40);
        let mut zero = PixelBuffer::new(160, 40);
        let mut a = Stroke::new(brush, DrawMode::Pen, Color::WHITE);
        a.start(&mut plain, 4, 20);
        a.continue_to(&mut plain, 140, 20);
        let mut b = Stroke::new(brush, DrawMode::Pen, Color::WHITE).with_tail(0);
        b.start(&mut zero, 4, 20);
        b.continue_to(&mut zero, 140, 20);
        assert_eq!(
            plain.as_bytes(),
            zero.as_bytes(),
            "tail 0 must be byte-identical to a plain stroke"
        );
        assert_eq!(a.bounding_box(), b.bounding_box());
    }

    #[test]
    fn tail_positive_grows_one_pixel_per_step_distance() {
        // +10 = one extra pixel of brush size per 10 px of stroke travel.
        assert_eq!(tail_size(10, 10, 0.0), 10);
        assert_eq!(tail_size(10, 10, 9.0), 10);
        assert_eq!(tail_size(10, 10, 10.0), 11);
        assert_eq!(tail_size(10, 10, 50.0), 15);
        let base = 10u8;
        let mut buf = PixelBuffer::new(128, 128);
        let mut stroke =
            Stroke::new(spec(base, BrushShape::Square), DrawMode::Pen, Color::WHITE).with_tail(10);
        stroke.start(&mut buf, 16, 64);
        stroke.continue_to(&mut buf, 66, 64);
        assert_eq!(
            column_height(&buf, 16),
            usize::from(base),
            "the stroke must start at the base size"
        );
        assert_eq!(
            column_height(&buf, 66),
            15,
            "50 px of travel at tail +10 must grow the 10 px brush to 15"
        );
    }

    #[test]
    fn tail_negative_shrinks_one_pixel_per_step_distance() {
        // -10 = one fewer pixel of brush size per 10 px of travel, floored.
        assert_eq!(tail_size(20, -10, 0.0), 20);
        assert_eq!(tail_size(20, -10, 10.0), 19);
        assert_eq!(tail_size(20, -10, 49.0), 16);
        assert_eq!(tail_size(20, -10, 50.0), 15);
        let base = 20u8;
        let mut buf = PixelBuffer::new(128, 128);
        let mut stroke =
            Stroke::new(spec(base, BrushShape::Square), DrawMode::Pen, Color::WHITE).with_tail(-10);
        stroke.start(&mut buf, 16, 64);
        stroke.continue_to(&mut buf, 66, 64);
        assert_eq!(
            column_height(&buf, 16),
            usize::from(base),
            "the stroke must start at the base size"
        );
        // The final size-15 stamp is one pixel narrower than the size-16 stamp
        // one step back, whose footprint still covers the end column: the
        // visible band is their union (15 + 1).
        assert_eq!(
            column_height(&buf, 66),
            16,
            "50 px of travel at tail -10 must shrink the 20 px brush to 15"
        );
        assert!(column_height(&buf, 66) < usize::from(base));
    }

    #[test]
    fn tail_clamps_at_min_and_max() {
        // A far-run shrink bottoms out at 1 px and a far-run grow tops out at 64.
        assert_eq!(tail_size(4, -1, 10.0), BrushSpec::MIN_SIZE);
        assert_eq!(tail_size(60, 1, 10.0), BrushSpec::MAX_SIZE);

        let mut min_buf = PixelBuffer::new(128, 128);
        let mut shrinking =
            Stroke::new(spec(4, BrushShape::Square), DrawMode::Pen, Color::WHITE).with_tail(-1);
        shrinking.start(&mut min_buf, 16, 64);
        shrinking.continue_to(&mut min_buf, 26, 64);
        assert_eq!(
            column_height(&min_buf, 26),
            usize::from(BrushSpec::MIN_SIZE)
        );

        let mut max_buf = PixelBuffer::new(128, 128);
        let mut growing =
            Stroke::new(spec(60, BrushShape::Square), DrawMode::Pen, Color::WHITE).with_tail(1);
        growing.start(&mut max_buf, 16, 64);
        growing.continue_to(&mut max_buf, 26, 64);
        assert_eq!(
            column_height(&max_buf, 26),
            usize::from(BrushSpec::MAX_SIZE)
        );
    }

    #[test]
    fn tail_one_pixel_per_step_accumulates_across_a_long_stroke() {
        // The size is an exact integer at every stamp: 50 px of travel at
        // tail +1 turns the 4 px base into 54 px.
        assert_eq!(tail_size(4, 1, 50.0), 54);
        let mut buf = PixelBuffer::new(128, 128);
        let mut stroke =
            Stroke::new(spec(4, BrushShape::Square), DrawMode::Pen, Color::WHITE).with_tail(1);
        stroke.start(&mut buf, 8, 64);
        stroke.continue_to(&mut buf, 58, 64);
        assert_eq!(
            column_height(&buf, 58),
            54,
            "one pixel per step must accumulate across the whole stroke"
        );
    }

    #[test]
    fn tail_range_is_minus_100_to_100() {
        assert_eq!(DrawTool::MAX_TAIL, 100);
        let mk = || Stroke::new(spec(8, BrushShape::Square), DrawMode::Pen, Color::WHITE);
        assert_eq!(mk().with_tail(0).tool.tail, 0);
        assert_eq!(mk().with_tail(100).tool.tail, 100, "+100 must pass through");
        assert_eq!(
            mk().with_tail(-100).tool.tail,
            -100,
            "-100 must pass through"
        );
        assert_eq!(mk().with_tail(127).tool.tail, 100, "above +100 clamps");
        assert_eq!(mk().with_tail(-128).tool.tail, -100, "below -100 clamps");
    }

    #[test]
    fn tail_undo_region_covers_the_largest_footprint() {
        let bg = Color::rgb(3, 4, 5);
        let mut buf = PixelBuffer::new(220, 128);
        buf.fill(bg);
        let mut stroke = Stroke::new(spec(24, BrushShape::Round), DrawMode::Pen, Color::WHITE)
            .with_tail(10)
            .with_scatter(3)
            .with_jitter_seed(0x5EED);
        stroke.start(&mut buf, 20, 64);
        stroke.continue_to(&mut buf, 180, 40);
        stroke.continue_to(&mut buf, 200, 90);
        let region = stroke.bounding_box();
        assert!(!region.is_empty(), "the tapered stroke must paint");
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                if buf.get_pixel(x, y) != Some(bg) {
                    assert!(
                        region.contains(x as i32, y as i32),
                        "touched pixel ({x},{y}) lies outside the undo region {region:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn pen_half_alpha_paints_each_pixel_once_per_stroke() {
        let half = Color::rgba(255, 0, 0, 128);
        let mut buf = PixelBuffer::new(16, 16);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, half);
        stroke.start(&mut buf, 4, 8);
        stroke.continue_to(&mut buf, 10, 8);
        // Back-and-forth over the same pixels: the second pass must not blend
        // the 50% source in again.
        stroke.continue_to(&mut buf, 4, 8);
        stroke.continue_to(&mut buf, 10, 8);
        for x in 4..=10 {
            assert_eq!(
                buf.get_pixel(x, 8),
                Some(half),
                "pixel ({x},8) must keep the 50% source alpha, not accumulate toward opaque"
            );
        }
    }

    #[test]
    fn eraser_subtracts_alpha_once_per_pixel_per_stroke() {
        let base = Color::rgb(200, 100, 50);
        let expected = Color::rgba(200, 100, 50, 127);
        let mut buf = PixelBuffer::new(16, 16);
        buf.fill(base);
        let mut stroke = Stroke::new(
            spec(1, BrushShape::Square),
            DrawMode::Eraser,
            Color::rgba(0, 0, 0, 128),
        );
        stroke.start(&mut buf, 4, 8);
        stroke.continue_to(&mut buf, 10, 8);
        stroke.continue_to(&mut buf, 4, 8);
        stroke.continue_to(&mut buf, 10, 8);
        for x in 4..=10 {
            assert_eq!(
                buf.get_pixel(x, 8),
                Some(expected),
                "pixel ({x},8) must lose the color's alpha exactly once per stroke"
            );
        }
    }

    #[test]
    fn separate_strokes_still_accumulate() {
        let half = Color::rgba(255, 0, 0, 128);
        let mut buf = PixelBuffer::new(16, 16);
        let mut first = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, half);
        first.start(&mut buf, 4, 8);
        assert_eq!(buf.get_pixel(4, 8), Some(half));
        let mut second = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, half);
        second.start(&mut buf, 4, 8);
        assert_eq!(
            buf.get_pixel(4, 8),
            Some(Color::rgba(255, 0, 0, 192)),
            "a second stroke must blend over the first, not be suppressed"
        );
    }

    #[test]
    fn scatter_and_tail_do_not_repaint_a_cell_within_a_stroke() {
        let half = Color::rgba(255, 0, 0, 128);
        let mut buf = PixelBuffer::new(48, 48);
        let mut stroke = Stroke::new(spec(5, BrushShape::Round), DrawMode::Pen, half)
            .with_scatter(6)
            .with_tail(100)
            .with_jitter_seed(0x5EED_1234);
        stroke.start(&mut buf, 12, 24);
        stroke.continue_to(&mut buf, 36, 24);
        // Return over the same band: jitter and the growing tail footprint
        // must not re-apply the blend to a cell the stroke already touched.
        stroke.continue_to(&mut buf, 12, 24);
        stroke.continue_to(&mut buf, 36, 24);
        let mut painted = 0;
        for y in 0..buf.height() {
            for x in 0..buf.width() {
                if let Some(px) = buf.get_pixel(x, y) {
                    if px != Color::TRANSPARENT {
                        painted += 1;
                        assert_eq!(
                            px, half,
                            "({x},{y}) must be painted exactly once within the stroke"
                        );
                    }
                }
            }
        }
        assert!(painted > 0, "the scattered tapered stroke must paint");
    }

    #[test]
    fn snap_line_angle_rounds_direction_to_22_5_degree_steps() {
        // A 45° drag is already a 22.5° multiple and stays put.
        assert_eq!(snap_line_angle((10, 10), (30, 30)), (30, 30));
        // A near-45° drag snaps exactly onto the 45° ray: equal offsets.
        let snapped = snap_line_angle((10, 10), (30, 32));
        assert_eq!(snapped.0 - 10, snapped.1 - 10);
        // A horizontal drag is on the 0° ray and is unaffected.
        let snapped = snap_line_angle((10, 10), (30, 10));
        assert_eq!(snapped, (30, 10));
        // A degenerate drag is a no-op.
        assert_eq!(snap_line_angle((10, 10), (10, 10)), (10, 10));
    }

    #[test]
    /// Given a masked clip crossing a stroke, when the stroke is drawn, then only selected cells change.
    fn stroke_mask_clip_paints_only_selected_cells() {
        let mut buffer = PixelBuffer::new(7, 3);
        buffer.fill(Color::WHITE);
        let selection = Selection::capture_mask(
            &buffer,
            Rect2i::new(1, 1, 5, 1),
            vec![true, false, true, true, false],
        )
        .unwrap();
        let clip = PixelClip::from_selection(&selection);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::BLACK)
            .with_clip(Some(clip));

        stroke.start(&mut buffer, 0, 1);
        stroke.continue_to(&mut buffer, 6, 1);

        assert_eq!(buffer.get_pixel(1, 1), Some(Color::BLACK));
        assert_eq!(buffer.get_pixel(2, 1), Some(Color::WHITE));
        assert_eq!(buffer.get_pixel(3, 1), Some(Color::BLACK));
        assert_eq!(buffer.get_pixel(4, 1), Some(Color::BLACK));
        assert_eq!(buffer.get_pixel(5, 1), Some(Color::WHITE));
        assert_eq!(buffer.get_pixel(0, 1), Some(Color::WHITE));
        assert_eq!(buffer.get_pixel(6, 1), Some(Color::WHITE));
    }

    #[test]
    /// Given no clip, when a stroke crosses a line, then every in-bounds cell on the line is painted.
    fn stroke_without_clip_still_paints_everywhere() {
        let mut buffer = PixelBuffer::new(5, 2);
        buffer.fill(Color::WHITE);
        let mut stroke = Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, Color::BLACK);

        stroke.start(&mut buffer, 0, 1);
        stroke.continue_to(&mut buffer, 4, 1);

        for x in 0..buffer.width() {
            assert_eq!(buffer.get_pixel(x, 1), Some(Color::BLACK));
        }
        assert_eq!(buffer.get_pixel(0, 0), Some(Color::WHITE));
    }

    #[test]
    /// Given a clip excludes the first stamp, when the stroke later reaches a selected cell, then that cell is still painted.
    fn stroke_mask_clip_does_not_mark_out_of_clip_cells_as_painted() {
        let mut buffer = PixelBuffer::new(8, 3);
        buffer.fill(Color::WHITE);
        let selection =
            Selection::capture_mask(&buffer, Rect2i::new(4, 1, 1, 1), vec![true]).unwrap();
        let clip = PixelClip::from_selection(&selection);
        let mut stroke = Stroke::new(spec(3, BrushShape::Square), DrawMode::Pen, Color::BLACK)
            .with_clip(Some(clip));

        stroke.start(&mut buffer, 0, 1);
        stroke.continue_to(&mut buffer, 4, 1);

        assert_eq!(buffer.get_pixel(4, 1), Some(Color::BLACK));
        assert_eq!(buffer.get_pixel(0, 1), Some(Color::WHITE));
        assert_eq!(buffer.get_pixel(1, 1), Some(Color::WHITE));
    }

    #[test]
    /// Given a clipped translucent stroke revisits selected cells, when it paints again, then each cell keeps one blend.
    fn stroke_mask_clip_keeps_paint_once_per_stroke_under_a_clip() {
        let mut buffer = PixelBuffer::new(6, 2);
        buffer.set_pixel(4, 1, Color::WHITE);
        let selection = Selection::capture_mask(
            &buffer,
            Rect2i::new(1, 1, 4, 1),
            vec![true, true, true, false],
        )
        .unwrap();
        let clip = PixelClip::from_selection(&selection);
        let paint = Color::rgba(255, 0, 0, 128);
        let mut stroke =
            Stroke::new(spec(1, BrushShape::Square), DrawMode::Pen, paint).with_clip(Some(clip));

        stroke.start(&mut buffer, 1, 1);
        stroke.continue_to(&mut buffer, 3, 1);
        stroke.continue_to(&mut buffer, 1, 1);
        stroke.continue_to(&mut buffer, 3, 1);

        for x in 1..=3 {
            assert_eq!(buffer.get_pixel(x, 1), Some(paint));
        }
        assert_eq!(buffer.get_pixel(0, 1), Some(Color::TRANSPARENT));
        assert_eq!(buffer.get_pixel(4, 1), Some(Color::WHITE));
    }

    #[test]
    /// Given a masked clip with scatter and tail, when the stroke runs, then every painted cell remains inside the clip.
    fn stroke_mask_clip_with_scatter_and_tail_stays_inside_the_clip() {
        let mut buffer = PixelBuffer::new(22, 12);
        buffer.fill(Color::WHITE);
        let rect = Rect2i::new(2, 2, 18, 7);
        let mut mask = vec![false; rect.area() as usize];
        for row in 1..=5 {
            for column in 0..rect.w {
                mask[(row * rect.w + column) as usize] = true;
            }
        }
        let selection = Selection::capture_mask(&buffer, rect, mask).unwrap();
        assert!(!selection.is_rectangular());
        let clip = PixelClip::from_selection(&selection);
        let mut stroke = Stroke::new(spec(3, BrushShape::Square), DrawMode::Pen, Color::BLACK)
            .with_scatter(2)
            .with_tail(1)
            .with_jitter_seed(7)
            .with_clip(Some(clip.clone()));

        stroke.start(&mut buffer, 6, 5);
        stroke.continue_to(&mut buffer, 14, 5);

        assert_eq!(buffer.get_pixel(10, 5), Some(Color::BLACK));
        for y in 0..buffer.height() {
            for x in 0..buffer.width() {
                if buffer.get_pixel(x, y) == Some(Color::BLACK) {
                    assert!(
                        clip.contains(x as i32, y as i32),
                        "painted ({x},{y}) must be inside the clip"
                    );
                }
            }
        }
        assert_eq!(buffer.get_pixel(2, 2), Some(Color::WHITE));
    }
}
