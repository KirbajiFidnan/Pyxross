//! Integrated tile model: root tile data is the single source of truth and the
//! layer BUFFER's tiled-cell footprints are baked copies of the oriented root
//! tile bytes.
//!
//! Core invariant: for every occupied tilemap cell, the layer buffer's
//! footprint rect `(cx*tile_size, cy*tile_size, ow, oh)` (ow/oh = oriented tile
//! dims) holds EXACTLY the oriented root-tile bytes. Editing a canvas cell that
//! a tile covers writes BACK to the root tile (un-oriented) and re-stamps ALL
//! instances of that tile on every layer, so the root data stays the single
//! source of truth. Clear (un-tile) does NOT touch the buffer: the last baked
//! content becomes ordinary visible pixels.
//!
//! This module is core-pure (CONTEXT.md §4.2): imports only the buffer, math,
//! tilemap, model and undo types — never egui/wgpu/winit.

use std::collections::BTreeMap;

use crate::core::buffer::PixelBuffer;
use crate::core::math::Rect2i;
use crate::core::model::{LayerId, LayerStack};
use crate::core::tilemap::{TileCell, TileId, TileMap, TilePalette, TilePixelOverrides};
use crate::core::undo::{TilePixelDiff, TilePixelEditCommand};

/// Inverse of the tile orientation: maps a canvas-footprint pixel back to the
/// ROOT tile's pixel coordinate.
///
/// `orient_tile_pixels` applies flips (X then Y) first, then `rotation`
/// clockwise 90° steps. The inverse therefore UNDOES the rotation FIRST (with
/// the CURRENT footprint width/height), then undoes the flips:
/// for each of the `rotation % 4` clockwise steps, the inverse step is
/// `(x, y) -> (y, cw - 1 - x)` (counter-clockwise), swapping (cw, ch); then
/// `flip_x` mirrors x and `flip_y` mirrors y.
///
/// `ox, oy` are the footprint-local pixel coordinates; `ow, oh` the ORIENTED
/// (footprint) tile dimensions; `tile_w, tile_h` the ROOT tile dimensions.
pub fn unorient_pixel(
    cell: TileCell,
    tile_w: u32,
    tile_h: u32,
    ox: u32,
    oy: u32,
    ow: u32,
    oh: u32,
) -> (u32, u32) {
    let mut x = ox;
    let mut y = oy;
    let mut cw = ow;
    let mut ch = oh;
    // Undo rotation: each clockwise step maps (x, y) -> (cw-1-y, x); invert it.
    for _ in 0..(cell.rotation % 4) {
        let nx = y;
        let ny = cw.saturating_sub(1).saturating_sub(x);
        x = nx;
        y = ny;
        // The footprint dims swap with each rotation step.
        let next_w = ch;
        let next_h = cw;
        cw = next_w;
        ch = next_h;
    }
    let tx = if cell.flip_x {
        cw.saturating_sub(1).saturating_sub(x)
    } else {
        x
    };
    let ty = if cell.flip_y {
        ch.saturating_sub(1).saturating_sub(y)
    } else {
        y
    };
    // Clamp into the root tile so an out-of-bounds (clipped) footprint pixel
    // never indexes past the root tile's buffer.
    (
        tx.min(tile_w.saturating_sub(1)),
        ty.min(tile_h.saturating_sub(1)),
    )
}

/// Collect every canvas-space footprint rect for an occupied cell on `tm`.
fn cell_footprints(
    tm: &TileMap,
    pal: &TilePalette,
    cx: u32,
    cy: u32,
) -> Option<(TileCell, u32, u32, Rect2i)> {
    let cell = tm.cell((cx, cy))?;
    let tile = pal.get(cell.tile_id)?;
    let tile_size = tm.tile_size.max(1);
    let rotated = cell.rotation % 2 == 1;
    let (ow, oh) = if rotated {
        (u32::from(tile.h), u32::from(tile.w))
    } else {
        (u32::from(tile.w), u32::from(tile.h))
    };
    let ox = (cx as u64 * u64::from(tile_size)) as i32;
    let oy = (cy as u64 * u64::from(tile_size)) as i32;
    Some((cell, ow, oh, Rect2i::new(ox, oy, ow as i32, oh as i32)))
}

