//! Tilemaps — a grid of tile cells referencing palette tiles, plus the
//! project's tile palette (self-contained pixel data).
//!
//! A [`TilePalette`] owns the pixel data for each tile (straight RGBA8). A
//! [`TileMap`] is a grid of [`TileCell`] references into the palette; each cell
//! carries a per-cell transform (rotation × 90° clockwise, and X/Y flips).
//! [`TileMap::rasterize`] composites the palette tiles into a pixel buffer,
//! palette-pure (verbatim bytes, no blending).
//!
//! This module is pure core (CONTEXT.md §4.2): it imports only `serde` and the
//! pixel-buffer type, never egui/wgpu/winit, so the model stays
//! headless-testable via `cargo test`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::core::buffer::PixelBuffer;
use crate::core::math::Rect2i;

/// Normalizes a rotation step count into `0..=3` (the number of 90° clockwise
/// turns). Out-of-range values wrap; `u8` is far smaller than any overflow
/// boundary here.
fn normalize_rotation(steps: u8) -> u8 {
    steps % 4
}

/// Deserializes a `TileCell.rotation` and normalizes it to `0..=3`, so a
/// hand-edited or corrupt manifest can never introduce an out-of-range step.
fn deserialize_rotation<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = u8::deserialize(deserializer)?;
    Ok(normalize_rotation(raw))
}

/// Stable identity of a palette tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TileId(pub u64);

/// Sparse, render-only replacements for root-tile pixels.
///
/// The nested ordered maps make both pixel lookup and tile iteration
/// deterministic without exposing the storage representation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TilePixelOverrides {
    pixels: BTreeMap<TileId, BTreeMap<(u32, u32), [u8; 4]>>,
}

impl TilePixelOverrides {
    /// The preview byte for a root pixel, if one is present.
    pub fn get(&self, id: TileId, x: u32, y: u32) -> Option<[u8; 4]> {
        self.pixels.get(&id)?.get(&(y, x)).copied()
    }

    /// Whether any root pixel of `id` is overridden.
    pub fn contains_tile(&self, id: TileId) -> bool {
        self.pixels.contains_key(&id)
    }

    /// Whether there are no preview pixels.
    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }

    /// Overridden tile ids in ascending deterministic order.
    pub fn tile_ids(&self) -> impl Iterator<Item = TileId> + '_ {
        self.pixels.keys().copied()
    }

    pub(crate) fn insert(&mut self, id: TileId, x: u32, y: u32, rgba: [u8; 4]) {
        self.pixels.entry(id).or_default().insert((y, x), rgba);
    }
}

/// A palette tile: self-contained pixel data.
///
/// `pixels` is row-major straight RGBA8 with `len == w * h * 4`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    pub id: TileId,
    pub w: u16,
    pub h: u16,
    pub pixels: Vec<u8>,
}

/// Named, selectable collection of tiles.
///
/// `next_id` is DERIVED from the stored tiles (max id + 1, starting at 1), so
/// ids are deterministic across save/load and removing the highest tile frees
/// its id for reuse. Invariants (enforced by the methods): ids are unique,
/// `selected` always names an existing tile or is `None`, and every operation
/// is deterministic and panic-free.
///
/// `change_epoch` is a RUNTIME change counter (never persisted, `#[serde(skip)]`)
/// bumped on every structural mutation (add/remove) so tilemap rasterize caches
/// can detect palette-driven invalidation. It is excluded from `PartialEq`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TilePalette {
    /// Tiles in insertion order.
    pub tiles: Vec<Tile>,
    /// Id of the selected tile; `None` when nothing is selected.
    pub selected: Option<TileId>,
    /// Runtime change counter (add/remove); not persisted.
    #[serde(skip)]
    pub change_epoch: u64,
}

impl PartialEq for TilePalette {
    fn eq(&self, other: &Self) -> bool {
        self.tiles == other.tiles && self.selected == other.selected
    }
}

impl TilePalette {
    /// An empty palette with nothing selected.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next id [`Self::add`] would assign: `1` when empty, otherwise
    /// `max(existing id) + 1`.
    pub fn next_id(&self) -> u64 {
        self.tiles
            .iter()
            .map(|tile| tile.id.0)
            .max()
            .map_or(1, |max| max.saturating_add(1))
    }

    /// Adds `tile`, assigning a fresh id when `0` or already present, and
    /// returns the stored id. Insertion order is preserved; selection is
    /// unchanged.
    pub fn add(&mut self, mut tile: Tile) -> TileId {
        if tile.id.0 == 0 || self.tiles.iter().any(|existing| existing.id == tile.id) {
            tile.id = TileId(self.next_id());
        }
        let id = tile.id;
        self.tiles.push(tile);
        self.change_epoch = self.change_epoch.wrapping_add(1);
        id
    }

