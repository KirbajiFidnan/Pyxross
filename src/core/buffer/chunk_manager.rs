//! Chunked canvas storage — 256×256 pixel pages with dirty-rect tracking.

use std::collections::HashMap;

use crate::core::buffer::PixelBuffer;
use crate::core::math::Rect2i;

/// Side length of each square chunk in pixels.
pub const CHUNK_SIZE: usize = 256;

/// Manages a canvas as a grid of [`PixelBuffer`] pages, each at most
/// `CHUNK_SIZE × CHUNK_SIZE` pixels.  Chunks are lazily allocated — a `None`
/// slot means nothing has been drawn there yet.  Dirty-rect tracking records
/// which regions need re-rendering; the renderer drains them once per frame
/// via [`iter_dirty_chunks`](Self::iter_dirty_chunks).
pub struct ChunkManager {
    canvas_width: usize,
    canvas_height: usize,
    chunks_per_row: usize,
    chunks_per_col: usize,
    chunks: HashMap<usize, PixelBuffer>,
    dirty: HashMap<usize, Rect2i>,
}

impl ChunkManager {
    /// Creates a new chunk manager for the given canvas dimensions.
    pub fn new(canvas_width: usize, canvas_height: usize) -> Self {
        let cpr = canvas_width.div_ceil(CHUNK_SIZE);
        let cpc = canvas_height.div_ceil(CHUNK_SIZE);
        Self {
            canvas_width,
            canvas_height,
            chunks_per_row: cpr,
            chunks_per_col: cpc,
            chunks: HashMap::new(),
            dirty: HashMap::new(),
        }
    }

    /// Width of the canvas in pixels.
    pub fn canvas_width(&self) -> usize {
        self.canvas_width
    }

    /// Height of the canvas in pixels.
    pub fn canvas_height(&self) -> usize {
        self.canvas_height
    }

    /// Number of chunks that fit along one row.
    pub fn chunks_per_row(&self) -> usize {
        self.chunks_per_row
    }

    /// Total number of chunks (rows × columns).
    pub fn chunk_count(&self) -> usize {
        self.try_chunk_count().unwrap_or(usize::MAX)
    }

    pub fn try_chunk_count(&self) -> Option<usize> {
        self.chunks_per_row.checked_mul(self.chunks_per_col)
    }

    /// Row-major index for chunk grid position `(cx, cy)`.
    pub fn chunk_idx(&self, cx: usize, cy: usize) -> usize {
        cy.saturating_mul(self.chunks_per_row).saturating_add(cx)
    }

    /// Returns the chunk that contains canvas coordinate `(x, y)`, or `None`
    /// if the coordinate is out of bounds or the chunk has not been allocated.
    pub fn chunk_at(&self, x: usize, y: usize) -> Option<&PixelBuffer> {
        if x >= self.canvas_width || y >= self.canvas_height {
            return None;
        }
        let cx = x / CHUNK_SIZE;
        let cy = y / CHUNK_SIZE;
        let idx = self.chunk_idx(cx, cy);
        self.chunks.get(&idx)
    }

    /// Mutable variant of [`chunk_at`](Self::chunk_at).  Allocates the chunk
    /// on first access with the correct edge-clipped dimensions.
    pub fn chunk_at_mut(&mut self, x: usize, y: usize) -> Option<&mut PixelBuffer> {
        if x >= self.canvas_width || y >= self.canvas_height {
            return None;
        }
        let cx = x / CHUNK_SIZE;
        let cy = y / CHUNK_SIZE;
        let idx = self.chunk_idx(cx, cy);
        self.materialize(idx, cx, cy);
        self.chunks.get_mut(&idx)
    }

    /// Returns the chunk at grid index `idx`, or `None` if unallocated.
    pub fn get_chunk(&self, idx: usize) -> Option<&PixelBuffer> {
        if idx >= self.try_chunk_count()? {
            return None;
        }
        self.chunks.get(&idx)
    }

    /// Mutable variant of [`get_chunk`](Self::get_chunk).  Allocates on first
    /// access.
    pub fn get_chunk_mut(&mut self, idx: usize) -> Option<&mut PixelBuffer> {
        if idx >= self.try_chunk_count()? {
            return None;
        }
        let cy = idx / self.chunks_per_row;
        let cx = idx % self.chunks_per_row;
        self.materialize(idx, cx, cy);
        self.chunks.get_mut(&idx)
    }