/// Reads the root tile's RGBA8 byte at `(tx, ty)`.
fn root_byte_at(pal: &TilePalette, id: TileId, tx: u32, ty: u32, tile_w: u32) -> [u8; 4] {
    let Some(tile) = pal.get(id) else {
        return [0; 4];
    };
    let index = ((ty as usize) * tile_w as usize + tx as usize) * 4;
    let bytes = &tile.pixels;
    let mut out = [0u8; 4];
    let end = (index + 4).min(bytes.len());
    out[..4.min(end.saturating_sub(index))].copy_from_slice(&bytes[index..end]);
    out
}

/// Writes the given RGBA8 byte back into the root tile at `(tx, ty)`.
fn set_root_byte(pal: &mut TilePalette, id: TileId, tx: u32, ty: u32, tile_w: u32, byte: &[u8]) {
    let Some(tile) = pal.get_mut(id) else {
        return;
    };
    let index = ((ty as usize) * tile_w as usize + tx as usize) * 4;
    let end = (index + 4).min(tile.pixels.len());
    if index < end {
        tile.pixels[index..end].copy_from_slice(&byte[..end - index]);
    }
}

/// The byte index of a pixel in a `w`-wide row-major buffer at `(x, y)`.
fn byte_index(x: u32, y: u32, w: u32) -> usize {
    ((y as usize) * w as usize + x as usize) * 4
}

/// Collects root-pixel edits in precisely the traversal order used by the
/// committed write-back path: source cells row-major, then changed canvas
/// pixels row-major, then overlapping cells in their row-major order.
fn collect_root_edits(
    layers: &LayerStack,
    palette: &TilePalette,
    layer_id: LayerId,
    region: Rect2i,
    before: &[u8],
    after: &[u8],
) -> BTreeMap<(TileId, u32, u32), ([u8; 4], [u8; 4])> {
    let mut edits = BTreeMap::new();
    let Some(layer) = layers.layer(layer_id) else {
        return edits;
    };
    let Some(tm) = layer.tilemap.as_ref() else {
        return edits;
    };
    if region.is_empty() || region.x < 0 || region.y < 0 {
        return edits;
    }
    let Some(expected_len) = (region.w as usize)
        .checked_mul(region.h as usize)
        .and_then(|area| area.checked_mul(4))
    else {
        return edits;
    };
    if before.len() != expected_len || after.len() != expected_len {
        return edits;
    }

    let canvas_w = layer.buffer.width() as u32;
    let canvas_h = layer.buffer.height() as u32;
    let cols = tm.cols.max(1);
    let rows = tm.rows.max(1);

    // Precompute source cell footprints in active-layer row-major order.
    let mut footprints: Vec<(TileCell, u32, u32, u32, u32, Rect2i)> = Vec::new();
    for cy in 0..rows {
        for cx in 0..cols {
            let Some((cell, ow, oh, rect)) = cell_footprints(tm, palette, cx, cy) else {
                continue;
            };
            let Some(tile) = palette.get(cell.tile_id) else {
                continue;
            };
            if !rect.intersects(region) {
                continue;
            }
            footprints.push((cell, ow, oh, u32::from(tile.w), u32::from(tile.h), rect));
        }
    }
    if footprints.is_empty() {
        return edits;
    }

    let region_x0 = region.x as u32;
    let region_y0 = region.y as u32;
    let region_x1 = (region.right() as u32).min(canvas_w);
    let region_y1 = (region.bottom() as u32).min(canvas_h);
    for y in region_y0..region_y1 {
        for x in region_x0..region_x1 {
            let idx = byte_index(x - region_x0, y - region_y0, region.w as u32);
            let b = &before[idx..idx + 4];
            let a = &after[idx..idx + 4];
            if b == a {
                continue;
            }
            for &(cell, ow, oh, tw, th, rect) in &footprints {
                let xi = x as i32;
                let yi = y as i32;
                if !(xi >= rect.x && yi >= rect.y && xi < rect.right() && yi < rect.bottom()) {
                    continue;
                }
                let lx = x - rect.x as u32;
                let ly = y - rect.y as u32;
                let (tx, ty) = unorient_pixel(cell, tw, th, lx, ly, ow, oh);
                let key = (cell.tile_id, ty, tx);
                let root_before = root_byte_at(palette, cell.tile_id, tx, ty, tw);
                let after_byte = [a[0], a[1], a[2], a[3]];
                match edits.get_mut(&key) {
                    Some((_, last_after)) => *last_after = after_byte,
                    None => {
                        edits.insert(key, (root_before, after_byte));
                    }
                }
            }
        }
    }
    edits
}

