//! Canvas texture upload + chunk management — R1 milestone.
//!
//! The canvas is a single `wgpu::Texture` (format `Rgba8UnormSrgb`, usage
//! `COPY_DST | TEXTURE_BINDING`) split into 256×256 chunks [D40]. Each chunk
//! is composited on the CPU and uploaded via `queue.write_texture`.
//!
//! # Dirty rectangles
//!
//! `PixelBuffer::pixels_changed(rect)` signals dirty regions. Callers feed
//! them into [`CanvasRenderer::mark_dirty`], which unions them into a single
//! accumulator. [`CanvasRenderer::take_dirty`] returns-and-clears the
//! accumulated rect; [`rect_to_chunks`] maps it to the set of chunk
//! coordinates that need re-upload.
//!
//! # Version counter [D65]
//!
//! An `Arc<AtomicU64>` version counter is bumped on every texture mutation
//! (`upload_region`, `resize`). Projection surfaces compare their cached
//! value against the shared counter and re-present only when the canvas
//! version changed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::core::math::Rect2i;

/// Side length of one canvas chunk in pixels [D40].
pub const CHUNK_SIZE: u32 = 256;

/// Canvas texture format: straight-alpha RGBA8, sRGB-encoded.
const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Canvas texture usage: written via `queue.write_texture` and sampled by
/// projection passes [D65].
const CANVAS_USAGE: wgpu::TextureUsages =
    wgpu::TextureUsages::COPY_DST.union(wgpu::TextureUsages::TEXTURE_BINDING);

/// Owns the canvas texture and tracks which regions need re-upload.
///
/// The renderer is deliberately generic: it takes bytes in and performs the
/// `write_texture`; compositing model state into bytes is a later wave.
pub struct CanvasRenderer {
    texture: Arc<wgpu::Texture>,
    canvas_size: (u32, u32),
    version: Arc<AtomicU64>,
    /// Union of all un-uploaded changes since the last sync. `None` when
    /// nothing is dirty (a bare `Rect2i::ZERO` cannot serve as "empty"
    /// because `union` would pull the origin into the result).
    dirty: Option<Rect2i>,
}

