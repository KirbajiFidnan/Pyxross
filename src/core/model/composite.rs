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
use crate::core::tilemap::{TilePalette, TilePixelOverrides};

use super::{BlendMode, Layer, LayerStack, TilemapRenderCache};

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
///
/// **Decision (tilemap-on-group):** a group's OWN `tilemap` is deliberately
/// NOT composited here — a group is a container whose pixels come from its
/// children, and its own buffer is empty by construction. Writing a tilemap on
/// a group would therefore be invisible, so the tile placer REJECTS stamps on
/// group layers with a `last_error` message (see `tile_placer_cell` in the UI).
/// This keeps the model consistent: groups never render their own tilemap, and
/// the UI never creates one it cannot show.
///
/// `palette` supplies the tile pixels for any leaf layer with a tilemap.
fn compose_node_region(
    dst: &mut PixelBuffer,
    stack: &LayerStack,
    node: &Layer,
    ox: i32,
    oy: i32,
    palette: &TilePalette,
    overrides: Option<&TilePixelOverrides>,
) {
    if !node.visible {
        return;
    }
    if node.is_group {
        let mut scratch = PixelBuffer::new(dst.width(), dst.height());
        for child_id in stack.children_of(Some(node.id)) {
            let child = stack
                .layer(child_id)
                .expect("group child id must reference an existing layer");
            compose_node_region(&mut scratch, stack, child, ox, oy, palette, overrides);
        }
        composite_over_offset(dst, &scratch, 0, 0, node.opacity, node.blend);
    } else {
        composite_layer_over(dst, node, ox, oy, palette, overrides);
    }
}

/// Composites one LEAF layer over `dst` at canvas offset `(ox, oy)`.
///
/// A layer with a tilemap is rasterized (through its [`TilemapRenderCache`],
/// re-rasterizing only when the tilemap/palette epochs or the canvas size
/// changed) into a canvas-sized scratch. **Z-order (documented): the tilemap
/// raster composites OVER the layer's OWN pixel buffer — a placed tile is a
/// deliberate stamp that must always be visible, even over pixels the user
/// painted first.**
///
/// **Integrated tile model (documented): the layer buffer IS the edit surface
/// and tiled cells are BAKED — for every occupied cell, the buffer footprint
/// holds EXACTLY the oriented root-tile bytes (see [`crate::core::tile_edit`]).
/// A Pencil stroke on a tiled cell therefore writes THROUGH to the root tile
/// (un-oriented) and re-stamps every instance of that tile on every layer;
/// strokes on empty cells write ordinary pixels. At tiled cells the tilemap
/// raster over the baked buffer is byte-identical to the baked buffer itself,
/// so this function needs NO functional change.** The combined scratch is then
/// composited over `dst` with the layer's opacity/blend exactly once, mirroring
/// the pixel path. The pixel buffer is deliberately NOT cached — always
/// re-read, it is cheap. `locked` does NOT affect rendering (it only gates
/// editing). The no-tilemap path is byte-identical to the original.
fn composite_layer_over(
    dst: &mut PixelBuffer,
    layer: &Layer,
    ox: i32,
    oy: i32,
    palette: &TilePalette,
    overrides: Option<&TilePixelOverrides>,
) {
    if let Some(tm) = &layer.tilemap {
        if let Some(overrides) = overrides {
            let mut raster = PixelBuffer::new(layer.buffer.width(), layer.buffer.height());
            tm.rasterize_with_overrides(palette, overrides, &mut raster);
            let mut combined = layer.buffer.clone();
            composite_over_offset(&mut combined, &raster, 0, 0, 1.0, BlendMode::Normal);
            tm.apply_pixel_overrides(palette, overrides, &mut combined);
            composite_over_offset(dst, &combined, ox, oy, layer.opacity, layer.blend);
            return;
        }
        let mut cache = layer.tilemap_cache.borrow_mut();
        let canvas_w = layer.buffer.width();
        let canvas_h = layer.buffer.height();
        let stale = match cache.as_ref() {
            Some(entry) => {
                entry.tilemap_epoch != tm.change_epoch
                    || entry.palette_epoch != palette.change_epoch
                    || entry.canvas_w != canvas_w
                    || entry.canvas_h != canvas_h
            }
            None => true,
        };
        if stale {
            let mut buffer = PixelBuffer::new(canvas_w, canvas_h);
            tm.rasterize(palette, &mut buffer);
            *cache = Some(TilemapRenderCache {
                tilemap_epoch: tm.change_epoch,
                palette_epoch: palette.change_epoch,
                canvas_w,
                canvas_h,
                buffer,
            });
        }
        // Guaranteed populated (stale covers None); a defensive `match`
        // keeps this a no-panic hot path even under a logic error.
        let raster = match cache.as_ref() {
            Some(raster) => raster,
            None => {
                let mut buffer = PixelBuffer::new(canvas_w, canvas_h);
                tm.rasterize(palette, &mut buffer);
                *cache = Some(TilemapRenderCache {
                    tilemap_epoch: tm.change_epoch,
                    palette_epoch: palette.change_epoch,
                    canvas_w,
                    canvas_h,
                    buffer,
                });
                cache
                    .as_ref()
                    .expect("tilemap cache is populated immediately above")
            }
        };
        // Combine: the layer's PIXEL BUFFER first, then the tilemap raster
        // OVER it (placed tiles win). The layer's opacity/blend applies ONCE
        // to the combined result — exactly like the pixel path.
        let mut combined = layer.buffer.clone();
        composite_over_offset(&mut combined, &raster.buffer, 0, 0, 1.0, BlendMode::Normal);
        composite_over_offset(dst, &combined, ox, oy, layer.opacity, layer.blend);
    } else {
        // No tilemap: drop any stale cache and take the pixel path.
        if overrides.is_none() {
            layer.tilemap_cache.borrow_mut().take();
        }
        composite_over_offset(dst, &layer.buffer, ox, oy, layer.opacity, layer.blend);
    }
}