    /// Removes the tile with `id`, returning `true` when one was removed. If the
    /// removed tile was selected, `selected` is cleared.
    pub fn remove(&mut self, id: TileId) -> bool {
        let before = self.tiles.len();
        self.tiles.retain(|tile| tile.id != id);
        let removed = self.tiles.len() != before;
        if removed {
            self.change_epoch = self.change_epoch.wrapping_add(1);
        }
        if removed && self.selected == Some(id) {
            self.selected = None;
        }
        removed
    }

    /// Selects the tile with `id`, returning `true` when it exists. An unknown
    /// id leaves the selection unchanged and returns `false`.
    pub fn select(&mut self, id: TileId) -> bool {
        if self.tiles.iter().any(|tile| tile.id == id) {
            self.selected = Some(id);
            true
        } else {
            false
        }
    }

    /// The selected tile, or `None` when nothing (valid) is selected.
    pub fn selected_tile(&self) -> Option<&Tile> {
        let id = self.selected?;
        self.tiles.iter().find(|tile| tile.id == id)
    }

    /// The tile with `id`, or `None`.
    pub fn get(&self, id: TileId) -> Option<&Tile> {
        self.tiles.iter().find(|tile| tile.id == id)
    }

    /// Mutable access to the tile with `id`, or `None`.
    ///
    /// Used by the integrated tile model's write-back path (editing a canvas
    /// cell writes through to the root tile data). Mutating `tile.pixels`
    /// directly does NOT bump `change_epoch`; callers that change pixel data
    /// must bump it themselves so every layer's render cache invalidates.
    pub fn get_mut(&mut self, id: TileId) -> Option<&mut Tile> {
        self.tiles.iter_mut().find(|tile| tile.id == id)
    }

    /// Whether the palette holds no tiles.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

/// One occupied grid cell: a palette reference plus a per-cell transform.
///
/// `rotation` counts 90° CLOCKWISE steps and is always normalized to `0..=3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileCell {
    pub tile_id: TileId,
    #[serde(deserialize_with = "deserialize_rotation")]
    pub rotation: u8,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl TileCell {
    /// A cell referencing `tile_id` with an identity transform.
    pub fn new(tile_id: TileId) -> Self {
        Self {
            tile_id,
            rotation: 0,
            flip_x: false,
            flip_y: false,
        }
    }

    /// Sets the clockwise 90° step count, normalizing it to `0..=3`.
    pub fn set_rotation(&mut self, steps: u8) {
        self.rotation = normalize_rotation(steps);
    }
}

/// A grid of tile cells over a layer's canvas.
///
/// `cells` is row-major with `len == cols * rows`; the canvas origin of cell
/// `(x, y)` is `(x * tile_size, y * tile_size)`. Out-of-bounds writes are
/// no-ops; every operation is deterministic and panic-free.
///
/// `change_epoch` (bumped on every `set_cell`) and the accumulated
/// `dirty_cells` bounding rect are RUNTIME change tracking (`#[serde(skip)]`,
/// never persisted) so render caches and the texture-sync dirty union can
/// detect edits cheaply. They are excluded from `PartialEq`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TileMap {
    pub tile_size: u32,
    pub cols: u32,
    pub rows: u32,
    /// Row-major cell list, private so the `len == cols * rows` invariant is
    /// only reachable through the methods.
    cells: Vec<Option<TileCell>>,
    /// Runtime change counter (every `set_cell`); not persisted.
    #[serde(skip)]
    pub change_epoch: u64,
    /// Accumulated bounding rect of changed cells `(x, y, w, h)` in CELL
    /// space, consumed by [`Self::take_dirty_canvas_region`]; not persisted.
    #[serde(skip)]
    dirty_cells: Option<(u32, u32, u32, u32)>,
}

impl PartialEq for TileMap {
    fn eq(&self, other: &Self) -> bool {
        self.tile_size == other.tile_size
            && self.cols == other.cols
            && self.rows == other.rows
            && self.cells == other.cells
    }
}

impl TileMap {
    /// An empty (all cells `None`) map over `cols × rows` cells.
    pub fn new(tile_size: u32, cols: u32, rows: u32) -> Self {
        let tile_size = tile_size.max(1);
        let len = (cols as usize).checked_mul(rows as usize).unwrap_or(0);
        Self {
            tile_size,
            cols,
            rows,
            cells: vec![None; len],
            change_epoch: 0,
            dirty_cells: None,
        }
    }