impl CanvasRenderer {
    /// Creates the canvas texture. A canvas is never empty: dimensions of 0
    /// are clamped to 1.
    pub fn new(device: &wgpu::Device, canvas_width: u32, canvas_height: u32) -> Self {
        let canvas_width = canvas_width.max(1);
        let canvas_height = canvas_height.max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d {
                width: canvas_width,
                height: canvas_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: CANVAS_USAGE,
            view_formats: &[],
        });
        Self {
            texture: Arc::new(texture),
            canvas_size: (canvas_width, canvas_height),
            version: Arc::new(AtomicU64::new(0)),
            dirty: None,
        }
    }

    /// Shared handle to the canvas texture (projection passes bind it).
    pub fn canvas_texture(&self) -> Arc<wgpu::Texture> {
        Arc::clone(&self.texture)
    }

    /// Shared version counter, bumped on every texture mutation [D65].
    pub fn version_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.version)
    }

    /// Canvas size in pixels `(width, height)`.
    pub fn canvas_size(&self) -> (u32, u32) {
        self.canvas_size
    }

    /// Re-allocates the canvas texture at a new size, marks the whole canvas
    /// dirty and bumps the version. Dimensions of 0 are clamped to 1.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        canvas_width: u32,
        canvas_height: u32,
    ) {
        let canvas_width = canvas_width.max(1);
        let canvas_height = canvas_height.max(1);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas"),
            size: wgpu::Extent3d {
                width: canvas_width,
                height: canvas_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_FORMAT,
            usage: CANVAS_USAGE,
            view_formats: &[],
        });
        self.texture = Arc::new(texture);
        self.canvas_size = (canvas_width, canvas_height);
        self.dirty = Some(Rect2i::new(
            0,
            0,
            canvas_width.min(i32::MAX as u32) as i32,
            canvas_height.min(i32::MAX as u32) as i32,
        ));
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    /// Unions `rect` (clipped to the canvas) into the dirty accumulator.
    pub fn mark_dirty(&mut self, rect: Rect2i) {
        let canvas = Rect2i::new(
            0,
            0,
            self.canvas_size.0.min(i32::MAX as u32) as i32,
            self.canvas_size.1.min(i32::MAX as u32) as i32,
        );
        let clipped = rect.clamp_to(canvas);
        if clipped.is_empty() {
            return;
        }
        self.dirty = Some(match self.dirty {
            Some(acc) => acc.union(clipped),
            None => clipped,
        });
    }

    /// Returns-and-clears the accumulated dirty rect, or `None` if nothing is
    /// dirty since the last sync.
    pub fn take_dirty(&mut self) -> Option<Rect2i> {
        self.dirty.take()
    }

    /// Maps a dirty rect to the chunk coordinates it intersects (pure).
    pub fn dirty_chunks(&self, dirty: Rect2i) -> Vec<(u32, u32)> {
        rect_to_chunks(dirty, self.canvas_size.0, self.canvas_size.1)
    }

    /// Uploads `rgba8` (tightly packed, `rect_w * rect_h * 4` bytes) into the
    /// canvas texture at `rect` (clipped to the canvas) with a single
    /// `queue.write_texture`, then bumps the version counter.
    ///
    /// Returns `false` (and uploads nothing) if the clipped rect is empty or
    /// the byte length does not match the clipped rect.
    pub fn upload_region(&self, queue: &wgpu::Queue, rect: Rect2i, rgba8: &[u8]) -> bool {
        let canvas = Rect2i::new(
            0,
            0,
            self.canvas_size.0.min(i32::MAX as u32) as i32,
            self.canvas_size.1.min(i32::MAX as u32) as i32,
        );
        let clipped = rect.clamp_to(canvas);
        if clipped.is_empty() {
            return false;
        }
        let w = clipped.w as u32;
        let h = clipped.h as u32;
        let Some(expected_len) = (w as usize)
            .checked_mul(h as usize)
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            return false;
        };
        if rgba8.len() != expected_len {
            return false;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.texture.as_ref(),
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: clipped.x as u32,
                    y: clipped.y as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            rgba8,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(0),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.version.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// Uploads exactly one `CHUNK_SIZE × CHUNK_SIZE` chunk. The on-canvas
    /// chunk rect is clipped at the canvas edge by [`chunk_rect`], and
    /// [`upload_region`] validates the byte length against that clipped rect.
    pub fn upload_chunk(
        &self,
        queue: &wgpu::Queue,
        chunk_x: u32,
        chunk_y: u32,
        rgba8: &[u8],
    ) -> bool {
        let rect = chunk_rect(chunk_x, chunk_y, self.canvas_size.0, self.canvas_size.1);
        self.upload_region(queue, rect, rgba8)
    }
}

/// Maps a canvas-space rect to the set of `CHUNK_SIZE` chunk coordinates it
/// intersects, clamped to the canvas bounds. Returns chunks in row-major
/// order for determinism. Empty if the rect does not touch the canvas.
pub fn rect_to_chunks(rect: Rect2i, canvas_width: u32, canvas_height: u32) -> Vec<(u32, u32)> {
    let canvas = Rect2i::new(
        0,
        0,
        canvas_width.min(i32::MAX as u32) as i32,
        canvas_height.min(i32::MAX as u32) as i32,
    );
    let clipped = rect.clamp_to(canvas);
    if clipped.is_empty() {
        return Vec::new();
    }
    let cs = CHUNK_SIZE as i32;
    let start_cx = clipped.x / cs;
    let start_cy = clipped.y / cs;
    // `right() - 1` is the last pixel column; dividing it yields the last
    // chunk column the rect touches.
    let end_cx = (clipped.right() - 1) / cs;
    let end_cy = (clipped.bottom() - 1) / cs;
    let mut chunks = Vec::new();
    for cy in start_cy..=end_cy {
        for cx in start_cx..=end_cx {
            chunks.push((cx as u32, cy as u32));
        }
    }
    chunks
}

