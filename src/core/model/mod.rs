//! Document data model — Layer, LayerStack, blend modes, compositing.
//!
//! ## Compositing
//!
//! [`Compositing`] is the pure, headless-testable straight-alpha compositing
//! primitive (ARCHITECTURE.md §5). It composites a bottom-to-top stack of
//! [`PixelBuffer`] layers over a `canvas_width × canvas_height` canvas into a
//! fresh RGBA8 `Vec<u8>`.
//!
//! ### Straight-alpha "over"
//!
//! Per pixel, bottom layer first. With `src` = the layer being placed on top
//! and `dst` = the accumulated result below it:
//!
//! ```text
//! out.a   = src.a + dst.a * (1 - src.a)
//! out.rgb = (src.rgb * src.a + dst.rgb * dst.a * (1 - src.a)) / out.a
//! ```
//!
//! Alpha and RGB are normalized to `0..=1` `f32` for the math, then rounded
//! back to `u8` (nearest, via `f32::round`). When `out.a == 0` the RGB
//! channels are undefined and written as `0` (fully transparent pixel).
//!
//! ### Semantics
//!
//! - Layers are passed bottom-to-top: index 0 is the bottom layer.
//! - Layer pixels are read in canvas space via [`PixelBuffer::get_pixel`];
//!   reads outside a layer's own bounds are fully transparent (a layer
//!   smaller than the canvas is padded with transparency).
//! - `composite_region` clips `rect` to the canvas; the returned buffer is
//!   `clipped_w * clipped_h * 4` bytes.
//! - Returns `None` when `rect` is empty or does not intersect the canvas.
//! - **Panic guarantees: none.** Out-of-bounds reads, empty rects, and
//!   zero-sized canvases are handled gracefully.
//!
//! ## LayerStack
//!
//! [`LayerStack`] is the document model: an ordered, bottom-to-top stack of
//! [`Layer`]s over a fixed-size canvas. Each layer carries a [`BlendMode`],
//! an opacity in `0..=1`, a visibility flag, and its own [`PixelBuffer`].
//! The active layer is tracked by [`LayerId`] (object identity), never by
//! index; ids are monotonic and never reused.
//!
//! Layers can be nested into groups. The stack stays a flat `Vec<Layer>`
//! kept in DFS pre-order: a group's descendants occupy a contiguous subrange
//! immediately after the group, in child order, before the group's next
//! sibling. Tree operations live in [`group`]; recursive compositing lives
//! in [`composite`].
//!
//! [`LayerStack::composite_layers`] and
//! [`LayerStack::composite_layers_region`] are the layer-aware compositing
//! entry points: hidden layers are skipped, opacity premultiplies the source
//! alpha, and the blend mode is applied to the source color against the
//! accumulated backdrop before the straight-alpha [`over`] math. When the
//! accumulated destination alpha is zero the blend is skipped and the plain
//! `over` result is used. Groups are isolated: their children composite into
//! a scratch buffer first, then the group's own opacity and blend apply once
//! to the combined result. Flat stacks (no groups) composite byte-identically
//! to the pre-group implementation.
//!
//! This module imports only `crate::core::{buffer, color, math}`, `serde`,
//! and `std` — no egui/wgpu/winit, keeping the core layer pure
//! (ARCHITECTURE.md §8).

use serde::{Deserialize, Serialize};

use crate::core::buffer::PixelBuffer;
use crate::core::color::Color;
use crate::core::math::Rect2i;

// R3 frame model: the canvas is the sprite sheet; frames are rect windows
// into it — never pixel copies.
pub mod composite;
pub mod frame;
pub mod group;
pub mod region;
pub mod sequence;

pub use composite::composite_over;
pub use frame::Frame;
pub use region::Region;
pub use sequence::AnimationSequence;

use composite::over;

/// Stable identifier for a layer. Monotonic; ids are never reused.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct LayerId(u64);

impl LayerId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Blend mode applied to a layer's color against the accumulated backdrop.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Add,
}

/// A single drawable layer: a pixel buffer plus its stack attributes.
///
/// Groups are layers with `is_group == true` and an empty (transparent)
/// buffer; their pixels come from compositing their children.
#[derive(Clone, PartialEq, Debug)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub buffer: PixelBuffer,
    /// Parent group id; `None` for a root-level layer.
    pub parent: Option<LayerId>,
    /// True for group layers (which carry an empty buffer).
    pub is_group: bool,
}

/// Bottom-to-top stack of layers over a fixed-size canvas.
///
/// `layers[0]` is the bottom layer. The active layer is tracked by
/// [`LayerId`] (object identity), never by index. `changed` is a single
/// coarse dirty flag set by every mutating method (D58).
pub struct LayerStack {
    layers: Vec<Layer>,
    active: LayerId,
    next_id: u64,
    changed: bool,
    width: usize,
    height: usize,
}

impl LayerStack {
    /// Creates a stack with one default layer "Layer 1": visible, opacity
    /// 1.0, [`BlendMode::Normal`], a transparent buffer of the given size.
    /// That layer becomes active.
    pub fn new(width: usize, height: usize) -> Self {
        match Self::try_new(width, height) {
            Some(stack) => stack,
            None => panic!("layer stack dimensions exceed addressable memory"),
        }
    }