    /// Grows (or shrinks) the map to `cols × rows` cells, preserving every
    /// existing cell that still fits. A no-op when the dimensions already
    /// match. Bumps `change_epoch` and marks the full affected canvas area
    /// dirty when the shape actually changes.
    ///
    /// The tile placer calls this so an existing tilemap (created earlier at a
    /// different `tile_size`, or before a canvas resize) never silently drops
    /// stamps outside its old bounds — a click there must still place a tile.
    pub fn resize(&mut self, cols: u32, rows: u32) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if self.cols == cols && self.rows == rows {
            return;
        }
        let mut cells = vec![None; (cols as usize).checked_mul(rows as usize).unwrap_or(0)];
        for y in 0..self.rows.min(rows) {
            for x in 0..self.cols.min(cols) {
                let old = (y as usize) * (self.cols as usize) + (x as usize);
                let new = (y as usize) * (cols as usize) + (x as usize);
                cells[new] = self.cells[old];
            }
        }
        self.cols = cols;
        self.rows = rows;
        self.cells = cells;
        self.change_epoch = self.change_epoch.wrapping_add(1);
        // The whole new area may have changed (old cells moved, new rows added):
        // mark the full region dirty so the canvas re-rasters.
        self.dirty_cells = Some((0, 0, cols, rows));
    }

    /// Snaps a canvas point to its grid-cell index (Euclidean floor division).
    pub fn snap_cell(p: (i32, i32), ts: u32) -> (u32, u32) {
        let ts = ts.max(1) as i32;
        (p.0.div_euclid(ts) as u32, p.1.div_euclid(ts) as u32)
    }

    /// The flat index of `(x, y)`, or `None` when out of bounds.
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.cols || y >= self.rows {
            return None;
        }
        Some(
            (y as usize)
                .checked_mul(self.cols as usize)?
                .checked_add(x as usize)?,
        )
    }

    /// Sets (or clears) the cell at `(x, y)`; a no-op when out of bounds.
    /// The cell's rotation is normalized. Bumps `change_epoch` and accumulates
    /// the changed cell into the dirty rect.
    pub fn set_cell(&mut self, c: (u32, u32), cell: Option<TileCell>) {
        if let Some(index) = self.index(c.0, c.1) {
            self.cells[index] = cell.map(|mut cell| {
                cell.rotation = normalize_rotation(cell.rotation);
                cell
            });
            self.change_epoch = self.change_epoch.wrapping_add(1);
            self.dirty_cells = Some(match self.dirty_cells {
                Some((x, y, w, h)) => {
                    let x0 = x.min(c.0);
                    let y0 = y.min(c.1);
                    let x1 = (x + w).max(c.0 + 1);
                    let y1 = (y + h).max(c.1 + 1);
                    (x0, y0, x1 - x0, y1 - y0)
                }
                None => (c.0, c.1, 1, 1),
            });
        }
    }

    /// The cell at `(x, y)`, or `None` when empty or out of bounds.
    pub fn cell(&self, c: (u32, u32)) -> Option<TileCell> {
        self.index(c.0, c.1).and_then(|index| self.cells[index])
    }

    /// The full row-major cell list (`len == cols * rows`).
    pub fn cells(&self) -> &[Option<TileCell>] {
        &self.cells
    }

    /// Consumes and returns the changed-cells region as a CANVAS-space rect
    /// (`dirty cell bbox × tile_size`, clipped to `canvas_w × canvas_h`), or
    /// `None` when nothing changed.
    pub fn take_dirty_canvas_region(&mut self, canvas_w: u32, canvas_h: u32) -> Option<Rect2i> {
        let (cx, cy, cw, ch) = self.dirty_cells.take()?;
        let ts = u64::from(self.tile_size.max(1));
        let x = ((u64::from(cx) * ts).min(u64::from(canvas_w))) as i32;
        let y = ((u64::from(cy) * ts).min(u64::from(canvas_h))) as i32;
        let right = ((u64::from(cx + cw) * ts).min(u64::from(canvas_w))) as i32;
        let bottom = ((u64::from(cy + ch) * ts).min(u64::from(canvas_h))) as i32;
        if right <= x || bottom <= y {
            return None;
        }
        Some(Rect2i::new(x, y, right - x, bottom - y))
    }

    /// The full canvas area this map covers (`cols × rows × tile_size`),
    /// clipped to the canvas — used for palette-driven re-uploads.
    pub fn full_canvas_region(&self, canvas_w: u32, canvas_h: u32) -> Rect2i {
        let w = ((u64::from(self.cols) * u64::from(self.tile_size.max(1))).min(u64::from(canvas_w)))
            as i32;
        let h = ((u64::from(self.rows) * u64::from(self.tile_size.max(1))).min(u64::from(canvas_h)))
            as i32;
        Rect2i::new(0, 0, w, h)
    }
}

/// Orients raw straight-RGBA8 pixels: horizontal then vertical mirror, then
/// `rotation` clockwise 90° steps (the same order as the original tile math).
fn orient_tile_pixels(
    bytes: &[u8],
    w: usize,
    h: usize,
    flip_x: bool,
    flip_y: bool,
    rotation: u8,
) -> Vec<u8> {
    let mut current = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let sx = if flip_x { w - 1 - x } else { x };
            let sy = if flip_y { h - 1 - y } else { y };
            let src = (sy * w + sx) * 4;
            let dst = (y * w + x) * 4;
            current[dst..dst + 4].copy_from_slice(&bytes[src..src + 4]);
        }
    }
    let mut cur_w = w;
    let mut cur_h = h;
    for _ in 0..(rotation % 4) {
        let new_w = cur_h;
        let new_h = cur_w;
        let mut next = vec![0u8; current.len()];
        for y in 0..cur_h {
            for x in 0..cur_w {
                let src = (y * cur_w + x) * 4;
                // 90° clockwise: (x, y) -> (new_w - 1 - y, x).
                let nx = new_w - 1 - y;
                let ny = x;
                let dst = (ny * new_w + nx) * 4;
                next[dst..dst + 4].copy_from_slice(&current[src..src + 4]);
            }
        }
        current = next;
        cur_w = new_w;
        cur_h = new_h;
    }
    current
}

