//! Pixel buffer types.

pub mod chunk_manager;

use crate::core::color::Color;
use crate::core::math::Rect2i;
use crate::core::select::Selection;

const BYTES_PER_PIXEL: usize = 4;

/// Contiguous RGBA8 pixel buffer, row-major. Row stride = `width * 4`.
/// R1 replaces the backing with chunked storage (`ChunkManager`); the
/// read/write API stays identical (ARCHITECTURE.md §5).
/// Change tracking: every mutation bumps `change_epoch` and accumulates a
/// dirty rect; consumers call `take_pixels_changed` once per frame.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PixelBuffer {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    change_epoch: u64,
    changed: Option<Rect2i>,
}

impl PixelBuffer {
    /// Allocates a transparent buffer of `width * height` pixels.
    pub fn new(width: usize, height: usize) -> Self {
        match Self::try_new(width, height) {
            Some(buffer) => buffer,
            None => panic!("pixel buffer dimensions exceed addressable memory"),
        }
    }

    /// Fallible constructor for dimensions supplied by an external boundary.
    pub fn try_new(width: usize, height: usize) -> Option<Self> {
        let len = width.checked_mul(height)?.checked_mul(BYTES_PER_PIXEL)?;
        Some(Self {
            width,
            height,
            pixels: vec![0; len],
            change_epoch: 0,
            changed: None,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Raw RGBA8 bytes; length == `width * height * 4`.
    pub fn as_bytes(&self) -> &[u8] {
        &self.pixels
    }

    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        &mut self.pixels
    }

    pub fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        let i = self.index(x, y)?;
        Some(Color::rgba(
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ))
    }

    /// Caller must guarantee `x < width && y < height`.
    pub fn get_pixel_unchecked(&self, x: usize, y: usize) -> Color {
        let i = y * (self.width * BYTES_PER_PIXEL) + x * BYTES_PER_PIXEL;
        Color::rgba(
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        )
    }

    /// Returns false when `(x, y)` is out of bounds; the buffer is unchanged.
    pub fn set_pixel(&mut self, x: usize, y: usize, color: Color) -> bool {
        match self.index(x, y) {
            Some(i) => {
                self.pixels[i..i + BYTES_PER_PIXEL]
                    .copy_from_slice(&[color.r, color.g, color.b, color.a]);
                self.note_changed(Rect2i::new(
                    x.min(i32::MAX as usize) as i32,
                    y.min(i32::MAX as usize) as i32,
                    1,
                    1,
                ));
                true
            }
            None => false,
        }
    }

    /// Caller must guarantee `x < width && y < height`.
    pub fn set_pixel_unchecked(&mut self, x: usize, y: usize, color: Color) {
        let i = y * (self.width * BYTES_PER_PIXEL) + x * BYTES_PER_PIXEL;
        self.pixels[i..i + BYTES_PER_PIXEL].copy_from_slice(&[color.r, color.g, color.b, color.a]);
        self.note_changed(Rect2i::new(
            x.min(i32::MAX as usize) as i32,
            y.min(i32::MAX as usize) as i32,
            1,
            1,
        ));
    }

    pub fn fill(&mut self, color: Color) {
        for chunk in self.pixels.chunks_exact_mut(BYTES_PER_PIXEL) {
            chunk.copy_from_slice(&[color.r, color.g, color.b, color.a]);
        }
        self.note_changed(Rect2i::new(
            0,
            0,
            self.width.min(i32::MAX as usize) as i32,
            self.height.min(i32::MAX as usize) as i32,
        ));
    }

    pub fn clear(&mut self) {
        self.fresh_fill(0);
        self.note_changed(Rect2i::new(
            0,
            0,
            self.width.min(i32::MAX as usize) as i32,
            self.height.min(i32::MAX as usize) as i32,
        ));
    }

    /// Flat full copy of the pixel data into a fresh buffer.
    ///
    /// The copy starts with clean change tracking: `change_epoch == 0` and no
    /// pending dirty rect, so it behaves like a freshly allocated buffer.
    pub fn duplicate(&self) -> Self {
        Self {
            width: self.width,
            height: self.height,
            pixels: self.pixels.clone(),
            change_epoch: 0,
            changed: None,
        }
    }

    /// Full-buffer copy; returns false when dimensions differ.
    pub fn copy_from(&mut self, src: &Self) -> bool {
        if src.width != self.width || src.height != self.height {
            return false;
        }
        self.pixels.copy_from_slice(&src.pixels);
        self.note_changed(Rect2i::new(
            0,
            0,
            self.width.min(i32::MAX as usize) as i32,
            self.height.min(i32::MAX as usize) as i32,
        ));
        true
    }

    /// Copies the `src_rect` region of `src` into `self` at `dst`.
    /// Returns false when the region does not fit either buffer.
    pub fn copy_region(&mut self, src: &Self, src_rect: Rect2i, dst: (i32, i32)) -> bool {
        let src_x = src_rect.x.max(0) as usize;
        let src_y = src_rect.y.max(0) as usize;
        let width = src_rect.w as usize;
        let height = src_rect.h as usize;
        let dst_x = dst.0.max(0) as usize;
        let dst_y = dst.1.max(0) as usize;
        let fits_src_x = src_x.checked_add(width).is_some_and(|end| end <= src.width);
        let fits_src_y = src_y
            .checked_add(height)
            .is_some_and(|end| end <= src.height);
        let fits_dst_x = dst_x
            .checked_add(width)
            .is_some_and(|end| end <= self.width);
        let fits_dst_y = dst_y
            .checked_add(height)
            .is_some_and(|end| end <= self.height);
        if !fits_src_x || !fits_src_y || !fits_dst_x || !fits_dst_y {
            return false;
        }
        for row in 0..height {
            let src_start = ((src_y + row) * src.width + src_x) * BYTES_PER_PIXEL;
            let dst_start = ((dst_y + row) * self.width + dst_x) * BYTES_PER_PIXEL;
            let len = width * BYTES_PER_PIXEL;
            self.pixels[dst_start..dst_start + len]
                .copy_from_slice(&src.pixels[src_start..src_start + len]);
        }
        self.note_changed(Rect2i::new(
            dst_x as i32,
            dst_y as i32,
            width as i32,
            height as i32,
        ));
        true
    }

    /// Writes raw RGBA8 bytes into `region` (canvas coordinates).
    /// Returns false when the region is out of bounds or the byte length
    /// does not match `region.w * region.h * 4`.
    pub fn blit_region(&mut self, region: Rect2i, bytes: &[u8]) -> bool {
        let rw = region.w as usize;
        let rh = region.h as usize;
        if region.x < 0
            || region.y < 0
            || rw == 0
            || rh == 0
            || (region.x as usize)
                .checked_add(rw)
                .is_none_or(|end| end > self.width)
            || (region.y as usize)
                .checked_add(rh)
                .is_none_or(|end| end > self.height)
        {
            return false;
        }
        let Some(expected) = rw
            .checked_mul(rh)
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
        else {
            return false;
        };
        if bytes.len() != expected {
            return false;
        }
        let x0 = region.x as usize;
        let y0 = region.y as usize;
        for row in 0..rh {
            let dst_start = ((y0 + row) * self.width + x0) * BYTES_PER_PIXEL;
            let src_start = row * rw * BYTES_PER_PIXEL;
            self.pixels[dst_start..dst_start + rw * BYTES_PER_PIXEL]
                .copy_from_slice(&bytes[src_start..src_start + rw * BYTES_PER_PIXEL]);
        }
        self.note_changed(region);
        true
    }

    /// Writes raw RGBA8 bytes into `region` (canvas coordinates), skipping
    /// every cell the mask does not select.  `None`, or a mask whose length
    /// does not match `region.area()`, writes everything exactly like
    /// [`blit_region`](Self::blit_region).  Returns false when the region is
    /// out of bounds or the byte length does not match `region.w * region.h * 4`.
    pub fn blit_region_masked(
        &mut self,
        region: Rect2i,
        bytes: &[u8],
        mask: Option<&[bool]>,
    ) -> bool {
        let rw = region.w as usize;
        let rh = region.h as usize;
        if region.x < 0
            || region.y < 0
            || rw == 0
            || rh == 0
            || (region.x as usize)
                .checked_add(rw)
                .is_none_or(|end| end > self.width)
            || (region.y as usize)
                .checked_add(rh)
                .is_none_or(|end| end > self.height)
        {
            return false;
        }
        let Some(expected) = rw
            .checked_mul(rh)
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
        else {
            return false;
        };
        if bytes.len() != expected {
            return false;
        }
        // The mask applies only when it covers the region cell for cell;
        // anything else degrades to the plain blit.
        let Some(cells) = mask.filter(|cells| cells.len() == rw * rh) else {
            return self.blit_region(region, bytes);
        };
        let x0 = region.x as usize;
        let y0 = region.y as usize;
        for row in 0..rh {
            let dst_start = ((y0 + row) * self.width + x0) * BYTES_PER_PIXEL;
            let src_start = row * rw * BYTES_PER_PIXEL;
            for x in 0..rw {
                if cells[row * rw + x] {
                    let px = x * BYTES_PER_PIXEL;
                    self.pixels[dst_start + px..dst_start + px + BYTES_PER_PIXEL]
                        .copy_from_slice(&bytes[src_start + px..src_start + px + BYTES_PER_PIXEL]);
                }
            }
        }
        self.note_changed(region);
        true
    }

    fn index(&self, x: usize, y: usize) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        y.checked_mul(self.width.checked_mul(BYTES_PER_PIXEL)?)?
            .checked_add(x.checked_mul(BYTES_PER_PIXEL)?)
    }

