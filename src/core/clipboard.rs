//! Internal clipboard: raw RGBA8 pixel region (D51) plus the PNG bridge
//! (G2) that turns a canvas [`Selection`] into RGBA bytes and to/from PNG.
//!
//! The region type is pure core — no filesystem I/O, no UI crates. The PNG
//! bridge wraps the pure codec in [`crate::core::png_codec`]
//! ([`crate::core::png_codec::encode_png`] /
//! [`crate::core::png_codec::decode_png`]) and only ever operates on
//! in-memory byte buffers; [`selection_to_rgba`] itself is pure core (no I/O
//! at all). Only the OS clipboard bridge (egui `Context::copy_image`) layers
//! on top of these, in the UI crate.

use crate::core::buffer::PixelBuffer;
use crate::core::color::Color;
use crate::core::math::Rect2i;
use crate::core::select::Selection;
use crate::core::transform::LayerBuffer;

const BYTES_PER_PIXEL: usize = 4;

/// Raw RGBA8 pixel region, row-major, stride = `width * 4`.
///
/// Invariant: `pixels.len() == width * height * 4`. Enforced by the
/// constructors; `new` panics on violation, `try_new` returns `None`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClipboardRegion {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl ClipboardRegion {
    /// Creates a region from raw RGBA8 bytes.
    ///
    /// # Panics
    ///
    /// Panics when `pixels.len() != width * height * 4`. Use [`Self::try_new`]
    /// for a checked, non-panicking constructor.
    pub fn new(width: usize, height: usize, pixels: Vec<u8>) -> Self {
        Self::try_new(width, height, pixels)
            .expect("ClipboardRegion::new: pixels.len() must equal width * height * 4")
    }

    /// Checked constructor: `None` when `pixels.len() != width * height * 4`
    /// or the byte count overflows `usize`.
    pub fn try_new(width: usize, height: usize, pixels: Vec<u8>) -> Option<Self> {
        let expected = width.checked_mul(height)?.checked_mul(BYTES_PER_PIXEL)?;
        if pixels.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Copies the whole buffer into a new region.
    pub fn from_pixel_buffer(buffer: &PixelBuffer) -> Self {
        Self {
            width: buffer.width(),
            height: buffer.height(),
            pixels: buffer.as_bytes().to_vec(),
        }
    }

    /// Copies the intersection of `rect` with the buffer into a new region.
    ///
    /// The region is clipped to the buffer bounds; returns `None` when the
    /// rectangle does not intersect the buffer at all.
    pub fn from_pixel_buffer_region(buffer: &PixelBuffer, rect: Rect2i) -> Option<Self> {
        let bounds = Rect2i::new(0, 0, buffer.width() as i32, buffer.height() as i32);
        let clipped = rect.clamp_to(bounds);
        if clipped.is_empty() {
            return None;
        }
        let bytes = buffer.export_region(clipped, None)?;
        Some(Self {
            width: clipped.w as usize,
            height: clipped.h as usize,
            pixels: bytes,
        })
    }

    /// Copies the selected pixels of `rect` into a new region: `mask` is a
    /// row-major `rect.area()` boolean map, and unselected pixels are written
    /// transparent.  Returns `None` when the clipped rect is empty.
    pub fn from_pixel_buffer_masked(
        buffer: &PixelBuffer,
        rect: Rect2i,
        mask: &[bool],
    ) -> Option<Self> {
        let bounds = Rect2i::new(0, 0, buffer.width() as i32, buffer.height() as i32);
        let clipped = rect.clamp_to(bounds);
        if clipped.is_empty() {
            return None;
        }
        let mut pixels = vec![0u8; clipped.area() as usize * 4];
        for y in clipped.y..clipped.bottom() {
            for x in clipped.x..clipped.right() {
                let src_idx = ((y - rect.y) as i64 * rect.w as i64 + (x - rect.x) as i64) as usize;
                if !mask.get(src_idx).copied().unwrap_or(false) {
                    continue;
                }
                let Some(px) = buffer.get_pixel(x as usize, y as usize) else {
                    continue;
                };
                let dst_idx =
                    ((y - clipped.y) as i64 * clipped.w as i64 + (x - clipped.x) as i64) as usize;
                pixels[dst_idx * 4] = px.r;
                pixels[dst_idx * 4 + 1] = px.g;
                pixels[dst_idx * 4 + 2] = px.b;
                pixels[dst_idx * 4 + 3] = px.a;
            }
        }
        Some(Self {
            width: clipped.w as usize,
            height: clipped.h as usize,
            pixels,
        })
    }

    /// New `PixelBuffer` with this region's dimensions and RGBA8 pixels.
    pub fn to_pixel_buffer(&self) -> PixelBuffer {
        let mut buffer = PixelBuffer::new(self.width, self.height);
        buffer.as_bytes_mut().copy_from_slice(&self.pixels);
        buffer
    }

    /// Returns the pixel at `(x, y)`, or `None` when out of bounds.
    pub fn get_pixel(&self, x: usize, y: usize) -> Option<Color> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = y * (self.width * BYTES_PER_PIXEL) + x * BYTES_PER_PIXEL;
        Some(Color::rgba(
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ))
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Raw RGBA8 bytes; length == `width * height * 4`.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Extracts the selected pixels of `buffer` as row-major RGBA8, writing every
/// unselected pixel transparent (`[0, 0, 0, 0]`).
///
/// The extracted region is `selection.rect()` clipped to the buffer bounds
/// (`0, 0, buffer.w, buffer.h`). Both mask kinds are handled through
/// [`Selection::contains`]: a rectangular selection (fast path) copies its
/// whole box, a bitmap selection leaves its holes transparent.
///
/// Returns `(rgba, width, height)`. An empty result `(Vec::new(), 0, 0)` means
/// the selection does not intersect the buffer, the buffer is empty, or
/// `buffer.buf` is too short for `buffer.w * buffer.h * 4`; the signature is
/// not fallible, so an inconsistent buffer is reported as "nothing to copy"
/// rather than panicking.
pub fn selection_to_rgba(buffer: &LayerBuffer, selection: &Selection) -> (Vec<u8>, u32, u32) {
    let Some(needed) = buffer
        .w
        .checked_mul(buffer.h)
        .and_then(|n| n.checked_mul(BYTES_PER_PIXEL))
    else {
        return (Vec::new(), 0, 0);
    };
    if buffer.buf.len() < needed {
        return (Vec::new(), 0, 0);
    }
    let bounds = Rect2i::new(0, 0, buffer.w as i32, buffer.h as i32);
    let clipped = selection.rect().clamp_to(bounds);
    if clipped.is_empty() {
        return (Vec::new(), 0, 0);
    }
    let w = clipped.w as usize;
    let h = clipped.h as usize;
    // Transparent defaults: unselected / masked-out pixels stay zeroed.
    let mut out = vec![0u8; w * h * BYTES_PER_PIXEL];
    for y in clipped.y..clipped.bottom() {
        for x in clipped.x..clipped.right() {
            if !selection.contains(x, y) {
                continue;
            }
            let src = (y as usize * buffer.w + x as usize) * BYTES_PER_PIXEL;
            let dst = ((y - clipped.y) as usize * w + (x - clipped.x) as usize) * BYTES_PER_PIXEL;
            out[dst..dst + BYTES_PER_PIXEL]
                .copy_from_slice(&buffer.buf[src..src + BYTES_PER_PIXEL]);
        }
    }
    (out, w as u32, h as u32)
}

/// Encodes an RGBA8 image (`w × h`, row-major, stride `w * 4`) as PNG bytes.
///
/// Thin wrapper over [`crate::core::png_codec::encode_png`]. The clipboard
/// contract is infallible, so an encoding failure (including a mismatched
/// `rgba` length) is reported as an empty `Vec` — callers should treat that
/// as "no image". Never panics.
pub fn png_encode(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    crate::core::png_codec::encode_png(w as usize, h as usize, rgba).unwrap_or_default()
}

/// Decodes PNG bytes into `(rgba, width, height)` row-major RGBA8.
///
/// Thin wrapper over [`crate::core::png_codec::decode_png`]. Malformed,
/// truncated or empty input yields `Err`; never panics.
pub fn png_decode(png: &[u8]) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
    let image = crate::core::png_codec::decode_png(png)?;
    Ok((image.rgba, image.width as u32, image.height as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_exact_size() {
        let r = ClipboardRegion::new(2, 2, vec![0; 16]);
        assert_eq!((r.width(), r.height()), (2, 2));
        assert_eq!(r.pixels().len(), 16);
        assert!(!r.is_empty());
    }

    #[test]
    fn try_new_rejects_size_mismatch() {
        assert!(ClipboardRegion::try_new(2, 2, vec![0; 15]).is_none());
        assert!(ClipboardRegion::try_new(2, 2, vec![0; 17]).is_none());
        assert!(ClipboardRegion::try_new(2, 2, vec![0; 16]).is_some());
        // Overflowing byte count.
        assert!(ClipboardRegion::try_new(usize::MAX, 2, Vec::new()).is_none());
    }

    #[test]
    #[should_panic(expected = "width * height * 4")]
    fn new_panics_on_size_mismatch() {
        let _ = ClipboardRegion::new(3, 3, vec![0; 8]);
    }

    #[test]
    fn from_pixel_buffer_roundtrip() {
        let mut b = PixelBuffer::new(3, 2);
        b.set_pixel(0, 0, Color::rgba(1, 2, 3, 4));
        b.set_pixel(2, 1, Color::rgba(5, 6, 7, 8));
        let r = ClipboardRegion::from_pixel_buffer(&b);
        assert_eq!((r.width(), r.height()), (3, 2));
        assert_eq!(r.pixels(), b.as_bytes());
        assert_eq!(r.get_pixel(0, 0), Some(Color::rgba(1, 2, 3, 4)));
        assert_eq!(r.get_pixel(2, 1), Some(Color::rgba(5, 6, 7, 8)));
    }

    #[test]
    fn from_pixel_buffer_region_roundtrip() {
        let mut b = PixelBuffer::new(4, 3);
        for y in 0..3 {
            for x in 0..4 {
                b.set_pixel(x, y, Color::rgba(x as u8, y as u8, 0, 255));
            }
        }
        let r = ClipboardRegion::from_pixel_buffer_region(&b, Rect2i::new(1, 1, 2, 2)).unwrap();
        assert_eq!((r.width(), r.height()), (2, 2));
        assert_eq!(r.get_pixel(0, 0), Some(Color::rgba(1, 1, 0, 255)));
        assert_eq!(r.get_pixel(1, 1), Some(Color::rgba(2, 2, 0, 255)));
        assert_eq!(r.get_pixel(1, 0), Some(Color::rgba(2, 1, 0, 255)));
    }

    #[test]
    fn from_pixel_buffer_region_clips_partial_overlap() {
        let mut b = PixelBuffer::new(4, 4);
        b.fill(Color::WHITE);
        // Rect sticking out past the right/bottom edges.
        let r = ClipboardRegion::from_pixel_buffer_region(&b, Rect2i::new(2, 2, 10, 10)).unwrap();
        assert_eq!((r.width(), r.height()), (2, 2));
        assert_eq!(r.get_pixel(1, 1), Some(Color::WHITE));
        // Rect sticking out past the top/left edges (negative origin).
        let r2 = ClipboardRegion::from_pixel_buffer_region(&b, Rect2i::new(-2, -2, 4, 4)).unwrap();
        assert_eq!((r2.width(), r2.height()), (2, 2));
        assert_eq!(r2.get_pixel(0, 0), Some(Color::WHITE));
    }

    #[test]
    fn from_pixel_buffer_region_no_intersection() {
        let b = PixelBuffer::new(4, 4);
        assert!(ClipboardRegion::from_pixel_buffer_region(&b, Rect2i::new(5, 5, 2, 2)).is_none());
        assert!(ClipboardRegion::from_pixel_buffer_region(&b, Rect2i::new(-5, -5, 2, 2)).is_none());
        assert!(ClipboardRegion::from_pixel_buffer_region(&b, Rect2i::new(0, 0, 0, 0)).is_none());
    }

    #[test]
    fn to_pixel_buffer_roundtrip() {
        let r = ClipboardRegion::new(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let b = r.to_pixel_buffer();
        assert_eq!((b.width(), b.height()), (2, 1));
        assert_eq!(b.as_bytes(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(b.get_pixel(0, 0), Some(Color::rgba(1, 2, 3, 4)));
        assert_eq!(b.get_pixel(1, 0), Some(Color::rgba(5, 6, 7, 8)));
    }

    #[test]
    fn get_pixel_out_of_bounds() {
        let r = ClipboardRegion::new(2, 2, vec![0; 16]);
        assert_eq!(r.get_pixel(2, 0), None);
        assert_eq!(r.get_pixel(0, 2), None);
        assert_eq!(r.get_pixel(usize::MAX, 0), None);
        assert_eq!(r.get_pixel(0, usize::MAX), None);
        assert_eq!(r.get_pixel(1, 1), Some(Color::TRANSPARENT));
    }

    #[test]
    fn empty_region() {
        let r = ClipboardRegion::new(0, 0, Vec::new());
        assert!(r.is_empty());
        assert_eq!(r.pixels().len(), 0);
        assert_eq!(r.get_pixel(0, 0), None);
        let r2 = ClipboardRegion::new(0, 5, Vec::new());
        assert!(r2.is_empty());
        let r3 = ClipboardRegion::new(5, 0, Vec::new());
        assert!(r3.is_empty());
    }

    #[test]
    fn region_to_buffer_to_region_roundtrip() {
        let mut b = PixelBuffer::new(3, 3);
        for y in 0..3 {
            for x in 0..3 {
                b.set_pixel(x, y, Color::rgba(x as u8 * 10, y as u8 * 10, 7, 255));
            }
        }
        let r = ClipboardRegion::from_pixel_buffer(&b);
        let b2 = r.to_pixel_buffer();
        assert_eq!(b2.as_bytes(), b.as_bytes());
        let r2 = ClipboardRegion::from_pixel_buffer(&b2);
        assert_eq!(r2, r);
    }

    // --- G2: Selection -> RGBA / PNG bridge -------------------------------

    fn layer_buffer(w: usize, h: usize, buf: Vec<u8>) -> LayerBuffer {
        LayerBuffer {
            layer_id: 0,
            w,
            h,
            buf,
        }
    }

    /// `w × h` RGBA8 fixture with a distinct color per pixel (row-major).
    fn filled_pixels(w: usize, h: usize) -> Vec<u8> {
        let mut buf = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                buf.extend_from_slice(&[x as u8, y as u8, (x + y) as u8, 255]);
            }
        }
        buf
    }

    fn pixel_buffer(w: usize, h: usize, pixels: &[u8]) -> PixelBuffer {
        let mut buffer = PixelBuffer::new(w, h);
        buffer.as_bytes_mut().copy_from_slice(pixels);
        buffer
    }

    #[test]
    fn selection_to_rgba_rectangle_copies_region() {
        let pixels = filled_pixels(4, 4);
        let pbuf = pixel_buffer(4, 4, &pixels);
        let sel = Selection::capture(&pbuf, Rect2i::new(1, 1, 2, 2)).unwrap();
        assert!(sel.is_rectangular());
        let buf = layer_buffer(4, 4, pixels.clone());
        let (rgba, w, h) = selection_to_rgba(&buf, &sel);
        assert_eq!((w, h), (2, 2));
        // The clipped 2×2 sub-region, copied verbatim in row-major order.
        let mut expected = Vec::new();
        for y in 1..3usize {
            for x in 1..3usize {
                let i = (y * 4 + x) * 4;
                expected.extend_from_slice(&pixels[i..i + 4]);
            }
        }
        assert_eq!(rgba, expected);
    }

    #[test]
    fn selection_to_rgba_bitmap_masks_outside_to_transparent() {
        let pixels = filled_pixels(3, 3);
        let pbuf = pixel_buffer(3, 3, &pixels);
        // Hole at the centre cell (index 4).
        let mask = vec![true, true, true, true, false, true, true, true, true];
        let sel = Selection::capture_mask(&pbuf, Rect2i::new(0, 0, 3, 3), mask).unwrap();
        assert!(!sel.is_rectangular());
        let buf = layer_buffer(3, 3, pixels.clone());
        let (rgba, w, h) = selection_to_rgba(&buf, &sel);
        assert_eq!((w, h), (3, 3));
        for y in 0..3usize {
            for x in 0..3usize {
                let i = (y * 3 + x) * 4;
                let got = &rgba[i..i + 4];
                if (x, y) == (1, 1) {
                    assert_eq!(got, [0, 0, 0, 0], "the mask hole must be transparent");
                } else {
                    assert_eq!(got, &pixels[i..i + 4], "selected pixel copied verbatim");
                }
            }
        }
    }

    #[test]
    fn selection_to_rgba_clips_to_buffer() {
        let sel_pixels = filled_pixels(4, 4);
        let pbuf = pixel_buffer(4, 4, &sel_pixels);
        // Selection over the 4×4 canvas at (2,2,2,2) ...
        let sel = Selection::capture(&pbuf, Rect2i::new(2, 2, 2, 2)).unwrap();
        // ... but the layer buffer is only 3×3, so it clips to a single pixel.
        let buf_pixels = filled_pixels(3, 3);
        let buf = layer_buffer(3, 3, buf_pixels.clone());
        let (rgba, w, h) = selection_to_rgba(&buf, &sel);
        assert_eq!((w, h), (1, 1));
        let i = (2 * 3 + 2) * 4;
        assert_eq!(rgba, buf_pixels[i..i + 4].to_vec());
    }

    #[test]
    fn selection_to_rgba_empty_when_no_intersection() {
        let pixels = filled_pixels(4, 4);
        let pbuf = pixel_buffer(4, 4, &pixels);
        let sel = Selection::capture(&pbuf, Rect2i::new(2, 2, 2, 2)).unwrap();
        // A 2×2 buffer entirely to the upper-left of the selection.
        let buf = layer_buffer(2, 2, filled_pixels(2, 2));
        let (rgba, w, h) = selection_to_rgba(&buf, &sel);
        assert!(rgba.is_empty());
        assert_eq!((w, h), (0, 0));
    }

    #[test]
    fn selection_to_rgba_undersized_buffer_is_empty() {
        let pbuf = PixelBuffer::new(2, 2);
        let sel = Selection::capture(&pbuf, Rect2i::new(0, 0, 2, 2)).unwrap();
        // Claims 4×4 but only holds 2×2 worth of bytes → safe empty result.
        let buf = layer_buffer(4, 4, vec![0u8; 2 * 2 * 4]);
        let (rgba, w, h) = selection_to_rgba(&buf, &sel);
        assert!(rgba.is_empty());
        assert_eq!((w, h), (0, 0));
    }

    #[test]
    fn png_encode_decode_roundtrip() {
        // 1×1 opaque.
        let one = [255u8, 0, 0, 255];
        let png = png_encode(&one, 1, 1);
        assert!(!png.is_empty());
        let (rgba, w, h) = png_decode(&png).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, one);

        // Fully transparent.
        let transparent = vec![0u8; 4 * 2 * 4];
        let png = png_encode(&transparent, 4, 2);
        let (rgba, w, h) = png_decode(&png).unwrap();
        assert_eq!((w, h), (4, 2));
        assert_eq!(rgba, transparent);

        // Fully opaque.
        let opaque = vec![200u8; 3 * 3 * 4];
        let png = png_encode(&opaque, 3, 3);
        let (rgba, w, h) = png_decode(&png).unwrap();
        assert_eq!((w, h), (3, 3));
        assert_eq!(rgba, opaque);
    }

    #[test]
    fn png_decode_rejects_garbage() {
        assert!(png_decode(b"not a png").is_err());
        assert!(png_decode(&[]).is_err());
    }

    #[test]
    fn selection_to_rgba_to_png_roundtrip() {
        let pixels = filled_pixels(4, 4);
        let pbuf = pixel_buffer(4, 4, &pixels);
        // Masked selection with two holes so the round-trip carries alpha 0.
        let mask = vec![
            true, true, true, true, true, false, true, true, true, true, true, false, true, true,
            true, true,
        ];
        let sel = Selection::capture_mask(&pbuf, Rect2i::new(0, 0, 4, 4), mask).unwrap();
        let buf = layer_buffer(4, 4, pixels);
        let (rgba, w, h) = selection_to_rgba(&buf, &sel);
        let png = png_encode(&rgba, w, h);
        let (decoded, dw, dh) = png_decode(&png).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(decoded, rgba);
    }
}