/// Derives render-only root-pixel replacements from a canvas stroke delta.
///
/// This is pure: it does not mutate layer buffers, tilemaps, palette pixels,
/// palette epochs, caches, or undo state. The mapping/order is shared with
/// [`write_back_region`], including the last-write winner for overlapping
/// source cell footprints.
pub fn derive_tile_pixel_overrides(
    layers: &LayerStack,
    palette: &TilePalette,
    layer_id: LayerId,
    region: Rect2i,
    before: &[u8],
    after: &[u8],
) -> TilePixelOverrides {
    let mut overrides = TilePixelOverrides::default();
    for ((id, y, x), (_, after)) in
        collect_root_edits(layers, palette, layer_id, region, before, after)
    {
        overrides.insert(id, x, y, after);
    }
    overrides
}

/// Write back the changed pixels of a region to the ROOT tiles they cover.
///
/// For each `(x, y)` in `region` where `before != after`, every occupied cell
/// whose footprint covers `(x, y)` maps the pixel through [`unorient_pixel`]
/// and records it. Per `(tile_id, tx, ty)` the FIRST root-before byte (read
/// from the ROOT, not the buffer) and the LAST `after` byte are kept — the
/// stroke's undo region is the full canvas, but multiple cells may cover the
/// same root pixel, and the LAST edit wins for redo while the FIRST before is
/// what a single undo must restore.
///
/// Writes the recorded `after` bytes into the roots, bumps the palette epoch
/// (invalidating every layer's render cache), re-stamps every instance of the
/// edited tiles, and returns a [`TilePixelEditCommand`] (or `None` when no
/// tiled cell was touched).
pub fn write_back_region(
    layers: &mut LayerStack,
    palette: &mut TilePalette,
    layer_id: LayerId,
    region: Rect2i,
    before: &[u8],
    after: &[u8],
) -> Option<TilePixelEditCommand> {
    if layers
        .layer(layer_id)
        .and_then(|layer| layer.tilemap.as_ref())
        .is_none()
    {
        return None;
    }
    let edits = collect_root_edits(layers, palette, layer_id, region, before, after);
    if edits.is_empty() {
        return None;
    }
    let edited_ids: BTreeMap<TileId, ()> = edits.keys().map(|(id, _, _)| (*id, ())).collect();

    // Record the per-tile diffs in deterministic (tile_id, ty, tx) order.
    let mut diffs: Vec<TilePixelDiff> = Vec::new();
    let mut current: Option<(TileId, Vec<(u32, u32)>, Vec<u8>, Vec<u8>)> = None;
    for ((id, ty, tx), (before_byte, after_byte)) in edits {
        match current.as_mut() {
            Some((cur_id, coords, before_bytes, after_bytes)) if *cur_id == id => {
                coords.push((tx, ty));
                before_bytes.extend_from_slice(&before_byte);
                after_bytes.extend_from_slice(&after_byte);
            }
            _ => {
                if let Some((cur_id, coords, before_bytes, after_bytes)) = current.take() {
                    diffs.push(TilePixelDiff {
                        tile_id: cur_id,
                        pixels: coords,
                        before: before_bytes,
                        after: after_bytes,
                    });
                }
                current = Some((
                    id,
                    vec![(tx, ty)],
                    before_byte.to_vec(),
                    after_byte.to_vec(),
                ));
            }
        }
    }
    if let Some((cur_id, coords, before_bytes, after_bytes)) = current.take() {
        diffs.push(TilePixelDiff {
            tile_id: cur_id,
            pixels: coords,
            before: before_bytes,
            after: after_bytes,
        });
    }

    // Apply the edits to the roots.
    for diff in &diffs {
        let Some(tile) = palette.get_mut(diff.tile_id) else {
            continue;
        };
        let tw = u32::from(tile.w);
        for (i, &(tx, ty)) in diff.pixels.iter().enumerate() {
            let byte = &diff.after[i * 4..i * 4 + 4];
            set_root_byte(palette, diff.tile_id, tx, ty, tw, byte);
        }
    }
    // Palette epoch bump invalidates every layer's tilemap render cache.
    palette.change_epoch = palette.change_epoch.wrapping_add(1);

    // Re-stamp every instance of the edited tiles on every layer.
    let edited: Vec<TileId> = edited_ids.keys().copied().collect();
    re_stamp_tile_instances(layers, palette, &edited);

    Some(TilePixelEditCommand::new(diffs))
}