    fn fresh_fill(&mut self, value: u8) {
        self.pixels.fill(value);
    }

    fn note_changed(&mut self, region: Rect2i) {
        self.change_epoch += 1;
        self.changed = Some(match self.changed {
            Some(pending) => pending.union(region),
            None => region,
        });
    }

    pub fn change_epoch(&self) -> u64 {
        self.change_epoch
    }

    pub fn take_pixels_changed(&mut self) -> Option<Rect2i> {
        let r = self.changed;
        self.changed = None;
        r
    }

    pub fn pixels_changed_since(&self, epoch: u64) -> bool {
        self.change_epoch != epoch
    }

    /// Exports the RGBA8 bytes of `region` (canvas coordinates), row-major.
    /// Returns `None` when the region is out of bounds.
    ///
    /// `mask` optionally restricts the export to a selection's selected cells:
    /// a non-rectangular selection keeps the region's dimensions but writes
    /// transparent zeros for every cell the mask does not select, so a lifted
    /// buffer only carries the selected pixels.  A rectangular selection, a
    /// mask whose length does not match `region.area()`, or `None` exports the
    /// plain rectangular copy.
    pub fn export_region(&self, region: Rect2i, mask: Option<&Selection>) -> Option<Vec<u8>> {
        let rw = region.w as usize;
        let rh = region.h as usize;
        if region.x < 0
            || region.y < 0
            || rw == 0
            || rh == 0
            || (region.x as usize)
                .checked_add(rw)
                .is_none_or(|end| end > self.width)
            || (region.y as usize)
                .checked_add(rh)
                .is_none_or(|end| end > self.height)
        {
            return None;
        }
        let x0 = region.x as usize;
        let y0 = region.y as usize;
        let capacity = rw.checked_mul(rh)?.checked_mul(BYTES_PER_PIXEL)?;
        // Only a non-rectangular selection with a matching cell count can
        // zero cells; anything else is the plain rectangular copy.
        let cells = match mask {
            Some(selection) if !selection.is_rectangular() => selection.mask(),
            _ => None,
        };
        let cells = match cells {
            Some(cells) if cells.len() == rw * rh => Some(cells),
            _ => None,
        };
        let mut out = Vec::with_capacity(capacity);
        for row in 0..rh {
            let start = ((y0 + row) * self.width + x0) * BYTES_PER_PIXEL;
            let src = &self.pixels[start..start + rw * BYTES_PER_PIXEL];
            match cells {
                Some(cells) => {
                    for (x, px) in src.chunks_exact(BYTES_PER_PIXEL).enumerate() {
                        if cells[row * rw + x] {
                            out.extend_from_slice(px);
                        } else {
                            out.extend_from_slice(&[0u8; BYTES_PER_PIXEL]);
                        }
                    }
                }
                None => out.extend_from_slice(src),
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_is_transparent() {
        let b = PixelBuffer::new(4, 3);
        assert_eq!((b.width(), b.height()), (4, 3));
        assert_eq!(b.as_bytes().len(), 4 * 3 * 4);
        assert_eq!(b.get_pixel(0, 0), Some(Color::TRANSPARENT));
        assert_eq!(b.get_pixel(3, 2), Some(Color::TRANSPARENT));
    }

    #[test]
    fn set_get_roundtrip() {
        let mut b = PixelBuffer::new(2, 2);
        assert!(b.set_pixel(1, 1, Color::rgb(1, 2, 3)));
        assert_eq!(b.get_pixel(1, 1), Some(Color::rgb(1, 2, 3)));
        assert_eq!(b.get_pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn out_of_bounds_is_rejected() {
        let mut b = PixelBuffer::new(2, 2);
        assert!(!b.set_pixel(2, 0, Color::WHITE));
        assert!(!b.set_pixel(0, 2, Color::WHITE));
        assert!(!b.set_pixel(usize::MAX, 0, Color::WHITE));
        assert_eq!(b.get_pixel(2, 0), None);
        assert_eq!(b.get_pixel(0, 2), None);
    }

    #[test]
    fn unchecked_matches_checked_in_bounds() {
        let mut b = PixelBuffer::new(8, 8);
        b.set_pixel_unchecked(3, 4, Color::rgba(9, 8, 7, 6));
        assert_eq!(b.get_pixel_unchecked(3, 4), Color::rgba(9, 8, 7, 6));
        assert_eq!(b.get_pixel(3, 4), Some(Color::rgba(9, 8, 7, 6)));
    }

    #[test]
    fn fill_and_clear() {
        let mut b = PixelBuffer::new(3, 3);
        b.fill(Color::BLACK);
        assert_eq!(b.get_pixel(2, 2), Some(Color::BLACK));
        b.clear();
        assert_eq!(b.get_pixel(1, 1), Some(Color::TRANSPARENT));
    }

    #[test]
    fn copy_from_requires_same_dimensions() {
        let mut a = PixelBuffer::new(2, 2);
        a.fill(Color::WHITE);
        let mut b = PixelBuffer::new(2, 2);
        assert!(b.copy_from(&a));
        assert_eq!(b.as_bytes(), a.as_bytes());
        let mut c = PixelBuffer::new(3, 2);
        assert!(!c.copy_from(&a));
        assert_eq!(c.get_pixel(0, 0), Some(Color::TRANSPARENT));
    }

    #[test]
    fn duplicate_copies_pixels_and_starts_clean() {
        let mut a = PixelBuffer::new(2, 2);
        a.set_pixel(0, 0, Color::rgba(1, 2, 3, 4));
        a.set_pixel(1, 1, Color::rgba(5, 6, 7, 8));
        let mut b = a.duplicate();
        assert_eq!(b.as_bytes(), a.as_bytes());
        assert_eq!(b.change_epoch(), 0);
        assert_eq!(b.take_pixels_changed(), None);
    }

    #[test]
    fn duplicate_is_independent_of_original() {
        let mut a = PixelBuffer::new(2, 2);
        a.fill(Color::WHITE);
        let mut b = a.duplicate();
        b.set_pixel(0, 0, Color::BLACK);
        assert_eq!(a.get_pixel(0, 0), Some(Color::WHITE));
        assert_eq!(b.get_pixel(0, 0), Some(Color::BLACK));
        assert_eq!(a.change_epoch(), 1);
        assert_eq!(b.change_epoch(), 1);
        a.set_pixel(1, 1, Color::BLACK);
        assert_eq!(b.get_pixel(1, 1), Some(Color::WHITE));
    }

    #[test]
    fn stride_layout_is_row_major() {
        let mut b = PixelBuffer::new(2, 1);
        b.set_pixel(0, 0, Color::rgba(1, 2, 3, 4));
        b.set_pixel(1, 0, Color::rgba(5, 6, 7, 8));
        assert_eq!(b.as_bytes(), &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn epoch_increments_exactly_once_per_successful_mutation() {
        let mut b = PixelBuffer::new(4, 4);
        assert_eq!(b.change_epoch(), 0);
        b.set_pixel(0, 0, Color::WHITE);
        assert_eq!(b.change_epoch(), 1);
        b.set_pixel(2, 3, Color::BLACK);
        assert_eq!(b.change_epoch(), 2);
        assert!(!b.set_pixel(99, 99, Color::WHITE));
        assert_eq!(b.change_epoch(), 2);
    }

    #[test]
    fn take_pixels_changed_tracks_union_of_dirty_rects() {
        let mut b = PixelBuffer::new(8, 8);
        assert_eq!(b.take_pixels_changed(), None);
        b.set_pixel(2, 3, Color::WHITE);
        assert_eq!(b.take_pixels_changed(), Some(Rect2i::new(2, 3, 1, 1)));
        assert_eq!(b.take_pixels_changed(), None);
        b.set_pixel(5, 5, Color::BLACK);
        b.set_pixel(2, 3, Color::WHITE);
        assert_eq!(b.take_pixels_changed(), Some(Rect2i::new(2, 3, 4, 3)));
    }

    #[test]
    fn blit_region_marks_exactly_blitted_rect() {
        let mut b = PixelBuffer::new(8, 8);
        let bytes = vec![0u8; 4 * 3 * 4];
        assert!(b.blit_region(Rect2i::new(1, 2, 4, 3), &bytes));
        assert_eq!(b.take_pixels_changed(), Some(Rect2i::new(1, 2, 4, 3)));
    }

    #[test]
    fn fill_marks_full_rect() {
        let mut b = PixelBuffer::new(5, 3);
        b.fill(Color::WHITE);
        assert_eq!(b.take_pixels_changed(), Some(Rect2i::new(0, 0, 5, 3)));
    }

    #[test]
    fn failed_blit_region_marks_nothing() {
        let mut b = PixelBuffer::new(2, 2);
        let bytes = vec![0u8; 4 * 4 * 4];
        assert!(!b.blit_region(Rect2i::new(0, 0, 4, 4), &bytes));
        assert_eq!(b.change_epoch(), 0);
        assert_eq!(b.take_pixels_changed(), None);
    }

    #[test]
    fn export_region_roundtrip() {
        let mut b = PixelBuffer::new(3, 2);
        b.set_pixel(0, 0, Color::rgba(1, 2, 3, 4));
        b.set_pixel(1, 0, Color::rgba(5, 6, 7, 8));
        b.set_pixel(2, 0, Color::rgba(9, 10, 11, 12));
        b.set_pixel(0, 1, Color::rgba(13, 14, 15, 16));
        b.set_pixel(1, 1, Color::rgba(17, 18, 19, 20));
        b.set_pixel(2, 1, Color::rgba(21, 22, 23, 24));
        let full = b.export_region(Rect2i::new(0, 0, 3, 2), None).unwrap();
        assert_eq!(
            full,
            vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
                24,
            ]
        );
        let sub = b.export_region(Rect2i::new(1, 0, 2, 1), None).unwrap();
        assert_eq!(sub, vec![5, 6, 7, 8, 9, 10, 11, 12]);
        assert_eq!(b.export_region(Rect2i::new(0, 0, 99, 1), None), None);
    }

    #[test]
    fn export_region_with_mask_zeros_non_mask_pixels() {
        let mut b = PixelBuffer::new(2, 2);
        b.set_pixel(0, 0, Color::rgba(1, 2, 3, 255));
        b.set_pixel(1, 0, Color::rgba(4, 5, 6, 255));
        b.set_pixel(0, 1, Color::rgba(7, 8, 9, 255));
        b.set_pixel(1, 1, Color::rgba(10, 11, 12, 255));
        // Row-major mask selecting only the diagonal cells (0,0) and (1,1).
        let sel =
            Selection::capture_mask(&b, Rect2i::new(0, 0, 2, 2), vec![true, false, false, true])
                .unwrap();
        let out = b
            .export_region(Rect2i::new(0, 0, 2, 2), Some(&sel))
            .unwrap();
        assert_eq!(
            out,
            vec![
                1, 2, 3, 255, // (0,0) selected: copied
                0, 0, 0, 0, // (1,0) masked out: zeroed
                0, 0, 0, 0, // (0,1) masked out: zeroed
                10, 11, 12, 255, // (1,1) selected: copied
            ]
        );
    }

    #[test]
    fn export_region_rectangular_and_none_match_old_behavior() {
        let mut b = PixelBuffer::new(3, 2);
        b.set_pixel(0, 0, Color::rgba(1, 2, 3, 4));
        b.set_pixel(1, 0, Color::rgba(5, 6, 7, 8));
        b.set_pixel(2, 0, Color::rgba(9, 10, 11, 12));
        b.set_pixel(0, 1, Color::rgba(13, 14, 15, 16));
        b.set_pixel(1, 1, Color::rgba(17, 18, 19, 20));
        b.set_pixel(2, 1, Color::rgba(21, 22, 23, 24));
        let rect = Rect2i::new(0, 0, 3, 2);
        let plain = b.export_region(rect, None).unwrap();
        // A rectangular selection is the fast path: byte-identical output.
        let sel = Selection::capture(&b, rect).unwrap();
        assert!(sel.is_rectangular());
        assert_eq!(b.export_region(rect, Some(&sel)).unwrap(), plain);
    }

    #[test]
    fn export_region_mask_length_mismatch_is_fully_selected() {
        let mut b = PixelBuffer::new(2, 2);
        b.set_pixel(0, 0, Color::rgba(1, 2, 3, 255));
        let sel =
            Selection::capture_mask(&b, Rect2i::new(0, 0, 2, 2), vec![true, false, false, false])
                .unwrap();
        // Exporting a smaller region than the selection's box: the mask length
        // no longer matches, so the export stays a plain rectangular copy.
        let out = b
            .export_region(Rect2i::new(0, 0, 1, 1), Some(&sel))
            .unwrap();
        assert_eq!(out, vec![1, 2, 3, 255]);
    }

    #[test]
    fn blit_region_masked_skips_unselected_cells() {
        let mut b = PixelBuffer::new(2, 1);
        assert!(b.blit_region(Rect2i::new(0, 0, 2, 1), &[255u8; 8]));
        // Mask selects only the first cell: the red pixel lands there and
        // the second cell keeps its white value.
        let red = [255u8, 0, 0, 255, 255, 0, 0, 255];
        assert!(b.blit_region_masked(Rect2i::new(0, 0, 2, 1), &red, Some(&[true, false])));
        assert_eq!(b.get_pixel(0, 0), Some(Color::rgb(255, 0, 0)));
        assert_eq!(b.get_pixel(1, 0), Some(Color::rgb(255, 255, 255)));
        // None writes everything, like blit_region.
        assert!(b.blit_region_masked(Rect2i::new(0, 0, 2, 1), &red, None));
        assert_eq!(b.get_pixel(1, 0), Some(Color::rgb(255, 0, 0)));
        // A length mismatch writes everything too.
        assert!(b.blit_region_masked(Rect2i::new(0, 0, 2, 1), &red, Some(&[true])));
        assert_eq!(b.get_pixel(0, 0), Some(Color::rgb(255, 0, 0)));
        // Bounds and length failures behave like blit_region.
        assert!(!b.blit_region_masked(Rect2i::new(0, 0, 4, 1), &red, Some(&[true, false])));
        assert!(!b.blit_region_masked(Rect2i::new(0, 0, 2, 1), &[0u8; 4], Some(&[true, false])));
    }

    #[test]
    fn pixels_changed_since_transitions() {
        let mut b = PixelBuffer::new(4, 4);
        let e0 = b.change_epoch();
        assert!(!b.pixels_changed_since(e0));
        b.set_pixel(0, 0, Color::WHITE);
        assert!(b.pixels_changed_since(e0));
        let e1 = b.change_epoch();
        assert!(!b.pixels_changed_since(e1));
        b.fill(Color::BLACK);
        assert!(b.pixels_changed_since(e1));
    }

    #[test]
    fn checked_constructor_rejects_overflow_without_partial_buffer() {
        assert!(PixelBuffer::try_new(usize::MAX, 2).is_none());
        assert!(PixelBuffer::try_new(usize::MAX / 4 + 1, 1).is_none());
        assert_eq!(PixelBuffer::new(0, usize::MAX).as_bytes(), &[] as &[u8]);
    }
}