    /// Marks `rect` (in canvas coordinates) as dirty.  The rect is clipped to
    /// the canvas bounds, then each affected chunk receives the intersection
    /// unioned with any previously recorded dirty rect.
    pub fn mark_dirty(&mut self, rect: Rect2i) {
        let canvas = Rect2i::new(
            0,
            0,
            self.canvas_width.min(i32::MAX as usize) as i32,
            self.canvas_height.min(i32::MAX as usize) as i32,
        );
        let clipped = rect.intersection(canvas);
        if clipped.is_empty() {
            return;
        }

        let cx_min = (clipped.x as usize) / CHUNK_SIZE;
        let cy_min = (clipped.y as usize) / CHUNK_SIZE;
        let Some(cx_max) = self.chunks_per_row.checked_sub(1) else {
            return;
        };
        let Some(cy_max) = self.chunks_per_col.checked_sub(1) else {
            return;
        };
        let cx_max = ((clipped.right() as usize - 1) / CHUNK_SIZE).min(cx_max);
        let cy_max = ((clipped.bottom() as usize - 1) / CHUNK_SIZE).min(cy_max);

        for cy in cy_min..=cy_max {
            for cx in cx_min..=cx_max {
                let idx = self.chunk_idx(cx, cy);
                let chunk_rect = self.chunk_rect_raw(cx, cy);
                let intersection = clipped.intersection(chunk_rect);
                if intersection.is_empty() {
                    continue;
                }
                self.dirty
                    .entry(idx)
                    .and_modify(|existing| *existing = existing.union(intersection))
                    .or_insert(intersection);
            }
        }
    }

    /// Yields `(chunk_idx, dirty_rect_in_canvas_coords)` for every chunk that
    /// was marked dirty, then **clears** the dirty state.  The renderer should
    /// call this once per frame.
    pub fn iter_dirty_chunks(&mut self) -> impl Iterator<Item = (usize, Rect2i)> {
        let mut dirty: Vec<_> = std::mem::take(&mut self.dirty).into_iter().collect();
        dirty.sort_unstable_by_key(|(idx, _)| *idx);
        dirty.into_iter()
    }

    /// Explicitly clears all dirty state without iterating.
    pub fn clear_dirty(&mut self) {
        self.dirty.clear();
    }

    /// Canvas-space rectangle covered by chunk `idx`.  Edge chunks may be
    /// smaller than `CHUNK_SIZE × CHUNK_SIZE`.
    pub fn chunk_rect(&self, idx: usize) -> Option<Rect2i> {
        if idx >= self.try_chunk_count()? {
            return None;
        }
        let cy = idx / self.chunks_per_row;
        let cx = idx % self.chunks_per_row;
        Some(self.chunk_rect_raw(cx, cy))
    }

    // ── Private helpers ────────────────────────────────────────────────

    fn materialize(&mut self, idx: usize, cx: usize, cy: usize) {
        if !self.chunks.contains_key(&idx) {
            let w = self.chunk_width(cx);
            let h = self.chunk_height(cy);
            self.chunks.insert(idx, PixelBuffer::new(w, h));
        }
    }

    fn chunk_width(&self, cx: usize) -> usize {
        let start = cx.saturating_mul(CHUNK_SIZE);
        self.canvas_width.saturating_sub(start).min(CHUNK_SIZE)
    }

    fn chunk_height(&self, cy: usize) -> usize {
        let start = cy.saturating_mul(CHUNK_SIZE);
        self.canvas_height.saturating_sub(start).min(CHUNK_SIZE)
    }