/// Re-stamps every cell referencing any of `ids` on every layer, blitting the
/// oriented root tile into the layer buffer's cell footprint (maintains the
/// core invariant: buffer footprint == oriented tile bytes).
pub fn re_stamp_tile_instances(layers: &mut LayerStack, palette: &TilePalette, ids: &[TileId]) {
    if ids.is_empty() {
        return;
    }
    for layer in layers.iter_mut() {
        let Some(tm) = layer.tilemap.as_mut() else {
            continue;
        };
        let cols = tm.cols.max(1);
        for cy in 0..tm.rows.max(1) {
            for cx in 0..cols {
                let Some(cell) = tm.cell((cx, cy)) else {
                    continue;
                };
                if !ids.contains(&cell.tile_id) {
                    continue;
                }
                tm.blit_cell(palette, &mut layer.buffer, cx, cy);
            }
        }
    }
}

/// Load migration: blits every occupied cell on every layer into its buffer
/// footprint (used once on load for old documents).
pub fn bake_all_tiles(layers: &mut LayerStack, palette: &TilePalette) {
    for layer in layers.iter_mut() {
        let Some(tm) = layer.tilemap.as_mut() else {
            continue;
        };
        let cols = tm.cols.max(1);
        for cy in 0..tm.rows.max(1) {
            for cx in 0..cols {
                tm.blit_cell(palette, &mut layer.buffer, cx, cy);
            }
        }
    }
}

