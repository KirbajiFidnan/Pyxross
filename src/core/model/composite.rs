//! Recursive layer compositing.
//!
//! [`composite_over`] is the public per-pixel compositing primitive: it
//! applies a layer's opacity to its alpha and blends its color against the
//! accumulated backdrop, exactly the semantics of the original flat-stack
//! loop. The recursive group path composites each group's children into an
//! isolated scratch buffer first, then applies the group's own opacity and
//! blend mode once to the combined result.
//!
//! Flat stacks (no groups) take a fast path that reproduces the original
//! per-pixel loop verbatim, guaranteeing byte-identical output.

use crate::core::buffer::PixelBuffer;
use crate::core::color::Color;
use crate::core::math::Rect2i;

use super::{BlendMode, Layer, LayerStack};

/// Straight-alpha "over": `src` composited on top of `dst`.
pub(super) fn over(src: Color, dst: Color) -> Color {
    let sa = src.a as f32 / 255.0;
    let da = dst.a as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    let out_a_u8 = (out_a * 255.0).round() as u8;
    if out_a == 0.0 {
        return Color::TRANSPARENT;
    }
    let inv_sa = 1.0 - sa;
    let r = ((src.r as f32 / 255.0 * sa + dst.r as f32 / 255.0 * da * inv_sa) / out_a * 255.0)
        .round() as u8;
    let g = ((src.g as f32 / 255.0 * sa + dst.g as f32 / 255.0 * da * inv_sa) / out_a * 255.0)
        .round() as u8;
    let b = ((src.b as f32 / 255.0 * sa + dst.b as f32 / 255.0 * da * inv_sa) / out_a * 255.0)
        .round() as u8;
    Color::rgba(r, g, b, out_a_u8)
}

/// Scales `src`'s alpha by the layer opacity (straight-alpha: color is kept).
fn apply_opacity(src: Color, opacity: f32) -> Color {
    let a = (src.a as f32 * opacity).round() as u8;
    Color::rgba(src.r, src.g, src.b, a)
}

/// Applies `blend` to `src`'s color against the accumulated `dst` backdrop,
/// then composites via the straight-alpha [`over`]. When the backdrop is
/// fully transparent the blend is skipped and the plain `over` result is
/// used (a blend against nothing must not darken or tint the source).
fn blend_over(src: Color, dst: Color, blend: BlendMode) -> Color {
    if dst.a == 0 {
        return over(src, dst);
    }
    let blended = match blend {
        BlendMode::Normal => src,
        BlendMode::Multiply => Color::rgba(
            (src.r as f32 * dst.r as f32 / 255.0).round() as u8,
            (src.g as f32 * dst.g as f32 / 255.0).round() as u8,
            (src.b as f32 * dst.b as f32 / 255.0).round() as u8,
            src.a,
        ),
        BlendMode::Screen => Color::rgba(
            (255.0 - (255.0 - src.r as f32) * (255.0 - dst.r as f32) / 255.0).round() as u8,
            (255.0 - (255.0 - src.g as f32) * (255.0 - dst.g as f32) / 255.0).round() as u8,
            (255.0 - (255.0 - src.b as f32) * (255.0 - dst.b as f32) / 255.0).round() as u8,
            src.a,
        ),
        BlendMode::Add => Color::rgba(
            (src.r as u16 + dst.r as u16).min(255) as u8,
            (src.g as u16 + dst.g as u16).min(255) as u8,
            (src.b as u16 + dst.b as u16).min(255) as u8,
            src.a,
        ),
    };
    over(blended, dst)
}

/// Composites `src` over `dst` in place, applying `opacity` to `src`'s alpha
/// and `blend` to its color against the accumulated backdrop.
///
/// `src` and `dst` are aligned: `src` pixel `(x, y)` lands on `dst` pixel
/// `(x, y)`. Pixels of `src` outside its own bounds are transparent.
pub fn composite_over(dst: &mut PixelBuffer, src: &PixelBuffer, opacity: f32, blend: BlendMode) {
    composite_over_offset(dst, src, 0, 0, opacity, blend);
}

/// Like [`composite_over`], but reads `src` at canvas position
/// `(ox + x, oy + y)` for each `dst`-local pixel `(x, y)`. Used by the
/// region compositor so a region-sized destination reads full-canvas layers
/// at the correct offset.
fn composite_over_offset(
    dst: &mut PixelBuffer,
    src: &PixelBuffer,
    ox: i32,
    oy: i32,
    opacity: f32,
    blend: BlendMode,
) {
    for y in 0..dst.height() {
        for x in 0..dst.width() {
            let px = ox + x as i32;
            let py = oy + y as i32;
            let src_px = if px >= 0
                && py >= 0
                && (px as usize) < src.width()
                && (py as usize) < src.height()
            {
                src.get_pixel_unchecked(px as usize, py as usize)
            } else {
                Color::TRANSPARENT
            };
            if src_px.a == 0 {
                continue;
            }
            let src_px = apply_opacity(src_px, opacity);
            if src_px.a == 0 {
                continue;
            }
            let dst_px = dst.get_pixel_unchecked(x, y);
            dst.set_pixel_unchecked(x, y, blend_over(src_px, dst_px, blend));
        }
    }
}