/// The on-canvas rect for a chunk, clipped at the right/bottom canvas edge.
/// Chunks fully outside the canvas yield an empty rect.
pub fn chunk_rect(chunk_x: u32, chunk_y: u32, canvas_width: u32, canvas_height: u32) -> Rect2i {
    let cs = CHUNK_SIZE as u64;
    let x = (chunk_x as u64).saturating_mul(cs);
    let y = (chunk_y as u64).saturating_mul(cs);
    let w = (canvas_width as u64).saturating_sub(x).min(cs);
    let h = (canvas_height as u64).saturating_sub(y).min(cs);
    let x = x.min(i32::MAX as u64) as i32;
    let y = y.min(i32::MAX as u64) as i32;
    let w = w.min(i32::MAX as u64) as i32;
    let h = h.min(i32::MAX as u64) as i32;
    Rect2i::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pixel_dirty_maps_to_one_chunk() {
        let chunks = rect_to_chunks(Rect2i::new(50, 50, 1, 1), 1000, 800);
        assert_eq!(chunks, vec![(0, 0)]);
    }

    #[test]
    fn rect_spanning_chunk_boundary_hits_all_four_chunks() {
        // Pixels (255,255) and (256,256) straddle the (0,0)/(1,1) boundary.
        let chunks = rect_to_chunks(Rect2i::new(255, 255, 2, 2), 1000, 800);
        assert_eq!(chunks, vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
    }

    #[test]
    fn rect_spanning_two_by_three_chunks_is_row_major() {
        let chunks = rect_to_chunks(Rect2i::new(0, 0, 512, 768), 1000, 800);
        assert_eq!(chunks, vec![(0, 0), (1, 0), (0, 1), (1, 1), (0, 2), (1, 2)]);
    }

    #[test]
    fn rect_fully_outside_canvas_is_empty() {
        let chunks = rect_to_chunks(Rect2i::new(300, 300, 50, 50), 200, 200);
        assert!(chunks.is_empty());
    }

    #[test]
    fn partially_out_of_bounds_rect_is_clamped() {
        // (200,200)-(400,400) on a 300×300 canvas: only in-canvas chunks.
        let chunks = rect_to_chunks(Rect2i::new(200, 200, 200, 200), 300, 300);
        assert_eq!(chunks, vec![(0, 0), (1, 0), (0, 1), (1, 1)]);
    }

    #[test]
    fn canvas_smaller_than_chunk_size_is_single_chunk() {
        let chunks = rect_to_chunks(Rect2i::new(0, 0, 100, 50), 100, 50);
        assert_eq!(chunks, vec![(0, 0)]);
        let whole_canvas = rect_to_chunks(Rect2i::new(0, 0, 100, 50), 100, 50);
        assert_eq!(whole_canvas, vec![(0, 0)]);
    }

    #[test]
    fn empty_rect_yields_no_chunks() {
        assert!(rect_to_chunks(Rect2i::ZERO, 1000, 800).is_empty());
        assert!(rect_to_chunks(Rect2i::new(10, 10, -5, -5), 1000, 800).is_empty());
    }

    #[test]
    fn chunk_zero_zero_is_full_chunk_on_large_canvas() {
        assert_eq!(chunk_rect(0, 0, 1000, 800), Rect2i::new(0, 0, 256, 256));
    }

    #[test]
    fn right_edge_chunk_is_clipped() {
        // Chunk (1,1) starts at (256,256); canvas is 300×300 → 44px wide.
        assert_eq!(chunk_rect(1, 1, 300, 300), Rect2i::new(256, 256, 44, 44));
    }

    #[test]
    fn bottom_edge_chunk_is_clipped() {
        // Chunk (0,1) starts at (0,256); canvas is 300×300 → 44px tall.
        assert_eq!(chunk_rect(0, 1, 300, 300), Rect2i::new(0, 256, 256, 44));
    }

    #[test]
    fn chunk_on_exact_chunk_sized_canvas_is_full() {
        assert_eq!(chunk_rect(0, 0, 256, 256), Rect2i::new(0, 0, 256, 256));
    }

    #[test]
    fn huge_chunk_coordinates_do_not_wrap() {
        assert_eq!(
            chunk_rect(u32::MAX, u32::MAX, u32::MAX, u32::MAX),
            Rect2i::new(i32::MAX, i32::MAX, 0, 0)
        );
    }

    #[test]
    fn chunk_round_trip_unions_back_to_full_canvas() {
        let canvas = Rect2i::new(0, 0, 300, 300);
        let chunks = rect_to_chunks(canvas, 300, 300);
        let rebuilt = chunks.iter().fold(Rect2i::ZERO, |acc, &(cx, cy)| {
            if acc.is_empty() {
                chunk_rect(cx, cy, 300, 300)
            } else {
                acc.union(chunk_rect(cx, cy, 300, 300))
            }
        });
        assert_eq!(rebuilt, canvas);
    }
}