    /// Fallible constructor for dimensions supplied by an external boundary.
    pub fn try_new(width: usize, height: usize) -> Option<Self> {
        let id = LayerId(1);
        let layer = Layer {
            id,
            name: "Layer 1".to_string(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            buffer: PixelBuffer::try_new(width, height)?,
            parent: None,
            is_group: false,
        };
        Some(Self {
            layers: vec![layer],
            active: id,
            next_id: 2,
            changed: false,
            width,
            height,
        })
    }

    /// Appends a new visible, opaque, [`BlendMode::Normal`] layer on top.
    /// The active layer is unchanged.
    pub fn add_layer(&mut self, name: &str) -> LayerId {
        let id = LayerId(self.next_id);
        self.next_id = match self.next_id.checked_add(1) {
            Some(next_id) => next_id,
            None => panic!("layer id space exhausted"),
        };
        self.add_layer_with_buffer(id, name, PixelBuffer::new(self.width, self.height))
    }

    fn add_layer_with_buffer(&mut self, id: LayerId, name: &str, buffer: PixelBuffer) -> LayerId {
        self.layers.push(Layer {
            id,
            name: name.to_string(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            buffer,
            parent: None,
            is_group: false,
        });
        self.changed = true;
        id
    }

    /// Adds a layer after all region validation and pixel writes have succeeded.
    /// Once the input checks pass, this operation cannot fail or partially mutate.
    pub fn add_layer_with_region(
        &mut self,
        name: &str,
        rect: Rect2i,
        bytes: &[u8],
    ) -> Option<LayerId> {
        let width = usize::try_from(rect.w).ok()?;
        let height = usize::try_from(rect.h).ok()?;
        let expected = width.checked_mul(height)?.checked_mul(4)?;
        if rect.x < 0
            || rect.y < 0
            || rect.right() > self.width as i32
            || rect.bottom() > self.height as i32
            || bytes.len() != expected
        {
            return None;
        }
        let mut buffer = PixelBuffer::new(self.width, self.height);
        if !buffer.blit_region(rect, bytes) {
            return None;
        }
        let id = LayerId(self.next_id);
        self.next_id = self.next_id.checked_add(1)?;
        Some(self.add_layer_with_buffer(id, name, buffer))
    }

    /// Inserts an existing layer at `index` (bottom-to-top), preserving its
    /// id and buffer. Used by structural undo to restore a removed layer at
    /// its original position with its original identity (D59: undo/redo
    /// restore the layer array by object identity).
    ///
    /// Returns `false` when `index` is out of bounds or a layer with the same
    /// id already exists (ids must stay unique). `next_id` is bumped past the
    /// inserted id so future [`LayerStack::add_layer`] calls never collide.
    pub fn insert_layer(&mut self, layer: Layer, index: usize) -> bool {
        if index > self.layers.len() || self.layers.iter().any(|l| l.id == layer.id) {
            return false;
        }
        let next_id = match layer.id.as_u64().checked_add(1) {
            Some(next_id) => next_id,
            None => return false,
        };
        self.next_id = self.next_id.max(next_id);
        self.layers.insert(index, layer);
        self.changed = true;
        true
    }

    /// Removes a layer, returning it. Removing a group also removes its whole
    /// subtree (the group and all descendants), keeping the stack a valid DFS
    /// pre-order tree. The last remaining layer cannot be removed (`None`).
    /// Removing the active layer (or a group containing it) re-selects a
    /// valid one.
    pub fn remove_layer(&mut self, id: LayerId) -> Option<Layer> {
        if self.layers.len() <= 1 {
            return None;
        }
        let pos = self.layers.iter().position(|l| l.id == id)?;
        let is_group = self.layers[pos].is_group;
        let end = if is_group {
            self.subtree_end(id)
        } else {
            pos + 1
        };
        let active_removed = (pos..end).any(|i| self.layers[i].id == self.active);
        let removed = self.layers.remove(pos);
        if end > pos + 1 {
            self.layers.drain(pos..end - 1);
        }
        if active_removed {
            self.active = self.layers[pos.min(self.layers.len() - 1)].id;
        }
        self.changed = true;
        Some(removed)
    }

    /// Moves a layer to `new_index` (bottom-to-top). Returns false when the
    /// id is unknown, the index is out of bounds, or the position is
    /// unchanged.
    pub fn reorder(&mut self, id: LayerId, new_index: usize) -> bool {
        let pos = match self.layers.iter().position(|l| l.id == id) {
            Some(p) => p,
            None => return false,
        };
        if new_index >= self.layers.len() || new_index == pos {
            return false;
        }
        let layer = self.layers.remove(pos);
        self.layers.insert(new_index, layer);
        self.changed = true;
        true
    }

    /// Makes `id` the active layer. Returns false when the id is unknown or
    /// already active.
    pub fn set_active(&mut self, id: LayerId) -> bool {
        if self.active == id || !self.layers.iter().any(|l| l.id == id) {
            return false;
        }
        self.active = id;
        self.changed = true;
        true
    }

    pub fn active_layer_id(&self) -> LayerId {
        self.active
    }

    pub fn active_layer(&self) -> &Layer {
        self.layers
            .iter()
            .find(|l| l.id == self.active)
            .expect("active layer id must reference an existing layer")
    }

    pub fn active_layer_mut(&mut self) -> &mut Layer {
        self.layers
            .iter_mut()
            .find(|l| l.id == self.active)
            .expect("active layer id must reference an existing layer")
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Bottom-to-top iteration over the layers.
    pub fn iter(&self) -> impl Iterator<Item = &Layer> {
        self.layers.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Layer> {
        self.layers.iter_mut()
    }

    pub fn set_name(&mut self, id: LayerId, name: &str) -> bool {
        let layer = match self.layers.iter_mut().find(|l| l.id == id) {
            Some(l) => l,
            None => return false,
        };
        if layer.name == name {
            return false;
        }
        layer.name = name.to_string();
        self.changed = true;
        true
    }

    pub fn set_visible(&mut self, id: LayerId, visible: bool) -> bool {
        let layer = match self.layers.iter_mut().find(|l| l.id == id) {
            Some(l) => l,
            None => return false,
        };
        if layer.visible == visible {
            return false;
        }
        layer.visible = visible;
        self.changed = true;
        true
    }

    /// Sets the layer opacity, clamped to `0..=1`.
    pub fn set_opacity(&mut self, id: LayerId, opacity: f32) -> bool {
        let layer = match self.layers.iter_mut().find(|l| l.id == id) {
            Some(l) => l,
            None => return false,
        };
        let clamped = if opacity.is_nan() {
            0.0
        } else {
            opacity.clamp(0.0, 1.0)
        };
        if layer.opacity == clamped {
            return false;
        }
        layer.opacity = clamped;
        self.changed = true;
        true
    }

    pub fn set_blend(&mut self, id: LayerId, blend: BlendMode) -> bool {
        let layer = match self.layers.iter_mut().find(|l| l.id == id) {
            Some(l) => l,
            None => return false,
        };
        if layer.blend == blend {
            return false;
        }
        layer.blend = blend;
        self.changed = true;
        true
    }

    pub fn changed(&self) -> bool {
        self.changed
    }

    pub fn clear_changed(&mut self) {
        self.changed = false;
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn canvas_size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Composites all visible layers (bottom-to-top) over the full canvas.
    pub fn composite_layers(&self) -> PixelBuffer {
        let full = Rect2i::new(
            0,
            0,
            self.width.min(i32::MAX as usize) as i32,
            self.height.min(i32::MAX as usize) as i32,
        );
        self.composite_layers_region(full)
            .unwrap_or_else(|| PixelBuffer::new(self.width, self.height))
    }

    /// Composites all visible layers (bottom-to-top) over `rect`, clipped to
    /// the canvas. Returns `None` when `rect` is empty or does not intersect
    /// the canvas; the result is `clipped_w * clipped_h` pixels.
    ///
    /// The result equals the full composite clipped to `rect`. Flat stacks
    /// (no groups) take a fast path byte-identical to the pre-group
    /// implementation; stacks with groups composite recursively.
    pub fn composite_layers_region(&self, rect: Rect2i) -> Option<PixelBuffer> {
        if rect.is_empty() {
            return None;
        }
        let canvas = Rect2i::new(
            0,
            0,
            self.width.min(i32::MAX as usize) as i32,
            self.height.min(i32::MAX as usize) as i32,
        );
        let clipped = rect.clamp_to(canvas);
        if clipped.is_empty() {
            return None;
        }
        Some(composite::composite_stack_region(self, clipped))
    }
}

/// Pure straight-alpha compositing namespace (ARCHITECTURE.md §5).
///
/// The two entry points are associated functions on this unit struct:
/// [`Compositing::composite`] (full canvas) and
/// [`Compositing::composite_region`] (arbitrary canvas-space rect).
pub struct Compositing;

impl Compositing {
    /// Composites `layers` (bottom-to-top) over the full canvas.
    ///
    /// Equivalent to [`Compositing::composite_region`] with the full-canvas
    /// rect. Returns `None` when the canvas is zero-sized.
    pub fn composite(
        canvas_width: usize,
        canvas_height: usize,
        layers: &[&PixelBuffer],
    ) -> Option<Vec<u8>> {
        let full = Rect2i::new(0, 0, canvas_width as i32, canvas_height as i32);
        Self::composite_region(full, canvas_width, canvas_height, layers)
    }

    /// Composites `layers` (bottom-to-top) over the canvas, clipped to `rect`.
    ///
    /// `rect` is in canvas space. The result is `clipped_w * clipped_h * 4`
    /// RGBA8 bytes, where `clipped` is `rect` intersected with the canvas.
    /// Returns `None` when `rect` is empty or does not intersect the canvas.
    pub fn composite_region(
        rect: Rect2i,
        canvas_width: usize,
        canvas_height: usize,
        layers: &[&PixelBuffer],
    ) -> Option<Vec<u8>> {
        if rect.is_empty() {
            return None;
        }
        let canvas = Rect2i::new(0, 0, canvas_width as i32, canvas_height as i32);
        let clipped = rect.clamp_to(canvas);
        if clipped.is_empty() {
            return None;
        }
        let out_w = clipped.w as usize;
        let out_h = clipped.h as usize;
        let mut out = vec![0u8; out_w * out_h * 4];
        for row in 0..out_h {
            let py = clipped.y + row as i32;
            for col in 0..out_w {
                let px = clipped.x + col as i32;
                let mut dst = Color::TRANSPARENT;
                for layer in layers {
                    if let Some(src) = layer.get_pixel(px as usize, py as usize) {
                        dst = over(src, dst);
                    }
                }
                let i = (row * out_w + col) * 4;
                out[i] = dst.r;
                out[i + 1] = dst.g;
                out[i + 2] = dst.b;
                out[i + 3] = dst.a;
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: usize, height: usize, color: Color) -> PixelBuffer {
        let mut buffer = PixelBuffer::new(width, height);
        buffer.fill(color);
        buffer
    }

    #[test]
    fn single_opaque_layer_full_canvas_passthrough() {
        let mut layer = PixelBuffer::new(2, 2);
        layer.set_pixel(0, 0, Color::rgba(10, 20, 30, 255));
        layer.set_pixel(1, 0, Color::rgba(40, 50, 60, 255));
        layer.set_pixel(0, 1, Color::rgba(70, 80, 90, 255));
        layer.set_pixel(1, 1, Color::rgba(100, 110, 120, 255));
        let out = Compositing::composite(2, 2, &[&layer]).unwrap();
        assert_eq!(out, layer.as_bytes());
    }

    #[test]
    fn opaque_top_layer_wins_over_opaque_bottom() {
        let bottom = solid(1, 1, Color::rgb(0, 0, 255));
        let top = solid(1, 1, Color::rgb(255, 0, 0));
        let out = Compositing::composite(1, 1, &[&bottom, &top]).unwrap();
        assert_eq!(out, vec![255, 0, 0, 255]);
    }

    #[test]
    fn half_alpha_red_over_opaque_blue_exact_math() {
        // src (255,0,0,128) over dst (0,0,255,255):
        //   out.a = 128/255 + 1*(1 - 128/255) = 1        -> 255
        //   out.r = (1*128/255)/1 = 128/255              -> 128
        //   out.b = (1*(1 - 128/255))/1 = 127/255        -> 127
        let bottom = solid(1, 1, Color::rgb(0, 0, 255));
        let top = solid(1, 1, Color::rgba(255, 0, 0, 128));
        let out = Compositing::composite(1, 1, &[&bottom, &top]).unwrap();
        assert_eq!(out, vec![128, 0, 127, 255]);
    }

    #[test]
    fn half_alpha_red_over_half_alpha_blue_exact_math() {
        // src (255,0,0,128) over dst (0,0,255,128):
        //   out.a = 128/255 + 128/255*(127/255) = 48896/65025 ≈ 0.751957 -> 192
        //   out.r = (128/255)/(48896/65025) ≈ 0.667539  -> 170
        //   out.b = (16256/65025)/(48896/65025) ≈ 0.332461 -> 85
        let bottom = solid(1, 1, Color::rgba(0, 0, 255, 128));
        let top = solid(1, 1, Color::rgba(255, 0, 0, 128));
        let out = Compositing::composite(1, 1, &[&bottom, &top]).unwrap();
        assert_eq!(out, vec![170, 0, 85, 192]);
    }

    #[test]
    fn three_layer_order_matters() {
        let red = solid(1, 1, Color::rgba(255, 0, 0, 128));
        let green = solid(1, 1, Color::rgba(0, 255, 0, 128));
        let blue = solid(1, 1, Color::rgba(0, 0, 255, 128));
        let bottom_to_top = Compositing::composite(1, 1, &[&red, &green, &blue]).unwrap();
        let reversed = Compositing::composite(1, 1, &[&blue, &green, &red]).unwrap();
        assert_eq!(bottom_to_top, vec![36, 73, 146, 224]);
        assert_eq!(reversed, vec![146, 73, 36, 224]);
    }

    #[test]
    fn fully_transparent_top_layer_leaves_bottom_unchanged() {
        let bottom = solid(2, 2, Color::rgba(12, 34, 56, 200));
        let top = solid(2, 2, Color::TRANSPARENT);
        let out = Compositing::composite(2, 2, &[&bottom, &top]).unwrap();
        assert_eq!(out, bottom.as_bytes());
    }

    #[test]
    fn region_subset_returns_only_rect_bytes() {
        let mut layer = PixelBuffer::new(4, 4);
        for y in 0..4 {
            for x in 0..4 {
                layer.set_pixel(x, y, Color::rgba(x as u8 * 10, y as u8 * 10, 0, 255));
            }
        }
        let out = Compositing::composite_region(Rect2i::new(0, 0, 2, 2), 4, 4, &[&layer]).unwrap();
        assert_eq!(out.len(), 2 * 2 * 4);
        assert_eq!(&out[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out[4..8], &[10, 0, 0, 255]);
        assert_eq!(&out[8..12], &[0, 10, 0, 255]);
        assert_eq!(&out[12..16], &[10, 10, 0, 255]);
    }

    #[test]
    fn region_offset_reads_correct_canvas_position() {
        let mut layer = PixelBuffer::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                layer.set_pixel(x, y, Color::rgba(x as u8, y as u8, 0, 255));
            }
        }
        let out = Compositing::composite_region(Rect2i::new(1, 1, 1, 1), 3, 3, &[&layer]).unwrap();
        assert_eq!(out, vec![1, 1, 0, 255]);
    }

    #[test]
    fn empty_rect_returns_none() {
        let layer = solid(2, 2, Color::WHITE);
        assert_eq!(
            Compositing::composite_region(Rect2i::ZERO, 2, 2, &[&layer]),
            None
        );
        assert_eq!(
            Compositing::composite_region(Rect2i::new(1, 1, 0, 5), 2, 2, &[&layer]),
            None
        );
        assert_eq!(
            Compositing::composite_region(Rect2i::new(1, 1, -3, 5), 2, 2, &[&layer]),
            None
        );
    }

    #[test]
    fn rect_fully_outside_canvas_returns_none() {
        let layer = solid(2, 2, Color::WHITE);
        assert_eq!(
            Compositing::composite_region(Rect2i::new(5, 5, 2, 2), 2, 2, &[&layer]),
            None
        );
        assert_eq!(
            Compositing::composite_region(Rect2i::new(-3, -3, 2, 2), 2, 2, &[&layer]),
            None
        );
        assert_eq!(
            Compositing::composite_region(Rect2i::new(2, 0, 2, 2), 2, 2, &[&layer]),
            None
        );
    }

    #[test]
    fn partially_out_of_bounds_rect_is_clipped() {
        let mut layer = PixelBuffer::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                layer.set_pixel(x, y, Color::rgba(x as u8, y as u8, 0, 255));
            }
        }
        let out = Compositing::composite_region(Rect2i::new(1, 1, 4, 4), 3, 3, &[&layer]).unwrap();
        assert_eq!(out.len(), 2 * 2 * 4);
        assert_eq!(&out[0..4], &[1, 1, 0, 255]);
        assert_eq!(&out[4..8], &[2, 1, 0, 255]);
        assert_eq!(&out[8..12], &[1, 2, 0, 255]);
        assert_eq!(&out[12..16], &[2, 2, 0, 255]);
    }

    #[test]
    fn composite_matches_full_canvas_region() {
        let bottom = solid(3, 2, Color::rgba(0, 0, 255, 128));
        let top = solid(3, 2, Color::rgba(255, 0, 0, 200));
        let full = Compositing::composite(3, 2, &[&bottom, &top]).unwrap();
        let region =
            Compositing::composite_region(Rect2i::new(0, 0, 3, 2), 3, 2, &[&bottom, &top]).unwrap();
        assert_eq!(full, region);
    }

    #[test]
    fn smaller_layer_is_padded_with_transparency() {
        let big = solid(3, 3, Color::rgb(0, 255, 0));
        let small = solid(1, 1, Color::rgb(255, 0, 0));
        let out = Compositing::composite(3, 3, &[&big, &small]).unwrap();
        assert_eq!(&out[0..4], &[255, 0, 0, 255]); // (0,0): red over green
        assert_eq!(&out[4..8], &[0, 255, 0, 255]); // (1,0): small layer transparent here
        assert_eq!(&out[32..36], &[0, 255, 0, 255]); // (2,2): green
    }

    #[test]
    fn zero_layers_produces_transparent_canvas() {
        let out = Compositing::composite(2, 2, &[]).unwrap();
        assert_eq!(out, vec![0; 2 * 2 * 4]);
        let region = Compositing::composite_region(Rect2i::new(1, 1, 1, 1), 2, 2, &[]).unwrap();
        assert_eq!(region, vec![0; 4]);
    }

    #[test]
    fn zero_sized_canvas_returns_none() {
        assert_eq!(Compositing::composite(0, 0, &[]), None);
        assert_eq!(Compositing::composite(0, 5, &[]), None);
        assert_eq!(Compositing::composite(5, 0, &[]), None);
    }

    #[test]
    fn transparent_bottom_layer_under_opaque_top() {
        let bottom = solid(1, 1, Color::TRANSPARENT);
        let top = solid(1, 1, Color::rgb(7, 8, 9));
        let out = Compositing::composite(1, 1, &[&bottom, &top]).unwrap();
        assert_eq!(out, vec![7, 8, 9, 255]);
    }

    #[test]
    fn new_creates_default_active_layer() {
        let stack = LayerStack::new(4, 3);
        assert_eq!(stack.len(), 1);
        assert!(!stack.is_empty());
        assert_eq!(stack.canvas_size(), (4, 3));
        assert_eq!(stack.width(), 4);
        assert_eq!(stack.height(), 3);
        assert!(!stack.changed());
        let layer = stack.active_layer();
        assert_eq!(layer.name, "Layer 1");
        assert!(layer.visible);
        assert_eq!(layer.opacity, 1.0);
        assert_eq!(layer.blend, BlendMode::Normal);
        assert_eq!(layer.buffer.width(), 4);
        assert_eq!(layer.buffer.height(), 3);
        assert_eq!(stack.active_layer_id(), layer.id);
        assert_eq!(stack.layer(layer.id).unwrap().id, layer.id);
    }

    #[test]
    fn add_layer_returns_monotonic_unique_ids() {
        let mut stack = LayerStack::new(2, 2);
        let a = stack.add_layer("A");
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert_eq!(stack.len(), 4);
        assert_eq!(stack.layer(a).unwrap().name, "A");
        assert_eq!(stack.layer(b).unwrap().name, "B");
        assert_eq!(stack.layer(c).unwrap().name, "C");
        let l = stack.layer(c).unwrap();
        assert!(l.visible);
        assert_eq!(l.opacity, 1.0);
        assert_eq!(l.blend, BlendMode::Normal);
        assert_eq!(l.buffer.width(), 2);
        assert_eq!(l.buffer.height(), 2);
    }

    #[test]
    fn add_layer_keeps_active_layer_unchanged() {
        let mut stack = LayerStack::new(2, 2);
        let initial = stack.active_layer_id();
        let id = stack.add_layer("A");
        assert_ne!(id, initial);
        assert_eq!(stack.active_layer_id(), initial);
    }

    #[test]
    fn single_normal_opaque_layer_matches_compositing_byte_identical() {
        let mut stack = LayerStack::new(3, 2);
        let id = stack.active_layer_id();
        {
            let layer = stack.layer_mut(id).unwrap();
            layer.buffer.set_pixel(0, 0, Color::rgba(10, 20, 30, 255));
            layer.buffer.set_pixel(1, 0, Color::rgba(40, 50, 60, 128));
            layer.buffer.set_pixel(2, 1, Color::rgba(70, 80, 90, 200));
        }
        let expected = Compositing::composite(3, 2, &[&stack.active_layer().buffer]).unwrap();
        assert_eq!(stack.composite_layers().as_bytes(), expected);
    }

    #[test]
    fn single_normal_opaque_layer_region_matches_compositing_byte_identical() {
        let mut stack = LayerStack::new(4, 3);
        let id = stack.active_layer_id();
        {
            let layer = stack.layer_mut(id).unwrap();
            layer.buffer.set_pixel(0, 0, Color::rgba(10, 20, 30, 255));
            layer.buffer.set_pixel(2, 1, Color::rgba(40, 50, 60, 128));
            layer.buffer.set_pixel(3, 2, Color::rgba(70, 80, 90, 200));
        }
        let rect = Rect2i::new(1, 0, 3, 3);
        let expected =
            Compositing::composite_region(rect, 4, 3, &[&stack.active_layer().buffer]).unwrap();
        let out = stack.composite_layers_region(rect).unwrap();
        assert_eq!(out.as_bytes(), expected);
    }

    #[test]
    fn full_region_matches_full_composite_for_all_layer_modes_and_visibility() {
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Add,
        ];
        for mode in modes {
            let mut stack = LayerStack::new(4, 3);
            let bottom = stack.active_layer_id();
            stack
                .layer_mut(bottom)
                .unwrap()
                .buffer
                .fill(Color::rgba(20, 40, 80, 180));
            let top = stack.add_layer("top");
            {
                let top_layer = stack.layer_mut(top).unwrap();
                top_layer.buffer.fill(Color::rgba(200, 100, 30, 128));
                top_layer.opacity = 0.5;
                top_layer.blend = mode;
            }
            let full = stack.composite_layers();
            let region = stack
                .composite_layers_region(Rect2i::new(0, 0, 4, 3))
                .unwrap();
            assert_eq!(full.as_bytes(), region.as_bytes(), "mode {mode:?}");

            stack.layer_mut(top).unwrap().visible = false;
            let hidden_full = stack.composite_layers();
            let hidden_region = stack
                .composite_layers_region(Rect2i::new(0, 0, 4, 3))
                .unwrap();
            assert_eq!(
                hidden_full.as_bytes(),
                hidden_region.as_bytes(),
                "hidden {mode:?}"
            );
        }
    }

    #[test]
    fn normal_blend_matches_compositing_for_multiple_layers() {
        let mut stack = LayerStack::new(2, 2);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgba(0, 0, 255, 128));
        let top = stack.add_layer("top");
        stack
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgba(255, 0, 0, 200));
        let expected = Compositing::composite(
            2,
            2,
            &[
                &stack.layer(bottom).unwrap().buffer,
                &stack.layer(top).unwrap().buffer,
            ],
        )
        .unwrap();
        assert_eq!(stack.composite_layers().as_bytes(), expected);
    }

    #[test]
    fn hidden_layer_is_skipped_in_composite() {
        let mut stack = LayerStack::new(1, 1);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 255));
        let top = stack.add_layer("top");
        stack
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        assert_eq!(stack.composite_layers().as_bytes(), &[255, 0, 0, 255]);
        assert!(stack.set_visible(top, false));
        assert_eq!(stack.composite_layers().as_bytes(), &[0, 0, 255, 255]);
        assert!(stack.set_visible(top, true));
        assert_eq!(stack.composite_layers().as_bytes(), &[255, 0, 0, 255]);
    }

    #[test]
    fn opacity_is_clamped_to_unit_range() {
        let mut stack = LayerStack::new(1, 1);
        let id = stack.active_layer_id();
        stack.layer_mut(id).unwrap().opacity = 0.5;
        assert!(stack.set_opacity(id, 2.0));
        assert_eq!(stack.layer(id).unwrap().opacity, 1.0);
        assert!(stack.set_opacity(id, -1.0));
        assert_eq!(stack.layer(id).unwrap().opacity, 0.0);
        assert!(stack.set_opacity(id, 0.5));
        assert_eq!(stack.layer(id).unwrap().opacity, 0.5);
        assert!(stack.set_opacity(id, f32::NAN));
        assert_eq!(stack.layer(id).unwrap().opacity, 0.0);
    }

    #[test]
    fn zero_opacity_layer_composites_as_transparent() {
        let mut stack = LayerStack::new(1, 1);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 255));
        let top = stack.add_layer("top");
        stack
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        assert!(stack.set_opacity(top, 0.0));
        assert_eq!(stack.composite_layers().as_bytes(), &[0, 0, 255, 255]);
    }

    #[test]
    fn half_opacity_premultiplies_source_alpha() {
        let mut stack = LayerStack::new(1, 1);
        let id = stack.active_layer_id();
        stack
            .layer_mut(id)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        assert!(stack.set_opacity(id, 0.5));
        // 255 * 0.5 = 127.5 -> round -> 128; over transparent -> (255,0,0,128)
        assert_eq!(stack.composite_layers().as_bytes(), &[255, 0, 0, 128]);
    }

    fn two_layer_stack() -> (LayerStack, LayerId, LayerId) {
        let mut stack = LayerStack::new(1, 1);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(100, 150, 200));
        let top = stack.add_layer("top");
        stack
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgb(50, 100, 150));
        (stack, bottom, top)
    }

    #[test]
    fn normal_blend_known_bytes() {
        let (stack, _bottom, _top) = two_layer_stack();
        assert_eq!(stack.composite_layers().as_bytes(), &[50, 100, 150, 255]);
    }

    #[test]
    fn multiply_blend_known_bytes() {
        let (mut stack, _bottom, top) = two_layer_stack();
        assert!(stack.set_blend(top, BlendMode::Multiply));
        // 50*100/255=20, 100*150/255=59, 150*200/255=118
        assert_eq!(stack.composite_layers().as_bytes(), &[20, 59, 118, 255]);
    }

    #[test]
    fn screen_blend_known_bytes() {
        let (mut stack, _bottom, top) = two_layer_stack();
        assert!(stack.set_blend(top, BlendMode::Screen));
        // 255-(205*155)/255=130, 255-(155*105)/255=191, 255-(105*55)/255=232
        assert_eq!(stack.composite_layers().as_bytes(), &[130, 191, 232, 255]);
    }

    #[test]
    fn add_blend_known_bytes() {
        let (mut stack, _bottom, top) = two_layer_stack();
        assert!(stack.set_blend(top, BlendMode::Add));
        // min(255,50+100)=150, min(255,100+150)=250, min(255,150+200)=255
        assert_eq!(stack.composite_layers().as_bytes(), &[150, 250, 255, 255]);
    }

    #[test]
    fn multiply_blend_with_partial_alpha() {
        let mut stack = LayerStack::new(1, 1);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(100, 150, 200));
        let top = stack.add_layer("top");
        stack
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgba(50, 100, 150, 128));
        assert!(stack.set_blend(top, BlendMode::Multiply));
        // blended (20,59,118,128) over opaque (100,150,200,255)
        assert_eq!(stack.composite_layers().as_bytes(), &[60, 104, 159, 255]);
    }

    #[test]
    fn multiply_over_transparent_canvas_falls_back_to_over() {
        let mut stack = LayerStack::new(1, 1);
        let id = stack.active_layer_id();
        stack
            .layer_mut(id)
            .unwrap()
            .buffer
            .fill(Color::rgb(50, 100, 150));
        assert!(stack.set_blend(id, BlendMode::Multiply));
        assert_eq!(stack.composite_layers().as_bytes(), &[50, 100, 150, 255]);
    }

    #[test]
    fn multiply_over_transparent_layer_falls_back_to_over() {
        let mut stack = LayerStack::new(1, 1);
        let _bottom = stack.active_layer_id(); // stays transparent
        let top = stack.add_layer("top");
        stack
            .layer_mut(top)
            .unwrap()
            .buffer
            .fill(Color::rgb(50, 100, 150));
        assert!(stack.set_blend(top, BlendMode::Multiply));
        assert_eq!(stack.composite_layers().as_bytes(), &[50, 100, 150, 255]);
    }

    #[test]
    fn layer_smaller_than_canvas_pads_with_transparency() {
        let mut stack = LayerStack::new(3, 3);
        let id = stack.active_layer_id();
        stack.layer_mut(id).unwrap().buffer = PixelBuffer::new(1, 1);
        stack
            .layer_mut(id)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        let out = stack.composite_layers();
        assert_eq!(out.get_pixel(0, 0), Some(Color::rgb(255, 0, 0)));
        assert_eq!(out.get_pixel(1, 0), Some(Color::TRANSPARENT));
        assert_eq!(out.get_pixel(2, 2), Some(Color::TRANSPARENT));
    }

    #[test]
    fn checked_stack_constructor_rejects_overflow_dimensions() {
        assert!(LayerStack::try_new(usize::MAX, 2).is_none());
    }

    #[test]
    fn reorder_moves_layer_to_new_index() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        assert!(stack.reorder(a, 2));
        let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![b, c, a]);
        assert!(stack.reorder(c, 0));
        let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![c, b, a]);
    }

    #[test]
    fn reorder_rejects_unknown_id_and_bad_index() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        stack.add_layer("B");
        assert!(!stack.reorder(LayerId::new(999), 0));
        assert!(!stack.reorder(a, 5));
        assert!(!stack.reorder(a, 0)); // no-op
        assert_eq!(stack.len(), 2);
    }

    #[test]
    fn insert_layer_restores_layer_at_index_with_identity() {
        let mut stack = LayerStack::new(2, 2);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        let removed = stack.remove_layer(b).unwrap();
        assert_eq!(stack.len(), 2);

        assert!(stack.insert_layer(removed.clone(), 1));
        let ids: Vec<LayerId> = stack.iter().map(|l| l.id).collect();
        assert_eq!(ids, vec![a, b, c]);
        assert_eq!(stack.layer(b).unwrap().name, "B");
        assert_eq!(stack.layer(b).unwrap().buffer.width(), 2);
    }

    #[test]
    fn insert_layer_rejects_duplicate_id_and_bad_index() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        let dup = stack.layer(a).unwrap().clone();
        assert!(!stack.insert_layer(dup, 0));
        assert!(!stack.insert_layer(stack.layer(b).unwrap().clone(), 5));
        assert_eq!(stack.len(), 2);
    }

    #[test]
    fn insert_layer_bumps_next_id_past_inserted_id() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        let removed = stack.remove_layer(b).unwrap();
        assert!(stack.insert_layer(removed, 1));
        let c = stack.add_layer("C");
        assert_ne!(c, a);
        assert_ne!(c, b);
        assert_eq!(stack.len(), 3);
    }

    #[test]
    fn remove_layer_returns_layer_and_shrinks_stack() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        let removed = stack.remove_layer(b).unwrap();
        assert_eq!(removed.id, b);
        assert_eq!(removed.name, "B");
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.layer(b), None);
        assert_eq!(stack.layer(a), Some(stack.active_layer()));
    }

    #[test]
    fn remove_last_layer_is_rejected() {
        let mut stack = LayerStack::new(1, 1);
        let id = stack.active_layer_id();
        assert_eq!(stack.remove_layer(id), None);
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn remove_unknown_layer_returns_none() {
        let mut stack = LayerStack::new(1, 1);
        stack.add_layer("B");
        assert_eq!(stack.remove_layer(LayerId::new(999)), None);
        assert_eq!(stack.len(), 2);
    }

    #[test]
    fn removing_active_layer_reselects_valid_one() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        assert!(stack.set_active(b));
        let removed = stack.remove_layer(b).unwrap();
        assert_eq!(removed.id, b);
        assert_ne!(stack.active_layer_id(), b);
        assert!(stack.layer(stack.active_layer_id()).is_some());
        assert_eq!(stack.len(), 2);
        assert!(stack.active_layer_id() == a || stack.active_layer_id() == c);
    }

    #[test]
    fn removing_top_active_layer_reselects_new_top() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        let c = stack.add_layer("C");
        assert!(stack.set_active(c));
        stack.remove_layer(c).unwrap();
        assert_eq!(stack.active_layer_id(), b);
        assert_ne!(stack.active_layer_id(), a);
    }

    #[test]
    fn removing_non_active_layer_keeps_active() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        assert!(stack.set_active(b));
        stack.remove_layer(a).unwrap();
        assert_eq!(stack.active_layer_id(), b);
    }

    #[test]
    fn set_active_switches_active_layer() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        assert!(stack.set_active(b));
        assert_eq!(stack.active_layer_id(), b);
        assert_eq!(stack.active_layer().name, "B");
        assert!(!stack.set_active(LayerId::new(999)));
        assert_eq!(stack.active_layer_id(), b);
        assert!(!stack.set_active(b)); // no-op
        assert_eq!(stack.active_layer_id(), b);
        assert!(stack.set_active(a));
        assert_eq!(stack.active_layer_id(), a);
    }

    #[test]
    fn setters_update_layer_attributes() {
        let mut stack = LayerStack::new(1, 1);
        let id = stack.active_layer_id();
        assert!(stack.set_name(id, "Renamed"));
        assert_eq!(stack.layer(id).unwrap().name, "Renamed");
        assert!(stack.set_visible(id, false));
        assert!(!stack.layer(id).unwrap().visible);
        assert!(stack.set_visible(id, true));
        assert!(stack.layer(id).unwrap().visible);
        assert!(stack.set_blend(id, BlendMode::Screen));
        assert_eq!(stack.layer(id).unwrap().blend, BlendMode::Screen);
        assert!(!stack.set_name(LayerId::new(999), "x"));
        assert!(!stack.set_visible(LayerId::new(999), true));
        assert!(!stack.set_opacity(LayerId::new(999), 0.5));
        assert!(!stack.set_blend(LayerId::new(999), BlendMode::Add));
    }

    #[test]
    fn noop_setters_do_not_mark_changed() {
        let mut stack = LayerStack::new(1, 1);
        let id = stack.active_layer_id();
        stack.clear_changed();
        assert!(!stack.set_name(id, "Layer 1"));
        assert!(!stack.set_visible(id, true));
        assert!(!stack.set_opacity(id, 1.0));
        assert!(!stack.set_blend(id, BlendMode::Normal));
        assert!(!stack.set_active(id));
        assert!(!stack.changed());
    }

    #[test]
    fn mutations_set_changed_flag() {
        let mut stack = LayerStack::new(1, 1);
        assert!(!stack.changed());
        let a = stack.active_layer_id();
        let b = stack.add_layer("B");
        assert!(stack.changed());
        stack.clear_changed();
        assert!(!stack.changed());
        assert!(stack.set_active(b));
        assert!(stack.changed());
        stack.clear_changed();
        assert!(stack.reorder(b, 0));
        assert!(stack.changed());
        stack.clear_changed();
        assert!(stack.set_opacity(b, 0.5));
        assert!(stack.changed());
        stack.clear_changed();
        assert!(stack.set_name(b, "B2"));
        assert!(stack.changed());
        stack.clear_changed();
        assert!(stack.set_visible(b, false));
        assert!(stack.changed());
        stack.clear_changed();
        assert!(stack.set_blend(b, BlendMode::Add));
        assert!(stack.changed());
        stack.clear_changed();
        assert!(stack.remove_layer(b).is_some());
        assert!(stack.changed());
        stack.clear_changed();
        assert!(!stack.changed());
        assert_eq!(stack.remove_layer(a), None);
        assert!(!stack.changed());
        assert!(!stack.set_active(LayerId::new(999)));
        assert!(!stack.changed());
    }

    #[test]
    fn accessors_report_canvas_and_layers() {
        let mut stack = LayerStack::new(5, 7);
        assert_eq!(stack.width(), 5);
        assert_eq!(stack.height(), 7);
        assert_eq!(stack.canvas_size(), (5, 7));
        assert_eq!(stack.len(), 1);
        assert!(!stack.is_empty());
        let id = stack.active_layer_id();
        assert_eq!(stack.layer(id).unwrap().id, id);
        assert_eq!(stack.layer_mut(id).unwrap().id, id);
        assert_eq!(stack.layer(LayerId::new(999)), None);
        assert_eq!(stack.layer_mut(LayerId::new(999)), None);
        let names: Vec<&str> = stack.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["Layer 1"]);
    }

    #[test]
    fn active_layer_mut_edits_active_buffer() {
        let mut stack = LayerStack::new(1, 1);
        stack.active_layer_mut().buffer.fill(Color::rgb(1, 2, 3));
        assert_eq!(stack.composite_layers().as_bytes(), &[1, 2, 3, 255]);
    }

    #[test]
    fn composite_region_clips_and_offsets() {
        let mut stack = LayerStack::new(3, 3);
        let id = stack.active_layer_id();
        {
            let layer = stack.layer_mut(id).unwrap();
            for y in 0..3 {
                for x in 0..3 {
                    layer
                        .buffer
                        .set_pixel(x, y, Color::rgba(x as u8, y as u8, 0, 255));
                }
            }
        }
        let out = stack
            .composite_layers_region(Rect2i::new(1, 1, 2, 2))
            .unwrap();
        assert_eq!(out.width(), 2);
        assert_eq!(out.height(), 2);
        assert_eq!(out.get_pixel(0, 0), Some(Color::rgba(1, 1, 0, 255)));
        assert_eq!(out.get_pixel(1, 1), Some(Color::rgba(2, 2, 0, 255)));
    }

    #[test]
    fn composite_region_empty_or_outside_returns_none() {
        let stack = LayerStack::new(2, 2);
        assert_eq!(stack.composite_layers_region(Rect2i::ZERO), None);
        assert_eq!(stack.composite_layers_region(Rect2i::new(5, 5, 2, 2)), None);
        assert_eq!(stack.composite_layers_region(Rect2i::new(0, 0, 0, 2)), None);
    }

    #[test]
    fn composite_layers_zero_canvas_returns_empty_buffer() {
        let stack = LayerStack::new(0, 0);
        let out = stack.composite_layers();
        assert_eq!(out.width(), 0);
        assert_eq!(out.height(), 0);
    }
}