impl TileMap {
    /// Rasterizes every occupied cell into `dst`, overwriting its pixels
    /// verbatim (palette-pure — no AA, no blending).
    ///
    /// For each cell with a [`TileCell`]: the referenced tile is looked up in
    /// `pal` (missing tiles are SKIPPED), oriented by the cell's flips then
    /// rotation (90° CW steps), and blitted at `cell * tile_size`, clipped to
    /// `dst`'s bounds. Deterministic; never panics.
    pub fn rasterize(&self, pal: &TilePalette, dst: &mut PixelBuffer) {
        self.rasterize_internal(pal, None, dst);
    }

    /// Rasterizes tile pixels with transient root-pixel replacements.
    ///
    /// Overrides are applied before each cell's flips and clockwise rotation,
    /// so every instance of a shared root tile previews correctly. This path
    /// does not consult or modify any render cache. An empty override set uses
    /// the ordinary rasterizer exactly.
    pub fn rasterize_with_overrides(
        &self,
        pal: &TilePalette,
        overrides: &TilePixelOverrides,
        dst: &mut PixelBuffer,
    ) {
        if overrides.is_empty() {
            self.rasterize(pal, dst);
        } else {
            self.rasterize_internal(pal, Some(overrides), dst);
        }
    }

    fn rasterize_internal(
        &self,
        pal: &TilePalette,
        overrides: Option<&TilePixelOverrides>,
        dst: &mut PixelBuffer,
    ) {
        let canvas_w = dst.width() as u32;
        let canvas_h = dst.height() as u32;
        let cols = self.cols.max(1) as usize;
        let tile_size = self.tile_size.max(1);
        for (index, cell) in self.cells.iter().enumerate() {
            let Some(cell) = cell else {
                continue;
            };
            let Some(tile) = pal.get(cell.tile_id) else {
                continue;
            };
            let rotated = cell.rotation % 2 == 1;
            let (ow, oh) = if rotated {
                (u32::from(tile.h), u32::from(tile.w))
            } else {
                (u32::from(tile.w), u32::from(tile.h))
            };
            let (cx, cy) = ((index % cols) as u32, (index / cols) as u32);
            let Some(ox) = cx.checked_mul(tile_size) else {
                continue;
            };
            let Some(oy) = cy.checked_mul(tile_size) else {
                continue;
            };
            if ox >= canvas_w || oy >= canvas_h {
                continue;
            }
            let mut preview_pixels;
            let pixels = if let Some(overrides) = overrides.filter(|o| o.contains_tile(cell.tile_id)) {
                preview_pixels = tile.pixels.clone();
                for y in 0..u32::from(tile.h) {
                    for x in 0..u32::from(tile.w) {
                        if let Some(rgba) = overrides.get(cell.tile_id, x, y) {
                            let offset = ((y as usize * tile.w as usize) + x as usize) * 4;
                            if offset + 4 <= preview_pixels.len() {
                                preview_pixels[offset..offset + 4].copy_from_slice(&rgba);
                            }
                        }
                    }
                }
                &preview_pixels
            } else {
                &tile.pixels
            };
            let oriented = orient_tile_pixels(
                pixels,
                tile.w as usize,
                tile.h as usize,
                cell.flip_x,
                cell.flip_y,
                cell.rotation,
            );
            for row in 0..oh as usize {
                let dy = oy + row as u32;
                if dy >= canvas_h {
                    continue;
                }
                for col in 0..ow as usize {
                    let dx = ox + col as u32;
                    if dx >= canvas_w {
                        continue;
                    }
                    let src = (row * ow as usize + col) * 4;
                    let dst_offset = ((dy as usize) * (canvas_w as usize) + dx as usize) * 4;
                    dst.as_bytes_mut()[dst_offset..dst_offset + 4]
                        .copy_from_slice(&oriented[src..src + 4]);
                }
            }
        }
    }