/// Composites one node (layer or group) into `dst`.
///
/// Groups are isolated: their children are composited into a fresh scratch
/// buffer first, then the group's own opacity and blend mode are applied once
/// to the combined result. Hidden nodes (and their whole subtree) are skipped.
fn compose_node_region(dst: &mut PixelBuffer, stack: &LayerStack, node: &Layer, ox: i32, oy: i32) {
    if !node.visible {
        return;
    }
    if node.is_group {
        let mut scratch = PixelBuffer::new(dst.width(), dst.height());
        for child_id in stack.children_of(Some(node.id)) {
            let child = stack
                .layer(child_id)
                .expect("group child id must reference an existing layer");
            compose_node_region(&mut scratch, stack, child, ox, oy);
        }
        composite_over_offset(dst, &scratch, 0, 0, node.opacity, node.blend);
    } else {
        composite_over_offset(dst, &node.buffer, ox, oy, node.opacity, node.blend);
    }
}

/// Composites the whole stack over `clipped` (already clamped to the canvas).
///
/// Flat stacks (no groups) take the fast path: the original per-pixel loop,
/// byte-identical to the pre-group implementation. Stacks with groups take
/// the recursive group path.
pub(super) fn composite_stack_region(stack: &LayerStack, clipped: Rect2i) -> PixelBuffer {
    let out_w = clipped.w as usize;
    let out_h = clipped.h as usize;
    let mut out = PixelBuffer::new(out_w, out_h);
    if !stack.has_groups() {
        for layer in stack.iter() {
            if !layer.visible {
                continue;
            }
            composite_over_offset(
                &mut out,
                &layer.buffer,
                clipped.x,
                clipped.y,
                layer.opacity,
                layer.blend,
            );
        }
    } else {
        for root_id in stack.children_of(None) {
            let node = stack
                .layer(root_id)
                .expect("root id must reference an existing layer");
            compose_node_region(&mut out, stack, node, clipped.x, clipped.y);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::color::Color;
    use crate::core::model::LayerId;

    fn solid(width: usize, height: usize, color: Color) -> PixelBuffer {
        let mut buffer = PixelBuffer::new(width, height);
        buffer.fill(color);
        buffer
    }

    /// Builds `[bottom, G, A, B]` where `G` wraps opaque red `A` and opaque
    /// blue `B`, over an opaque green bottom.
    fn stack_with_group() -> (LayerStack, LayerId, LayerId, LayerId) {
        let mut stack = LayerStack::new(1, 1);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 255, 0));
        let a = stack.add_layer("A");
        stack
            .layer_mut(a)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        let b = stack.add_layer("B");
        stack
            .layer_mut(b)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 255));
        let group = stack.create_group_around(&[a, b], "G").unwrap();
        (stack, bottom, group, a)
    }

    #[test]
    fn group_composites_children_isolated() {
        let (mut stack, _bottom, group, _a) = stack_with_group();
        stack.set_opacity(group, 0.5);
        // Isolated: children combine to opaque blue, then the group's opacity
        // 0.5 scales that once: (0,0,255,128) over green -> (0,127,128,255).
        // Non-isolated (per-child opacity) would give (127,63,128,255).
        assert_eq!(stack.composite_layers().as_bytes(), &[0, 127, 128, 255]);
    }

    #[test]
    fn group_opacity_scales_children_result() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.add_layer("A");
        stack
            .layer_mut(a)
            .unwrap()
            .buffer
            .fill(Color::rgba(255, 0, 0, 128));
        let b = stack.add_layer("B");
        stack
            .layer_mut(b)
            .unwrap()
            .buffer
            .fill(Color::rgba(0, 0, 255, 128));
        let group = stack.create_group_around(&[a, b], "G").unwrap();
        stack.set_opacity(group, 0.5);
        // Children combine to (85,0,170,192); group opacity 0.5 scales the
        // combined alpha once: 192 * 0.5 = 96.
        assert_eq!(stack.composite_layers().as_bytes(), &[85, 0, 170, 96]);
    }

    #[test]
    fn group_blend_applies_once_to_group_result() {
        let mut stack = LayerStack::new(1, 1);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 0));
        let a = stack.add_layer("A");
        stack
            .layer_mut(a)
            .unwrap()
            .buffer
            .fill(Color::rgb(100, 0, 0));
        let b = stack.add_layer("B");
        stack
            .layer_mut(b)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 100, 0));
        let group = stack.create_group_around(&[a, b], "G").unwrap();
        stack.set_blend(group, BlendMode::Add);
        // Children combine to (0,100,0); the group's Add blend applies once
        // against black. Per-child Add would give (100,100,0).
        assert_eq!(stack.composite_layers().as_bytes(), &[0, 100, 0, 255]);
    }

    #[test]
    fn group_hidden_skips_subtree() {
        let (mut stack, _bottom, group, _a) = stack_with_group();
        assert_eq!(stack.composite_layers().as_bytes(), &[0, 0, 255, 255]);
        assert!(stack.set_visible(group, false));
        assert_eq!(stack.composite_layers().as_bytes(), &[0, 255, 0, 255]);
    }

    #[test]
    fn nested_group_opacity_multiplies() {
        let mut stack = LayerStack::new(1, 1);
        let a = stack.add_layer("A");
        stack
            .layer_mut(a)
            .unwrap()
            .buffer
            .fill(Color::rgb(255, 0, 0));
        let inner = stack.create_group_around(&[a], "Inner").unwrap();
        let outer = stack.create_group_around(&[inner], "Outer").unwrap();
        stack.set_opacity(inner, 0.5);
        stack.set_opacity(outer, 0.5);
        // 255 * 0.5 * 0.5 = 63.75 -> 64
        assert_eq!(stack.composite_layers().as_bytes(), &[255, 0, 0, 64]);
    }

    #[test]
    fn flat_composite_matches_reference_loop() {
        let mut stack = LayerStack::new(4, 3);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgba(20, 40, 80, 180));
        let top = stack.add_layer("top");
        {
            let layer = stack.layer_mut(top).unwrap();
            layer.buffer.fill(Color::rgba(200, 100, 30, 128));
            layer.opacity = 0.5;
            layer.blend = BlendMode::Multiply;
        }
        // Reference: the original per-pixel loop, reproduced verbatim.
        let mut expected = PixelBuffer::new(4, 3);
        for y in 0..3 {
            for x in 0..4 {
                let mut dst = Color::TRANSPARENT;
                for layer in stack.iter() {
                    if !layer.visible {
                        continue;
                    }
                    let src = if x < layer.buffer.width() && y < layer.buffer.height() {
                        layer.buffer.get_pixel_unchecked(x, y)
                    } else {
                        Color::TRANSPARENT
                    };
                    if src.a == 0 {
                        continue;
                    }
                    let src = apply_opacity(src, layer.opacity);
                    if src.a == 0 {
                        continue;
                    }
                    dst = blend_over(src, dst, layer.blend);
                }
                expected.set_pixel_unchecked(x, y, dst);
            }
        }
        assert_eq!(stack.composite_layers().as_bytes(), expected.as_bytes());
    }

    #[test]
    fn group_region_matches_full_composite_clipped() {
        let mut stack = LayerStack::new(3, 3);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgba(10, 20, 30, 200));
        let a = stack.add_layer("A");
        stack
            .layer_mut(a)
            .unwrap()
            .buffer
            .fill(Color::rgba(255, 0, 0, 128));
        let b = stack.add_layer("B");
        stack
            .layer_mut(b)
            .unwrap()
            .buffer
            .fill(Color::rgba(0, 0, 255, 128));
        let group = stack.create_group_around(&[a, b], "G").unwrap();
        stack.set_opacity(group, 0.5);
        let full = stack.composite_layers();
        let region = stack
            .composite_layers_region(Rect2i::new(1, 1, 2, 2))
            .unwrap();
        assert_eq!((region.width(), region.height()), (2, 2));
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(
                    region.get_pixel(x, y),
                    full.get_pixel(x + 1, y + 1),
                    "pixel ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn composite_over_applies_opacity_then_blend() {
        let mut dst = PixelBuffer::new(1, 1);
        dst.fill(Color::rgb(0, 0, 255));
        let src = solid(1, 1, Color::rgb(255, 0, 0));
        composite_over(&mut dst, &src, 0.5, BlendMode::Normal);
        // (255,0,0,128) over (0,0,255,255) -> (128,0,127,255)
        assert_eq!(dst.as_bytes(), &[128, 0, 127, 255]);
    }

    #[test]
    fn composite_over_skips_transparent_src() {
        let mut dst = PixelBuffer::new(1, 1);
        dst.fill(Color::rgb(1, 2, 3));
        let src = solid(1, 1, Color::TRANSPARENT);
        composite_over(&mut dst, &src, 1.0, BlendMode::Multiply);
        assert_eq!(dst.as_bytes(), &[1, 2, 3, 255]);
    }
}