/// Blit helper used by tests and the loader: one cell's oriented footprint.
/// Mirrors `TileMap::blit_cell`.
pub fn blit_one_cell(
    tm: &TileMap,
    pal: &TilePalette,
    dst: &mut PixelBuffer,
    cx: u32,
    cy: u32,
) -> bool {
    tm.blit_cell(pal, dst, cx, cy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::LayerStack;
    use crate::core::tilemap::{Tile, TileCell, TileId};
    use crate::core::undo::Command;

    fn solid_tile(id: u64, w: u16, h: u16, color: [u8; 4]) -> Tile {
        Tile {
            id: TileId(id),
            w,
            h,
            pixels: color.repeat(w as usize * h as usize),
        }
    }

    fn px(buf: &PixelBuffer, x: u32, y: u32) -> [u8; 4] {
        let i = byte_index(x, y, buf.width() as u32);
        let b = buf.as_bytes();
        [b[i], b[i + 1], b[i + 2], b[i + 3]]
    }

    #[test]
    fn unorient_identity_and_round_trip() {
        let cell = TileCell::new(TileId(1));
        assert_eq!(unorient_pixel(cell, 2, 2, 0, 0, 2, 2), (0, 0));
        assert_eq!(unorient_pixel(cell, 2, 2, 1, 0, 2, 2), (1, 0));
        assert_eq!(unorient_pixel(cell, 2, 2, 0, 1, 2, 2), (0, 1));
        assert_eq!(unorient_pixel(cell, 2, 2, 1, 1, 2, 2), (1, 1));
    }

    #[test]
    fn unorient_rotation_round_trip() {
        // A 2x2 tile rotated 90° CW. Forward orient maps root (x,y) →
        // footprint (1-y, x) (for a square tile). The inverse therefore maps
        // footprint (fx,fy) → root (fy, 1-fx).
        let mut cell = TileCell::new(TileId(1));
        cell.set_rotation(1);
        // Root (0,0) → footprint (1,0); invert back.
        assert_eq!(unorient_pixel(cell, 2, 2, 1, 0, 2, 2), (0, 0));
        // Root (1,0) → footprint (1,1); invert back.
        assert_eq!(unorient_pixel(cell, 2, 2, 1, 1, 2, 2), (1, 0));
        // Root (0,1) → footprint (0,0); invert back.
        assert_eq!(unorient_pixel(cell, 2, 2, 0, 0, 2, 2), (0, 1));
        // Root (1,1) → footprint (0,1); invert back.
        assert_eq!(unorient_pixel(cell, 2, 2, 0, 1, 2, 2), (1, 1));
    }

    #[test]
    fn unorient_flip_round_trip() {
        let cell = TileCell {
            tile_id: TileId(1),
            rotation: 0,
            flip_x: true,
            flip_y: true,
        };
        // Root (1,1) -> footprint (0,0) under both flips; invert back.
        assert_eq!(unorient_pixel(cell, 2, 2, 0, 0, 2, 2), (1, 1));
        assert_eq!(unorient_pixel(cell, 2, 2, 1, 1, 2, 2), (0, 0));
    }

    #[test]
    fn unorient_rotation_then_flip_round_trip() {
        // Rotation is applied AFTER flips in orient, so the inverse undoes
        // rotation first, then flips. Root (1,0) flipped-x -> (0,0), rotated
        // 90° CW -> footprint (0,1)? Verify the inverse returns (1,0).
        let cell = TileCell {
            tile_id: TileId(1),
            rotation: 1,
            flip_x: true,
            flip_y: false,
        };
        // Simulate forward orient for root (1,0): flip_x -> (0,0); rotate 90°
        // CW: (x,y)=(0,0) -> (nw-1-y, x) = (1, 0) with nw=2. Footprint (1,0).
        // Invert: undo rotation on footprint (1,0): (y, cw-1-x) = (0, 2-1-1)= (0,0);
        // then flip_x on (0,0) -> (1,0). So unorient(1,0) == (1,0).
        assert_eq!(unorient_pixel(cell, 2, 2, 1, 0, 2, 2), (1, 0));
        // Footprint (0,1): undo rotation: (y, cw-1-x) = (1, 1) -> flip_x -> (0,1).
        assert_eq!(unorient_pixel(cell, 2, 2, 0, 1, 2, 2), (0, 1));
    }

    #[test]
    fn write_back_updates_root_and_all_instances() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255])); // root tile
        let mut tm = TileMap::new(4, 2, 2);
        // Two cells both reference tile 1: (0,0) identity, (1,0) identity.
        tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        tm.set_cell((1, 0), Some(TileCell::new(TileId(1))));
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        // Bake the initial instances.
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);

        // Pencil a cell at canvas (1,0): covered by both cell (0,0) and (1,0).
        let region = Rect2i::new(1, 0, 1, 1);
        let before = {
            let buf = &layers.layer(lid).unwrap().buffer;
            let i = byte_index(1, 0, 8);
            buf.as_bytes()[i..i + 4].to_vec()
        };
        // The after byte (a red pixel).
        let after = vec![255, 0, 0, 255];
        // Simulate the stroke having been applied to the buffer.
        layers.layer_mut(lid).unwrap().buffer.set_pixel(
            1,
            0,
            crate::core::color::Color::rgb(255, 0, 0),
        );
        let cmd = write_back_region(&mut layers, &mut palette, lid, region, &before, &after)
            .expect("tiled cell touched");
        assert_eq!(cmd.name(), "Tile Edit");

        // Root tile updated at the un-oriented offset: canvas (1,0) -> cell
        // (0,0) local (1,0) -> root (1,0).
        let root = palette.get(TileId(1)).unwrap();
        let i = byte_index(1, 0, 2);
        assert_eq!(&root.pixels[i..i + 4], &[255, 0, 0, 255]);
        // Root (0,0) unchanged.
        let i0 = byte_index(0, 0, 2);
        assert_eq!(&root.pixels[i0..i0 + 4], &[10, 20, 30, 255]);

        // Both instances re-baked: cell (0,0) footprint and cell (1,0)
        // footprint both show the red pixel at their local (1,0).
        let buf = &layers.layer(lid).unwrap().buffer;
        assert_eq!(px(buf, 1, 0), [255, 0, 0, 255]); // cell (0,0)
        assert_eq!(px(buf, 5, 0), [255, 0, 0, 255]); // cell (1,0) at canvas 4+1
                                                     // Other root pixels unchanged on both.
        assert_eq!(px(buf, 0, 0), [10, 20, 30, 255]);
        assert_eq!(px(buf, 4, 0), [10, 20, 30, 255]);
    }

    #[test]
    fn write_back_unselected_cell_returns_none() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255]));
        let mut tm = TileMap::new(4, 2, 2);
        tm.set_cell((1, 1), Some(TileCell::new(TileId(1)))); // far from origin
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);

        // An untiled cell: nothing written back.
        let region = Rect2i::new(0, 0, 1, 1);
        let before = vec![0, 0, 0, 0];
        let after = vec![255, 255, 255, 255];
        assert!(
            write_back_region(&mut layers, &mut palette, lid, region, &before, &after).is_none()
        );
        let root = palette.get(TileId(1)).unwrap();
        assert_eq!(&root.pixels[..4], &[10, 20, 30, 255]);
    }

    #[test]
    fn write_back_palette_epoch_bumps() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255]));
        let mut tm = TileMap::new(4, 2, 2);
        tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);
        let epoch = palette.change_epoch;
        let region = Rect2i::new(1, 0, 1, 1);
        let before = vec![10, 20, 30, 255];
        let after = vec![255, 0, 0, 255];
        assert!(
            write_back_region(&mut layers, &mut palette, lid, region, &before, &after).is_some()
        );
        assert_ne!(palette.change_epoch, epoch);
    }

    #[test]
    fn write_back_first_before_last_after() {
        // Two cells covering the SAME root pixel with DIFFERENT edits in one
        // region: the first root-before must be recorded, the last after wins.
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255]));
        let mut tm = TileMap::new(4, 2, 2);
        tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        tm.set_cell((1, 0), Some(TileCell::new(TileId(1))));
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);
        // Canvas (5,0) is cell (1,0) local (1,0) == root (1,0) — same root
        // pixel as cell (0,0) local (1,0). Edit both in one region: the undo
        // before must be the ORIGINAL root byte (10,20,30,255) not the buffer.
        let region = Rect2i::new(1, 0, 5, 1); // x in 1..5 covers both cells' (1,0)
        let before = {
            let buf = &layers.layer(lid).unwrap().buffer;
            let mut v = Vec::new();
            for x in 1..6 {
                let i = byte_index(x, 0, 8);
                v.extend_from_slice(&buf.as_bytes()[i..i + 4]);
            }
            v
        };
        let mut after = before.clone();
        for i in (0..after.len()).step_by(4) {
            after[i..i + 4].copy_from_slice(&[99, 88, 77, 255]);
        }
        // Apply "after" to the buffer.
        for x in 1..6 {
            layers.layer_mut(lid).unwrap().buffer.set_pixel(
                x,
                0,
                crate::core::color::Color::rgb(99, 88, 77),
            );
        }
        let cmd = write_back_region(&mut layers, &mut palette, lid, region, &before, &after)
            .expect("tiled cell touched");
        // Root (1,0) now holds the LAST after (99,88,77,255).
        let root = palette.get(TileId(1)).unwrap();
        let i = byte_index(1, 0, 2);
        assert_eq!(&root.pixels[i..i + 4], &[99, 88, 77, 255]);
        // The recorded diff's before is the ORIGINAL root byte.
        let diff = cmd.diffs().first().unwrap();
        let pos = diff
            .pixels
            .iter()
            .position(|&(tx, ty)| tx == 1 && ty == 0)
            .unwrap();
        let bi = pos * 4;
        assert_eq!(&diff.before[bi..bi + 4], &[10, 20, 30, 255]);
    }

    #[test]
    fn derived_overrides_are_pure_and_match_committed_rotated_flipped_conflicts() {
        let mut layers = LayerStack::new(8, 6);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(Tile {
            id: TileId(1),
            w: 2,
            h: 2,
            pixels: [
                [10, 20, 30, 255],
                [40, 50, 60, 255],
                [70, 80, 90, 255],
                [100, 110, 120, 255],
            ]
            .concat(),
        });
        let mut tm = TileMap::new(1, 8, 6);
        tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        tm.set_cell(
            (1, 0),
            Some(TileCell {
                tile_id: TileId(1),
                rotation: 0,
                flip_x: true,
                flip_y: false,
            }),
        );
        tm.set_cell(
            (4, 0),
            Some(TileCell {
                tile_id: TileId(1),
                rotation: 1,
                flip_x: false,
                flip_y: true,
            }),
        );
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);

        let region = Rect2i::new(0, 0, 6, 2);
        let before = layers
            .layer(lid)
            .unwrap()
            .buffer
            .export_region(region, None)
            .unwrap();
        let mut after = before.clone();
        for y in 0..2usize {
            for x in 0..6usize {
                let offset = (y * 6 + x) * 4;
                after[offset..offset + 4]
                    .copy_from_slice(&[x as u8 * 20 + 1, y as u8 * 30 + 2, 199, 255]);
            }
        }

        let buffer_before = layers.layer(lid).unwrap().buffer.as_bytes().to_vec();
        let palette_before = palette.clone();
        let palette_epoch_before = palette.change_epoch;
        let tilemap_epoch_before = layers.layer(lid).unwrap().tilemap.as_ref().unwrap().change_epoch;
        let overrides = derive_tile_pixel_overrides(&layers, &palette, lid, region, &before, &after);
        assert_eq!(layers.layer(lid).unwrap().buffer.as_bytes(), buffer_before);
        assert_eq!(palette, palette_before);
        assert_eq!(palette.change_epoch, palette_epoch_before);
        assert_eq!(
            layers.layer(lid).unwrap().tilemap.as_ref().unwrap().change_epoch,
            tilemap_epoch_before
        );
        assert!(layers.layer(lid).unwrap().tilemap_cache.borrow().is_none());
        // Traversal is row-major in canvas space, so the y=1 row is visited
        // after y=0. The rotated+flip_y source cell at grid (4,0) maps canvas
        // (4,1) back to root pixel (1,0); being the last writer in that order,
        // it wins with the after byte [4*20+1, 1*30+2, 199, 255].
        assert_eq!(overrides.get(TileId(1), 1, 0), Some([81, 32, 199, 255]));

        let mut committed_layers = layers;
        let mut committed_palette = palette;
        let cmd = write_back_region(
            &mut committed_layers,
            &mut committed_palette,
            lid,
            region,
            &before,
            &after,
        )
        .expect("the source region overlaps occupied tile cells");
        assert_eq!(cmd.name(), "Tile Edit");
        let root_before = palette_before.get(TileId(1)).unwrap();
        let root_after = committed_palette.get(TileId(1)).unwrap();
        for y in 0..2 {
            for x in 0..2 {
                let offset = byte_index(x, y, 2);
                let committed = [
                    root_after.pixels[offset],
                    root_after.pixels[offset + 1],
                    root_after.pixels[offset + 2],
                    root_after.pixels[offset + 3],
                ];
                let original = [
                    root_before.pixels[offset],
                    root_before.pixels[offset + 1],
                    root_before.pixels[offset + 2],
                    root_before.pixels[offset + 3],
                ];
                assert_eq!(overrides.get(TileId(1), x, y).unwrap_or(original), committed);
            }
        }
    }

    #[test]
    fn cross_layer_rebake_updates_other_layers() {
        let mut layers = LayerStack::new(8, 8);
        let lid_a = layers.active_layer_id();
        let lid_b = layers.add_layer("B");
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255]));
        for lid in [lid_a, lid_b] {
            let mut tm = TileMap::new(4, 2, 2);
            tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
            layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        }
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);
        let region = Rect2i::new(1, 0, 1, 1);
        let before = vec![10, 20, 30, 255];
        let after = vec![255, 0, 0, 255];
        layers.layer_mut(lid_a).unwrap().buffer.set_pixel(
            1,
            0,
            crate::core::color::Color::rgb(255, 0, 0),
        );
        assert!(
            write_back_region(&mut layers, &mut palette, lid_a, region, &before, &after).is_some()
        );
        // Both layers' instances re-baked.
        let buf_b = &layers.layer(lid_b).unwrap().buffer;
        assert_eq!(px(buf_b, 1, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn tile_pixel_edit_command_undo_redo_round_trip() {
        use crate::core::undo::{Command, CommandContext};

        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255]));
        let mut tm = TileMap::new(4, 2, 2);
        tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        tm.set_cell((1, 0), Some(TileCell::new(TileId(1))));
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);

        // Build a TilePixelEditCommand via write_back_region.
        let region = Rect2i::new(1, 0, 1, 1);
        let before = vec![10, 20, 30, 255];
        let after = vec![255, 0, 0, 255];
        layers.layer_mut(lid).unwrap().buffer.set_pixel(
            1,
            0,
            crate::core::color::Color::rgb(255, 0, 0),
        );
        let mut cmd = write_back_region(&mut layers, &mut palette, lid, region, &before, &after)
            .expect("tiled cell touched");

        // Undo restores the root byte and re-bakes both instances.
        let mut palette2 = palette.clone();
        {
            let mut ctx = CommandContext::new(&mut layers, &mut palette2);
            assert!(cmd.undo(&mut ctx));
        }
        let root = palette2.get(TileId(1)).unwrap();
        let i = byte_index(1, 0, 2);
        assert_eq!(&root.pixels[i..i + 4], &[10, 20, 30, 255]);
        assert_eq!(
            px(&layers.layer(lid).unwrap().buffer, 1, 0),
            [10, 20, 30, 255]
        );
        assert_eq!(
            px(&layers.layer(lid).unwrap().buffer, 5, 0),
            [10, 20, 30, 255]
        );

        // Redo re-applies the after byte and re-bakes.
        {
            let mut ctx = CommandContext::new(&mut layers, &mut palette2);
            assert!(cmd.redo(&mut ctx));
        }
        let root = palette2.get(TileId(1)).unwrap();
        assert_eq!(&root.pixels[i..i + 4], &[255, 0, 0, 255]);
        assert_eq!(
            px(&layers.layer(lid).unwrap().buffer, 1, 0),
            [255, 0, 0, 255]
        );
        assert_eq!(
            px(&layers.layer(lid).unwrap().buffer, 5, 0),
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn stamp_clear_buffer_invariant() {
        // After a stamp, the buffer footprint holds EXACTLY the oriented tile
        // bytes. After a clear, the last baked content stays as ordinary
        // pixels (the buffer is untouched by un-tiling).
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255]));
        let mut tm = TileMap::new(4, 2, 2);
        tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        // Bake via blit_cell.
        let baked = {
            let layer = layers.layer_mut(lid).unwrap();
            layer
                .tilemap
                .as_ref()
                .unwrap()
                .blit_cell(&palette, &mut layer.buffer, 0, 0)
        };
        assert!(baked);
        let buf = &layers.layer(lid).unwrap().buffer;
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(px(buf, x, y), [10, 20, 30, 255], "footprint == tile bytes");
            }
        }
        // Clear the cell: buffer stays as the last baked content.
        layers
            .layer_mut(lid)
            .unwrap()
            .tilemap
            .as_mut()
            .unwrap()
            .set_cell((0, 0), None);
        let buf = &layers.layer(lid).unwrap().buffer;
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(px(buf, x, y), [10, 20, 30, 255]);
            }
        }
    }

    #[test]
    fn write_back_palette_epoch_invalidates_render_cache() {
        use crate::core::model::LayerStack;

        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut palette = TilePalette::new();
        palette.add(solid_tile(1, 2, 2, [10, 20, 30, 255]));
        let mut tm = TileMap::new(4, 2, 2);
        tm.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        layers.layer_mut(lid).unwrap().tilemap = Some(tm);
        re_stamp_tile_instances(&mut layers, &palette, &[TileId(1)]);

        // Composite once (populates the render cache).
        let before = layers.composite_layers(&palette);
        let i = byte_index(1, 0, 8);
        assert_eq!(&before.as_bytes()[i..i + 4], &[10, 20, 30, 255]);

        // Write back a pixel: the palette epoch bumps, so the NEXT composite
        // must re-rasterize and show the edited pixel even though the buffer
        // was also re-baked (cache invalidation is belt-and-braces).
        let region = Rect2i::new(1, 0, 1, 1);
        let before_bytes = vec![10, 20, 30, 255];
        let after_bytes = vec![255, 0, 0, 255];
        layers.layer_mut(lid).unwrap().buffer.set_pixel(
            1,
            0,
            crate::core::color::Color::rgb(255, 0, 0),
        );
        assert!(write_back_region(
            &mut layers,
            &mut palette,
            lid,
            region,
            &before_bytes,
            &after_bytes
        )
        .is_some());
        let after = layers.composite_layers(&palette);
        assert_eq!(&after.as_bytes()[i..i + 4], &[255, 0, 0, 255]);
    }
}