    fn chunk_rect_raw(&self, cx: usize, cy: usize) -> Rect2i {
        Rect2i::new(
            cx.saturating_mul(CHUNK_SIZE).min(i32::MAX as usize) as i32,
            cy.saturating_mul(CHUNK_SIZE).min(i32::MAX as usize) as i32,
            self.chunk_width(cx).min(i32::MAX as usize) as i32,
            self.chunk_height(cy).min(i32::MAX as usize) as i32,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basic properties ──────────────────────────────────────────────

    #[test]
    fn canvas_dimensions_stored() {
        let mgr = ChunkManager::new(500, 300);
        assert_eq!(mgr.canvas_width(), 500);
        assert_eq!(mgr.canvas_height(), 300);
    }

    #[test]
    fn exact_multiple_of_chunk_size() {
        let mgr = ChunkManager::new(512, 256);
        assert_eq!(mgr.chunks_per_row(), 2);
        assert_eq!(mgr.chunk_count(), 2);
        assert_eq!(mgr.chunk_rect(0), Some(Rect2i::new(0, 0, 256, 256)));
        assert_eq!(mgr.chunk_rect(1), Some(Rect2i::new(256, 0, 256, 256)));
    }

    #[test]
    fn single_pixel_canvas() {
        let mgr = ChunkManager::new(1, 1);
        assert_eq!(mgr.chunks_per_row(), 1);
        assert_eq!(mgr.chunk_count(), 1);
        assert_eq!(mgr.chunk_rect(0), Some(Rect2i::new(0, 0, 1, 1)));
    }

    // ── Chunk indexing math ───────────────────────────────────────────

    #[test]
    fn indexing_math_300x300() {
        let mgr = ChunkManager::new(300, 300);
        assert_eq!(mgr.chunks_per_row(), 2);
        assert_eq!(mgr.chunk_count(), 4);

        assert_eq!(mgr.chunk_idx(0, 0), 0);
        assert_eq!(mgr.chunk_idx(1, 0), 1);
        assert_eq!(mgr.chunk_idx(0, 1), 2);
        assert_eq!(mgr.chunk_idx(1, 1), 3);
    }

    // ── Edge chunk sizing ─────────────────────────────────────────────

    #[test]
    fn edge_chunks_clipped_to_canvas() {
        let mgr = ChunkManager::new(300, 300);
        // Top-left: full 256×256
        assert_eq!(mgr.chunk_rect(0), Some(Rect2i::new(0, 0, 256, 256)));
        // Top-right: 44 wide (300 − 256)
        assert_eq!(mgr.chunk_rect(1), Some(Rect2i::new(256, 0, 44, 256)));
        // Bottom-left: 44 tall
        assert_eq!(mgr.chunk_rect(2), Some(Rect2i::new(0, 256, 256, 44)));
        // Bottom-right: 44 × 44
        assert_eq!(mgr.chunk_rect(3), Some(Rect2i::new(256, 256, 44, 44)));
    }

    #[test]
    fn chunk_rect_out_of_bounds() {
        let mgr = ChunkManager::new(300, 300);
        assert_eq!(mgr.chunk_rect(4), None);
        assert_eq!(mgr.chunk_rect(usize::MAX), None);
    }

    // ── Lazy allocation ───────────────────────────────────────────────

    #[test]
    fn lazy_allocation_returns_none() {
        let mgr = ChunkManager::new(512, 512);
        assert!(mgr.chunk_at(0, 0).is_none());
        assert!(mgr.chunk_at(255, 255).is_none());
        assert!(mgr.get_chunk(0).is_none());
    }

    #[test]
    fn chunk_at_mut_materializes() {
        let mut mgr = ChunkManager::new(512, 512);
        {
            let chunk = mgr.chunk_at_mut(10, 10).unwrap();
            assert_eq!(chunk.width(), 256);
            assert_eq!(chunk.height(), 256);
        }
        // Now visible through immutable path
        assert!(mgr.chunk_at(10, 10).is_some());
    }

    #[test]
    fn get_chunk_mut_materializes() {
        let mut mgr = ChunkManager::new(256, 256);
        assert!(mgr.get_chunk(0).is_none());
        let _ = mgr.get_chunk_mut(0);
        assert!(mgr.get_chunk(0).is_some());
    }

    #[test]
    fn chunk_at_out_of_bounds() {
        let mut mgr = ChunkManager::new(256, 256);
        assert!(mgr.chunk_at(256, 0).is_none());
        assert!(mgr.chunk_at(0, 256).is_none());
        assert!(mgr.chunk_at_mut(256, 0).is_none());
        assert!(mgr.chunk_at_mut(0, 256).is_none());
    }

    #[test]
    fn get_chunk_mut_out_of_bounds() {
        let mut mgr = ChunkManager::new(256, 256);
        assert!(mgr.get_chunk_mut(1).is_none());
    }

    // ── Dirty-rect tracking ───────────────────────────────────────────

    #[test]
    fn mark_dirty_single_chunk() {
        let mut mgr = ChunkManager::new(256, 256);
        mgr.mark_dirty(Rect2i::new(10, 20, 30, 40));

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].0, 0);
        assert_eq!(dirty[0].1, Rect2i::new(10, 20, 30, 40));
    }

    #[test]
    fn mark_dirty_crosses_chunk_boundaries() {
        let mut mgr = ChunkManager::new(512, 512);
        // Rect spans all 4 chunks: (200,200)–(300,300)
        mgr.mark_dirty(Rect2i::new(200, 200, 100, 100));

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 4);