    /// Replaces just the canvas pixels supplied by root overrides. Used after
    /// the ordinary tilemap-over-buffer composition so transparent override
    /// bytes replace stale baked copies rather than alpha-compositing over
    /// them.
    pub(crate) fn apply_pixel_overrides(
        &self,
        pal: &TilePalette,
        overrides: &TilePixelOverrides,
        dst: &mut PixelBuffer,
    ) {
        let canvas_w = dst.width() as u32;
        let canvas_h = dst.height() as u32;
        let cols = self.cols.max(1) as usize;
        let tile_size = self.tile_size.max(1);
        for (index, cell) in self.cells.iter().enumerate() {
            let Some(cell) = cell else { continue };
            let Some(tile) = pal.get(cell.tile_id) else { continue };
            let Some(pixels) = overrides.pixels.get(&cell.tile_id) else {
                continue;
            };
            let (cx, cy) = ((index % cols) as u32, (index / cols) as u32);
            let Some(ox) = cx.checked_mul(tile_size) else { continue };
            let Some(oy) = cy.checked_mul(tile_size) else { continue };
            for (&(root_y, root_x), &rgba) in pixels {
                if root_x >= u32::from(tile.w) || root_y >= u32::from(tile.h) {
                    continue;
                }
                let mut x = if cell.flip_x {
                    u32::from(tile.w) - 1 - root_x
                } else {
                    root_x
                };
                let mut y = if cell.flip_y {
                    u32::from(tile.h) - 1 - root_y
                } else {
                    root_y
                };
                let mut width = u32::from(tile.w);
                let mut height = u32::from(tile.h);
                for _ in 0..(cell.rotation % 4) {
                    let next_x = height - 1 - y;
                    let next_y = x;
                    x = next_x;
                    y = next_y;
                    std::mem::swap(&mut width, &mut height);
                }
                let dx = ox + x;
                let dy = oy + y;
                if dx < canvas_w && dy < canvas_h {
                    let offset = ((dy as usize * canvas_w as usize) + dx as usize) * 4;
                    dst.as_bytes_mut()[offset..offset + 4].copy_from_slice(&rgba);
                }
            }
        }
    }