/// Composites the whole stack over `clipped` (already clamped to the canvas).
///
/// Flat stacks (no groups) take the fast path: the original per-pixel loop,
/// byte-identical to the pre-group implementation (with tilemap layers
/// rasterized through the cache). Stacks with groups take the recursive group
/// path.
pub(super) fn composite_stack_region(
    stack: &LayerStack,
    palette: &TilePalette,
    clipped: Rect2i,
) -> PixelBuffer {
    composite_stack_region_internal(stack, palette, clipped, None)
}

/// Uncached counterpart to [`composite_stack_region`] used for transient tile
/// pixel previews. Every leaf tilemap receives the same root overrides.
pub(super) fn composite_stack_region_with_tile_overrides(
    stack: &LayerStack,
    palette: &TilePalette,
    overrides: &TilePixelOverrides,
    clipped: Rect2i,
) -> PixelBuffer {
    composite_stack_region_internal(stack, palette, clipped, Some(overrides))
}

fn composite_stack_region_internal(
    stack: &LayerStack,
    palette: &TilePalette,
    clipped: Rect2i,
    overrides: Option<&TilePixelOverrides>,
) -> PixelBuffer {
    let out_w = clipped.w as usize;
    let out_h = clipped.h as usize;
    let mut out = PixelBuffer::new(out_w, out_h);
    if !stack.has_groups() {
        for layer in stack.iter() {
            if !layer.visible {
                continue;
            }
            composite_layer_over(&mut out, layer, clipped.x, clipped.y, palette, overrides);
        }
    } else {
        for root_id in stack.children_of(None) {
            let node = stack
                .layer(root_id)
                .expect("root id must reference an existing layer");
            compose_node_region(
                &mut out,
                stack,
                node,
                clipped.x,
                clipped.y,
                palette,
                overrides,
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::color::Color;
    use crate::core::model::LayerId;
    use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

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
        assert_eq!(
            stack.composite_layers(&TilePalette::new()).as_bytes(),
            &[0, 127, 128, 255]
        );
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
        assert_eq!(
            stack.composite_layers(&TilePalette::new()).as_bytes(),
            &[85, 0, 170, 96]
        );
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
        assert_eq!(
            stack.composite_layers(&TilePalette::new()).as_bytes(),
            &[0, 100, 0, 255]
        );
    }

    #[test]
    fn group_hidden_skips_subtree() {
        let (mut stack, _bottom, group, _a) = stack_with_group();
        assert_eq!(
            stack.composite_layers(&TilePalette::new()).as_bytes(),
            &[0, 0, 255, 255]
        );
        assert!(stack.set_visible(group, false));
        assert_eq!(
            stack.composite_layers(&TilePalette::new()).as_bytes(),
            &[0, 255, 0, 255]
        );
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
        assert_eq!(
            stack.composite_layers(&TilePalette::new()).as_bytes(),
            &[255, 0, 0, 64]
        );
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
        assert_eq!(
            stack.composite_layers(&TilePalette::new()).as_bytes(),
            expected.as_bytes()
        );
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
        let full = stack.composite_layers(&TilePalette::new());
        let region = stack
            .composite_layers_region(Rect2i::new(1, 1, 2, 2), &TilePalette::new())
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

    // -- tilemap composite path ----------------------------------------------

    fn solid_tile(id: u64, w: u16, h: u16, color: [u8; 4]) -> Tile {
        Tile {
            id: TileId(id),
            w,
            h,
            pixels: color.repeat(w as usize * h as usize),
        }
    }

    /// Adds a leaf layer with a tilemap whose cells are `(x, y, cell)`.
    fn tilemap_layer(
        stack: &mut LayerStack,
        name: &str,
        tile_size: u32,
        cells: &[(u32, u32, TileCell)],
    ) -> LayerId {
        let id = stack.add_layer(name);
        let mut tm = TileMap::new(tile_size, 8, 8);
        for &(x, y, ref cell) in cells {
            tm.set_cell((x, y), Some(*cell));
        }
        stack.layer_mut(id).unwrap().tilemap = Some(tm);
        id
    }

    #[test]
    fn tilemap_layer_composites_over_pixel_layer() {
        use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

        let mut stack = LayerStack::new(8, 8);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 255, 0));
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 2, 2, [255, 0, 0, 255]));
        tilemap_layer(&mut stack, "tiles", 4, &[(0, 0, TileCell::new(TileId(1)))]);

        let out = stack.composite_layers(&palette);
        // Cell (0,0) at (0,0): an opaque red 2x2 overwrites the green bottom.
        assert_eq!(out.get_pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(out.get_pixel(1, 1), Some(Color::rgba(255, 0, 0, 255)));
        // Outside the tile the green bottom shows through.
        assert_eq!(out.get_pixel(4, 4), Some(Color::rgb(0, 255, 0)));
    }

    #[test]
    fn tilemap_layer_opacity_blends_over_backdrop() {
        use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

        let mut stack = LayerStack::new(8, 8);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 0, 255));
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 2, 2, [255, 0, 0, 255]));
        let id = tilemap_layer(&mut stack, "tiles", 4, &[(0, 0, TileCell::new(TileId(1)))]);
        stack.set_opacity(id, 0.5);

        let out = stack.composite_layers(&palette);
        // (255,0,0,128) over (0,0,255,255) -> (128,0,127,255).
        assert_eq!(out.get_pixel(0, 0), Some(Color::rgba(128, 0, 127, 255)));
    }

    #[test]
    fn hidden_tilemap_layer_is_not_drawn_and_locked_layer_still_is() {
        use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

        let mut stack = LayerStack::new(8, 8);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 255, 0));
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 2, 2, [255, 0, 0, 255]));

        let hidden = tilemap_layer(&mut stack, "hidden", 4, &[(0, 0, TileCell::new(TileId(1)))]);
        stack.set_visible(hidden, false);
        let locked = tilemap_layer(&mut stack, "locked", 4, &[(1, 0, TileCell::new(TileId(1)))]);
        stack.layer_mut(locked).unwrap().locked = true;

        let out = stack.composite_layers(&palette);
        assert_eq!(
            out.get_pixel(0, 0),
            Some(Color::rgb(0, 255, 0)),
            "a hidden tilemap layer is not drawn"
        );
        assert_eq!(
            out.get_pixel(4, 0),
            Some(Color::rgba(255, 0, 0, 255)),
            "a locked tilemap layer is still rendered (locked only gates editing)"
        );
    }

    #[test]
    fn tilemap_cells_rotate_and_flip_composite() {
        use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

        let mut stack = LayerStack::new(8, 8);
        let mut palette = TilePalette::new();
        // 2x1 tile [red, green].
        palette.add(Tile {
            id: TileId(1),
            w: 2,
            h: 1,
            pixels: [255, 0, 0, 255, 0, 255, 0, 255].to_vec(),
        });
        // Cell (0,0) rotated 90° -> a 1x2 red-over-green column.
        let mut rotated = TileCell::new(TileId(1));
        rotated.set_rotation(1);
        // Cell (1,0) flipped X -> [green, red].
        let flipped = TileCell {
            tile_id: TileId(1),
            rotation: 0,
            flip_x: true,
            flip_y: false,
        };
        tilemap_layer(&mut stack, "tiles", 4, &[(0, 0, rotated), (1, 0, flipped)]);

        let out = stack.composite_layers(&palette);
        assert_eq!(out.get_pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(out.get_pixel(0, 1), Some(Color::rgba(0, 255, 0, 255)));
        assert_eq!(out.get_pixel(4, 0), Some(Color::rgba(0, 255, 0, 255)));
        assert_eq!(out.get_pixel(5, 0), Some(Color::rgba(255, 0, 0, 255)));
    }

    #[test]
    fn tilemap_cache_reused_until_epoch_changes() {
        use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

        let mut stack = LayerStack::new(8, 8);
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 2, 2, [255, 0, 0, 255]));
        let id = tilemap_layer(&mut stack, "tiles", 4, &[(0, 0, TileCell::new(TileId(1)))]);
        let first = stack.composite_layers(&palette);
        assert_eq!(first.get_pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));

        // Unchanged epochs: a second composite REUSES the cache buffer (its
        // backing allocation must not be replaced).
        let second = stack.composite_layers(&palette);
        assert_eq!(second.as_bytes(), first.as_bytes(), "deterministic");
        let pointer = {
            let cache = stack.layer(id).unwrap().tilemap_cache.borrow();
            cache.as_ref().unwrap().buffer.as_bytes().as_ptr()
        };
        let _ = stack.composite_layers(&palette);
        let pointer_after = {
            let cache = stack.layer(id).unwrap().tilemap_cache.borrow();
            cache.as_ref().unwrap().buffer.as_bytes().as_ptr()
        };
        assert_eq!(
            pointer, pointer_after,
            "an unchanged tilemap must NOT be re-rasterized (cache reused)"
        );

        // A cell edit bumps the tilemap epoch and re-rasterizes.
        stack
            .layer_mut(id)
            .unwrap()
            .tilemap
            .as_mut()
            .unwrap()
            .set_cell((1, 0), Some(TileCell::new(TileId(1))));
        let third = stack.composite_layers(&palette);
        assert_eq!(
            third.get_pixel(4, 0),
            Some(Color::rgba(255, 0, 0, 255)),
            "the new cell renders after the re-rasterize"
        );

        // A palette change (add) bumps the palette epoch and re-rasterizes
        // (the removed-tile path is exercised by clearing the palette).
        palette.tiles.clear();
        palette.change_epoch = palette.change_epoch.wrapping_add(1);
        let after_palette = stack.composite_layers(&palette);
        assert_eq!(
            after_palette.get_pixel(0, 0),
            Some(Color::TRANSPARENT),
            "a palette change re-rasterizes with the new palette (missing tile skipped)"
        );
    }

    #[test]
    fn tilemap_cache_cleared_when_tilemap_removed() {
        use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

        let mut stack = LayerStack::new(8, 8);
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 2, 2, [255, 0, 0, 255]));
        let id = tilemap_layer(&mut stack, "tiles", 4, &[(0, 0, TileCell::new(TileId(1)))]);
        stack.composite_layers(&palette);
        assert!(stack.layer(id).unwrap().tilemap_cache.borrow().is_some());

        stack.layer_mut(id).unwrap().tilemap = None;
        let out = stack.composite_layers(&palette);
        assert!(
            stack.layer(id).unwrap().tilemap_cache.borrow().is_none(),
            "removing the tilemap drops its render cache"
        );
        assert_eq!(out.get_pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn tile_overrides_composite_all_visible_instances_without_mutating_state() {
        let mut stack = LayerStack::new(16, 4);
        let background = stack.active_layer_id();
        stack
            .layer_mut(background)
            .unwrap()
            .buffer
            .fill(Color::rgb(20, 40, 60));

        let red = [255, 0, 0, 255];
        let green = [0, 255, 0, 255];
        let blue = [0, 0, 255, 255];
        let mut palette = TilePalette::new();
        palette.add(Tile {
            id: TileId(1),
            w: 2,
            h: 1,
            pixels: [red, green].concat(),
        });

        let first = tilemap_layer(
            &mut stack,
            "first instances",
            4,
            &[
                (0, 0, TileCell::new(TileId(1))),
                (
                    1,
                    0,
                    TileCell {
                        tile_id: TileId(1),
                        rotation: 0,
                        flip_x: true,
                        flip_y: false,
                    },
                ),
            ],
        );
        let mut rotated = TileCell::new(TileId(1));
        rotated.set_rotation(1);
        let second = tilemap_layer(&mut stack, "rotated instance", 4, &[(2, 0, rotated)]);
        let group = stack
            .create_group_around(&[first, second], "Tile preview group")
            .unwrap();
        stack.set_opacity(group, 0.6);
        stack.set_blend(group, BlendMode::Screen);

        let hidden = tilemap_layer(
            &mut stack,
            "hidden instance",
            4,
            &[(3, 0, TileCell::new(TileId(1)))],
        );
        stack.set_visible(hidden, false);

        let region = Rect2i::new(0, 0, 16, 4);
        // Populate the visible tilemap caches so the preview can prove that it
        // bypasses, rather than invalidates or refreshes, committed render state.
        let _ = stack.composite_layers_region(region, &palette).unwrap();
        let cache_state = |id: LayerId| {
            let cache = stack.layer(id).unwrap().tilemap_cache.borrow();
            cache.as_ref().map(|entry| {
                (
                    entry.tilemap_epoch,
                    entry.palette_epoch,
                    entry.canvas_w,
                    entry.canvas_h,
                    entry.buffer.as_bytes().as_ptr(),
                    entry.buffer.as_bytes().to_vec(),
                )
            })
        };
        let first_cache_before = cache_state(first);
        let second_cache_before = cache_state(second);
        let hidden_cache_before = cache_state(hidden);
        let palette_before = palette.clone();
        let palette_epoch_before = palette.change_epoch;

        let mut overrides = TilePixelOverrides::default();
        overrides.insert(TileId(1), 0, 0, blue);
        let preview = stack
            .composite_layers_region_with_tile_overrides(region, &palette, &overrides)
            .unwrap();

        assert_eq!(palette, palette_before);
        assert_eq!(palette.change_epoch, palette_epoch_before);
        assert_eq!(cache_state(first), first_cache_before);
        assert_eq!(cache_state(second), second_cache_before);
        assert_eq!(cache_state(hidden), hidden_cache_before);
        assert_eq!(
            preview.get_pixel(12, 0),
            Some(Color::rgb(20, 40, 60)),
            "the hidden tilemap instance must not leak into the preview"
        );

        // Compare against the committed render of the same root-pixel edit.
        // This exercises every visible shared-tile instance, its orientation,
        // layer ordering, and the enclosing group's opacity/blend semantics.
        let mut committed_palette = palette.clone();
        committed_palette.get_mut(TileId(1)).unwrap().pixels[..4].copy_from_slice(&blue);
        committed_palette.change_epoch = committed_palette.change_epoch.wrapping_add(1);
        let committed = stack
            .composite_layers_region(region, &committed_palette)
            .unwrap();
        assert_eq!(preview.as_bytes(), committed.as_bytes());
    }

    #[test]
    fn tilemap_dirty_region_and_partial_composite() {
        use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

        let mut stack = LayerStack::new(8, 8);
        let bottom = stack.active_layer_id();
        stack
            .layer_mut(bottom)
            .unwrap()
            .buffer
            .fill(Color::rgb(0, 255, 0));
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 2, 2, [255, 0, 0, 255]));
        let id = tilemap_layer(&mut stack, "tiles", 4, &[(1, 1, TileCell::new(TileId(1)))]);
        stack.layer_mut(id).unwrap().tilemap = None;
        // Re-attach the tilemap AFTER the dirty cells accumulated so the
        // dirty region is observable on the layer's tilemap.
        let mut tm = TileMap::new(4, 8, 8);
        tm.set_cell((1, 1), Some(TileCell::new(TileId(1))));
        stack.layer_mut(id).unwrap().tilemap = Some(tm);

        // The edited cell (1,1) marks canvas region (4,4,4,4) as dirty.
        let tm = stack.layer_mut(id).unwrap().tilemap.as_mut().unwrap();
        assert_eq!(
            tm.take_dirty_canvas_region(8, 8),
            Some(Rect2i::new(4, 4, 4, 4))
        );

        // A partial composite of that region returns the expected bytes: the
        // 2x2 red tile at (4,4) over the green bottom, transparent elsewhere.
        let region = stack
            .composite_layers_region(Rect2i::new(4, 4, 4, 4), &palette)
            .unwrap();
        assert_eq!(region.get_pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(region.get_pixel(1, 1), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(region.get_pixel(2, 0), Some(Color::rgb(0, 255, 0)));
        // And the partial region equals the full composite clipped to it.
        let full = stack.composite_layers(&palette);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(
                    region.get_pixel(x, y),
                    full.get_pixel(x + 4, y + 4),
                    "partial equals full-clipped at ({x},{y})"
                );
            }
        }
    }
}