        let expected = [
            (0, Rect2i::new(200, 200, 56, 56)),
            (1, Rect2i::new(256, 200, 44, 56)),
            (2, Rect2i::new(200, 256, 56, 44)),
            (3, Rect2i::new(256, 256, 44, 44)),
        ];

        for (idx, rect) in &dirty {
            let exp = expected.iter().find(|(i, _)| i == idx).unwrap();
            assert_eq!(*rect, exp.1, "chunk {idx} dirty rect mismatch");
        }
    }

    #[test]
    fn mark_dirty_union() {
        let mut mgr = ChunkManager::new(256, 256);
        mgr.mark_dirty(Rect2i::new(0, 0, 10, 10));
        mgr.mark_dirty(Rect2i::new(20, 20, 10, 10));

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 1);
        // Union of (0,0,10,10) and (20,20,10,10) = (0,0,30,30)
        assert_eq!(dirty[0].1, Rect2i::new(0, 0, 30, 30));
    }

    #[test]
    fn iter_dirty_clears_state() {
        let mut mgr = ChunkManager::new(256, 256);
        mgr.mark_dirty(Rect2i::new(0, 0, 10, 10));

        let first: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(first.len(), 1);

        let second: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn clear_dirty_explicit() {
        let mut mgr = ChunkManager::new(256, 256);
        mgr.mark_dirty(Rect2i::new(0, 0, 10, 10));
        mgr.clear_dirty();

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 0);
    }

    // ── Out-of-canvas clipping ────────────────────────────────────────

    #[test]
    fn mark_dirty_negative_origin_clips() {
        let mut mgr = ChunkManager::new(256, 256);
        mgr.mark_dirty(Rect2i::new(-10, -10, 30, 30));

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 1);
        assert_eq!(dirty[0].1, Rect2i::new(0, 0, 20, 20));
    }

    #[test]
    fn mark_dirty_beyond_canvas_right_bottom_clips() {
        let mut mgr = ChunkManager::new(300, 300);
        mgr.mark_dirty(Rect2i::new(280, 280, 100, 100));

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 1);
        // Only chunk 3 (bottom-right) affected: intersection with (256,256,44,44)
        assert_eq!(dirty[0].0, 3);
        assert_eq!(dirty[0].1, Rect2i::new(280, 280, 20, 20));
    }

    #[test]
    fn mark_dirty_completely_outside_canvas() {
        let mut mgr = ChunkManager::new(256, 256);
        mgr.mark_dirty(Rect2i::new(500, 500, 10, 10));

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 0);
    }

    #[test]
    fn mark_dirty_rect_spanning_full_canvas_width() {
        let mut mgr = ChunkManager::new(512, 256);
        mgr.mark_dirty(Rect2i::new(0, 100, 512, 10));

        let dirty: Vec<_> = mgr.iter_dirty_chunks().collect();
        assert_eq!(dirty.len(), 2);
        // Both chunks in the single row
        let mut sorted = dirty;
        sorted.sort_by_key(|(i, _)| *i);
        assert_eq!(sorted[0], (0, Rect2i::new(0, 100, 256, 10)));
        assert_eq!(sorted[1], (1, Rect2i::new(256, 100, 256, 10)));
    }

    // ── Large canvas ──────────────────────────────────────────────────

    #[test]
    fn large_canvas_grid() {
        let mgr = ChunkManager::new(1024, 768);
        assert_eq!(mgr.chunks_per_row(), 4);
        assert_eq!(mgr.chunk_count(), 4 * 3);

        // Bottom-right corner chunk
        let idx = mgr.chunk_idx(3, 2);
        assert_eq!(mgr.chunk_rect(idx), Some(Rect2i::new(768, 512, 256, 256)));
    }

    #[test]
    fn zero_dimension_canvas_has_no_chunks_or_dirty_regions() {
        let mut mgr = ChunkManager::new(0, 4096);
        assert_eq!(mgr.chunk_count(), 0);
        mgr.mark_dirty(Rect2i::new(0, 0, 1, 1));
        assert_eq!(mgr.iter_dirty_chunks().count(), 0);
    }

    #[test]
    fn huge_sparse_canvas_keeps_metadata_unmaterialized_and_reports_count_overflow() {
        let mgr = ChunkManager::new(usize::MAX, usize::MAX);
        assert_eq!(mgr.chunks_per_row(), usize::MAX / CHUNK_SIZE + 1);
        assert_eq!(mgr.chunk_count(), usize::MAX);
        assert_eq!(mgr.try_chunk_count(), None);
        assert!(mgr.get_chunk(0).is_none());
    }
}