    /// Blits ONE occupied cell's oriented tile into `dst` at its footprint,
    /// clipped to the buffer. Returns `true` when the cell was occupied and its
    /// tile exists; `false` for an empty cell or a missing tile (a no-op).
    ///
    /// This is the single bake primitive of the integrated tile model: the
    /// layer buffer's tiled-cell footprint holds EXACTLY the oriented root-tile
    /// bytes. `rasterize` stays the whole-map render path; `blit_cell` is the
    /// per-cell write-back used by the placer, the write-back re-stamp and the
    /// load migration.
    pub fn blit_cell(&self, pal: &TilePalette, dst: &mut PixelBuffer, cx: u32, cy: u32) -> bool {
        let Some(cell) = self.cell((cx, cy)) else {
            return false;
        };
        let Some(tile) = pal.get(cell.tile_id) else {
            return false;
        };
        let canvas_w = dst.width() as u32;
        let canvas_h = dst.height() as u32;
        let tile_size = self.tile_size.max(1);
        let rotated = cell.rotation % 2 == 1;
        let (ow, oh) = if rotated {
            (u32::from(tile.h), u32::from(tile.w))
        } else {
            (u32::from(tile.w), u32::from(tile.h))
        };
        let Some(ox) = cx.checked_mul(tile_size) else {
            return false;
        };
        let Some(oy) = cy.checked_mul(tile_size) else {
            return false;
        };
        if ox >= canvas_w || oy >= canvas_h {
            return true;
        }
        let oriented = orient_tile_pixels(
            &tile.pixels,
            tile.w as usize,
            tile.h as usize,
            cell.flip_x,
            cell.flip_y,
            cell.rotation,
        );
        for row in 0..oh as usize {
            let dy = oy + row as u32;
            if dy >= canvas_h {
                continue;
            }
            for col in 0..ow as usize {
                let dx = ox + col as u32;
                if dx >= canvas_w {
                    continue;
                }
                let src = (row * ow as usize + col) * 4;
                let dst_offset = ((dy as usize) * (canvas_w as usize) + dx as usize) * 4;
                dst.as_bytes_mut()[dst_offset..dst_offset + 4]
                    .copy_from_slice(&oriented[src..src + 4]);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(id: u64, w: u16, h: u16) -> Tile {
        Tile {
            id: TileId(id),
            w,
            h,
            pixels: vec![0u8; w as usize * h as usize * 4],
        }
    }

    #[test]
    fn empty_palette_defaults() {
        let palette = TilePalette::new();
        assert_eq!(palette, TilePalette::default());
        assert!(palette.is_empty());
        assert_eq!(palette.selected, None);
        assert!(palette.selected_tile().is_none());
        assert_eq!(palette.next_id(), 1);
    }

    #[test]
    fn palette_add_assigns_deterministic_ids() {
        let mut palette = TilePalette::new();
        assert_eq!(palette.add(tile(0, 1, 1)), TileId(1));
        assert_eq!(palette.add(tile(0, 1, 1)), TileId(2));
        assert_eq!(palette.add(tile(0, 1, 1)), TileId(3));
        assert_eq!(palette.next_id(), 4);
        // Insertion order is preserved.
        assert_eq!(
            palette.tiles.iter().map(|t| t.id).collect::<Vec<_>>(),
            vec![TileId(1), TileId(2), TileId(3)]
        );
    }

    #[test]
    fn palette_add_keeps_explicit_unique_id() {
        let mut palette = TilePalette::new();
        assert_eq!(palette.add(tile(42, 1, 1)), TileId(42));
        assert_eq!(palette.next_id(), 43);
    }

    #[test]
    fn palette_add_reassigns_duplicate_id() {
        let mut palette = TilePalette::new();
        assert_eq!(palette.add(tile(5, 1, 1)), TileId(5));
        assert_eq!(palette.add(tile(5, 1, 1)), TileId(6));
    }

    #[test]
    fn palette_next_id_reuses_highest_after_removal() {
        let mut palette = TilePalette::new();
        palette.add(tile(0, 1, 1));
        palette.add(tile(0, 1, 1));
        assert!(palette.remove(TileId(2)));
        assert_eq!(palette.next_id(), 2);
        assert_eq!(palette.add(tile(0, 1, 1)), TileId(2));
    }

    #[test]
    fn palette_select_and_remove_clear_selection() {
        let mut palette = TilePalette::new();
        let id = palette.add(tile(0, 1, 1));
        assert!(!palette.select(TileId(999)));
        assert!(palette.select(id));
        assert_eq!(palette.selected_tile().map(|t| t.id), Some(id));
        assert!(palette.remove(id));
        assert_eq!(palette.selected, None);
        assert!(palette.selected_tile().is_none());
        assert!(!palette.remove(id));
        assert!(palette.is_empty());
        // Removing a different tile keeps the selection.
        let a = palette.add(tile(0, 1, 1));
        let b = palette.add(tile(0, 1, 1));
        assert!(palette.select(b));
        assert!(palette.remove(a));
        assert_eq!(palette.selected, Some(b));
    }

    #[test]
    fn palette_get_and_serde_roundtrip() {
        let mut palette = TilePalette::new();
        let a = palette.add(tile(0, 2, 3));
        let b = palette.add(tile(0, 4, 4));
        assert!(palette.select(b));
        assert_eq!(palette.get(a).map(|t| t.id), Some(a));
        assert_eq!(palette.get(TileId(999)), None);
        let json = serde_json::to_string(&palette).unwrap();
        let restored: TilePalette = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, palette);
        // JSON shape: tiles + selected (no stored next_id).
        let value = serde_json::to_value(&palette).unwrap();
        assert_eq!(value["tiles"][0]["id"], serde_json::json!(a.0));
        assert_eq!(value["tiles"][0]["w"], serde_json::json!(2));
        assert_eq!(value["tiles"][0]["h"], serde_json::json!(3));
        assert_eq!(value["selected"], serde_json::json!(b.0));
    }

    #[test]
    fn tile_map_new_and_out_of_bounds_cell_access() {
        let map = TileMap::new(4, 3, 2);
        assert_eq!(map.cols, 3);
        assert_eq!(map.rows, 2);
        assert_eq!(map.cells().len(), 6);
        assert_eq!(map.cell((0, 0)), None);
        assert_eq!(map.cell((2, 1)), None);
        // Out of bounds reads and writes are no-ops.
        assert_eq!(map.cell((3, 0)), None);
        assert_eq!(map.cell((0, 2)), None);
        let mut map = map;
        map.set_cell((3, 0), Some(TileCell::new(TileId(1))));
        map.set_cell((0, 2), Some(TileCell::new(TileId(1))));
        assert!(map.cells().iter().all(|c| c.is_none()));
    }

    #[test]
    fn tile_map_set_cell_and_cell_round_trip() {
        let mut map = TileMap::new(4, 3, 2);
        map.set_cell((1, 0), Some(TileCell::new(TileId(7))));
        map.set_cell((2, 1), None);
        assert_eq!(map.cell((1, 0)), Some(TileCell::new(TileId(7))));
        assert_eq!(map.cell((2, 1)), None);
        map.set_cell((1, 0), None);
        assert_eq!(map.cell((1, 0)), None);
    }

    #[test]
    fn tile_cell_rotation_is_normalized() {
        let mut cell = TileCell::new(TileId(1));
        cell.set_rotation(7);
        assert_eq!(cell.rotation, 3);
        cell.set_rotation(4);
        assert_eq!(cell.rotation, 0);
        // Deserialization normalizes too.
        let json = serde_json::json!({
            "tile_id": 1,
            "rotation": 7,
            "flip_x": false,
            "flip_y": false
        });
        let parsed: TileCell = serde_json::from_value(json).unwrap();
        assert_eq!(parsed.rotation, 3);
    }

    #[test]
    fn snap_cell_floors_with_euclidean_division() {
        assert_eq!(TileMap::snap_cell((5, 3), 4), (1, 0));
        assert_eq!(TileMap::snap_cell((-1, 16), 4), (u32::MAX, 4));
        assert_eq!(TileMap::snap_cell((0, 0), 0), (0, 0));
    }

    fn solid_tile(id: u64, w: u16, h: u16, color: [u8; 4]) -> Tile {
        Tile {
            id: TileId(id),
            w,
            h,
            pixels: color.repeat(w as usize * h as usize),
        }
    }

    #[test]
    fn rasterize_identity_places_verbatim_pixels() {
        let mut palette = TilePalette::new();
        let red = [255u8, 0, 0, 255];
        palette.add(solid_tile(0, 2, 2, red));
        let mut map = TileMap::new(4, 4, 4);
        map.set_cell((1, 1), Some(TileCell::new(TileId(1))));
        let mut dst = PixelBuffer::new(16, 16);
        map.rasterize(&palette, &mut dst);
        // Cell (1,1) at (4,4): a 2x2 red block.
        assert_eq!(
            dst.get_pixel(4, 4),
            Some(crate::core::color::Color::rgba(255, 0, 0, 255))
        );
        assert_eq!(
            dst.get_pixel(5, 4),
            Some(crate::core::color::Color::rgba(255, 0, 0, 255))
        );
        assert_eq!(
            dst.get_pixel(4, 5),
            Some(crate::core::color::Color::rgba(255, 0, 0, 255))
        );
        assert_eq!(
            dst.get_pixel(3, 3),
            Some(crate::core::color::Color::TRANSPARENT)
        );
        assert_eq!(
            dst.get_pixel(6, 4),
            Some(crate::core::color::Color::TRANSPARENT)
        );
    }

    #[test]
    fn rasterize_rotation_and_flips() {
        use crate::core::color::Color;
        // 2x1 tile: [red, green].
        let red = [255u8, 0, 0, 255];
        let green = [0u8, 255, 0, 255];
        let mut palette = TilePalette::new();
        palette.add(Tile {
            id: TileId(1),
            w: 2,
            h: 1,
            pixels: [red, green].concat(),
        });
        let mut map = TileMap::new(4, 4, 4);

        // Rotation 90° CW: the 2x1 becomes a 1x2 column [red; green].
        map.set_cell(
            (0, 0),
            Some(TileCell {
                tile_id: TileId(1),
                rotation: 1,
                flip_x: false,
                flip_y: false,
            }),
        );
        // Flip X: [green, red].
        map.set_cell(
            (1, 0),
            Some(TileCell {
                tile_id: TileId(1),
                rotation: 0,
                flip_x: true,
                flip_y: false,
            }),
        );
        // Rotation 180 + flip Y: dims stay 2x1, row flips too: [red, green] -> flip_y [red, green] (h=1, no change).
        map.set_cell(
            (2, 0),
            Some(TileCell {
                tile_id: TileId(1),
                rotation: 2,
                flip_x: false,
                flip_y: true,
            }),
        );

        let mut dst = PixelBuffer::new(16, 16);
        map.rasterize(&palette, &mut dst);
        // (0,0) rot 90: column at (0,0): (0,0)=red, (0,1)=green.
        assert_eq!(dst.get_pixel(0, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(dst.get_pixel(0, 1), Some(Color::rgba(0, 255, 0, 255)));
        // (1,0) flip_x: (4,0)=green, (5,0)=red.
        assert_eq!(dst.get_pixel(4, 0), Some(Color::rgba(0, 255, 0, 255)));
        assert_eq!(dst.get_pixel(5, 0), Some(Color::rgba(255, 0, 0, 255)));
        // (2,0) rotation 180: [red,green] rotated 180 -> [green,red].
        assert_eq!(dst.get_pixel(8, 0), Some(Color::rgba(0, 255, 0, 255)));
        assert_eq!(dst.get_pixel(9, 0), Some(Color::rgba(255, 0, 0, 255)));
    }

    #[test]
    fn rasterize_with_overrides_is_empty_identical_and_transforms_root_pixels() {
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
        let mut map = TileMap::new(4, 3, 1);
        map.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        let mut rotated = TileCell::new(TileId(1));
        rotated.set_rotation(1);
        map.set_cell((1, 0), Some(rotated));
        map.set_cell(
            (2, 0),
            Some(TileCell {
                tile_id: TileId(1),
                rotation: 0,
                flip_x: true,
                flip_y: false,
            }),
        );

        let empty = TilePixelOverrides::default();
        let mut ordinary = PixelBuffer::new(12, 4);
        let mut empty_override = PixelBuffer::new(12, 4);
        map.rasterize(&palette, &mut ordinary);
        map.rasterize_with_overrides(&palette, &empty, &mut empty_override);
        assert_eq!(ordinary.as_bytes(), empty_override.as_bytes());

        let mut overrides = TilePixelOverrides::default();
        overrides.insert(TileId(1), 0, 0, blue);
        let mut preview = PixelBuffer::new(12, 4);
        map.rasterize_with_overrides(&palette, &overrides, &mut preview);
        // The overridden root pixel (0,0) is transformed into every instance:
        // identity cell at (0,0), 90°-rotated cell at (4,0), x-flipped at (9,0).
        assert_eq!(preview.get_pixel(0, 0), Some(crate::core::color::Color::rgba(0, 0, 255, 255)));
        assert_eq!(preview.get_pixel(4, 0), Some(crate::core::color::Color::rgba(0, 0, 255, 255)));
        assert_eq!(preview.get_pixel(9, 0), Some(crate::core::color::Color::rgba(0, 0, 255, 255)));
        // The non-overridden green root pixel (1,0) lands at identity (1,0),
        // rotated (4,1) (the 2x1 tile becomes a 1x2 column), and flipped (8,0).
        assert_eq!(preview.get_pixel(1, 0), Some(crate::core::color::Color::rgba(0, 255, 0, 255)));
        assert_eq!(preview.get_pixel(4, 1), Some(crate::core::color::Color::rgba(0, 255, 0, 255)));
        assert_eq!(preview.get_pixel(8, 0), Some(crate::core::color::Color::rgba(0, 255, 0, 255)));
        // The rotated cell's footprint is only 1 px wide, so (5,0) belongs to
        // no cell and stays transparent — exactly as the ordinary rasterizer
        // leaves it (the empty-override path is byte-identical, asserted above).
        assert_eq!(preview.get_pixel(5, 0), Some(crate::core::color::Color::TRANSPARENT));
        assert_eq!(ordinary.get_pixel(5, 0), Some(crate::core::color::Color::TRANSPARENT));
        assert_eq!(overrides.tile_ids().collect::<Vec<_>>(), vec![TileId(1)]);
    }

    #[test]
    fn rasterize_skips_missing_tiles_and_clips_off_canvas() {
        use crate::core::color::Color;
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 2, 2, [255, 0, 0, 255]));
        let mut map = TileMap::new(4, 4, 4);
        // A cell referencing a missing tile is skipped (empty).
        map.set_cell((0, 0), Some(TileCell::new(TileId(99))));
        // A cell that starts off-canvas is skipped entirely.
        map.set_cell((3, 0), Some(TileCell::new(TileId(1)))); // origin (12,0), 2x2 fits
        map.set_cell((0, 3), Some(TileCell::new(TileId(1)))); // origin (0,12), fits
        let mut dst = PixelBuffer::new(14, 14);
        map.rasterize(&palette, &mut dst);
        assert_eq!(
            dst.get_pixel(0, 0),
            Some(Color::TRANSPARENT),
            "missing tile skipped"
        );
        // Cell (3,0) at (12,0) is clipped at the 14-wide canvas: (13,0) painted,
        // (14,0) out of bounds.
        assert_eq!(dst.get_pixel(12, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(dst.get_pixel(13, 0), Some(Color::rgba(255, 0, 0, 255)));
        assert_eq!(dst.get_pixel(0, 12), Some(Color::rgba(255, 0, 0, 255)));

        // A fully off-canvas cell (origin beyond 14x14) paints nothing.
        let mut map = TileMap::new(8, 4, 4);
        map.set_cell((3, 3), Some(TileCell::new(TileId(1)))); // origin (24,24)
        let mut dst = PixelBuffer::new(14, 14);
        map.rasterize(&palette, &mut dst);
        assert!(dst.as_bytes().iter().all(|b| *b == 0));
    }

    #[test]
    fn rasterize_is_deterministic_and_palette_pure() {
        let mut palette = TilePalette::new();
        palette.add(solid_tile(0, 3, 2, [10, 20, 30, 255]));
        let mut map = TileMap::new(4, 3, 3);
        map.set_cell((0, 0), Some(TileCell::new(TileId(1))));
        map.set_cell(
            (2, 2),
            Some(TileCell {
                tile_id: TileId(1),
                rotation: 1,
                flip_x: true,
                flip_y: false,
            }),
        );
        let mut a = PixelBuffer::new(12, 12);
        let mut b = PixelBuffer::new(12, 12);
        map.rasterize(&palette, &mut a);
        map.rasterize(&palette, &mut b);
        assert_eq!(a.as_bytes(), b.as_bytes(), "rasterize is deterministic");
        // Verbatim: every non-zero pixel is exactly the palette colour.
        for (i, px) in a.as_bytes().chunks_exact(4).enumerate() {
            if px[3] != 0 {
                assert_eq!(px, &[10, 20, 30, 255], "verbatim palette pixel {i}");
            }
        }
    }

    #[test]
    fn tile_map_serde_round_trip() {
        let mut map = TileMap::new(16, 2, 2);
        map.set_cell(
            (1, 0),
            Some(TileCell {
                tile_id: TileId(3),
                rotation: 2,
                flip_x: true,
                flip_y: false,
            }),
        );
        let json = serde_json::to_string(&map).unwrap();
        let restored: TileMap = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, map);
        assert_eq!(restored.cell((1, 0)), map.cell((1, 0)));
    }
}
