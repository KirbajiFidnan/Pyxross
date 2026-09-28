//! Pixel-exact transforms — 90° rotations and axis flips.
//!
//! Every output pixel is a verbatim copy of exactly one source pixel
//! (pure index remapping, no interpolation), so the output palette is a
//! subset of the input palette and alpha is preserved bit-for-bit.

use super::{checked_dims, BYTES_PER_PIXEL};

/// Rotates 90° clockwise. Output is `h × w`.
///
/// Round-trip safe: `rotate_90_cw ∘ rotate_90_ccw` is the identity.
pub fn rotate_90_cw(buf: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return (Vec::new(), 0, 0);
    };
    let mut out = vec![0u8; w * h * BYTES_PER_PIXEL];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * BYTES_PER_PIXEL;
            let dst = (x * h + (h - 1 - y)) * BYTES_PER_PIXEL;
            out[dst..dst + BYTES_PER_PIXEL].copy_from_slice(&buf[src..src + BYTES_PER_PIXEL]);
        }
    }
    (out, h, w)
}

/// Rotates 90° counter-clockwise. Output is `h × w`.
///
/// Round-trip safe: `rotate_90_ccw ∘ rotate_90_cw` is the identity.
pub fn rotate_90_ccw(buf: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return (Vec::new(), 0, 0);
    };
    let mut out = vec![0u8; w * h * BYTES_PER_PIXEL];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * BYTES_PER_PIXEL;
            let dst = ((w - 1 - x) * h + y) * BYTES_PER_PIXEL;
            out[dst..dst + BYTES_PER_PIXEL].copy_from_slice(&buf[src..src + BYTES_PER_PIXEL]);
        }
    }
    (out, h, w)
}

/// Rotates 180°. Output dimensions are unchanged (`w × h`).
///
/// Round-trip safe: `rotate_180 ∘ rotate_180` is the identity.
pub fn rotate_180(buf: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return (Vec::new(), 0, 0);
    };
    let mut out = vec![0u8; w * h * BYTES_PER_PIXEL];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * BYTES_PER_PIXEL;
            let dst = ((h - 1 - y) * w + (w - 1 - x)) * BYTES_PER_PIXEL;
            out[dst..dst + BYTES_PER_PIXEL].copy_from_slice(&buf[src..src + BYTES_PER_PIXEL]);
        }
    }
    (out, w, h)
}

/// Horizontal mirror (left ↔ right). Output dimensions are unchanged.
///
/// Involution: `flip_h ∘ flip_h` is the identity.
pub fn flip_h(buf: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return (Vec::new(), 0, 0);
    };
    let mut out = vec![0u8; w * h * BYTES_PER_PIXEL];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * BYTES_PER_PIXEL;
            let dst = (y * w + (w - 1 - x)) * BYTES_PER_PIXEL;
            out[dst..dst + BYTES_PER_PIXEL].copy_from_slice(&buf[src..src + BYTES_PER_PIXEL]);
        }
    }
    (out, w, h)
}

/// Vertical mirror (top ↔ bottom). Output dimensions are unchanged.
///
/// Involution: `flip_v ∘ flip_v` is the identity.
pub fn flip_v(buf: &[u8], w: usize, h: usize) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return (Vec::new(), 0, 0);
    };
    let mut out = vec![0u8; w * h * BYTES_PER_PIXEL];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * BYTES_PER_PIXEL;
            let dst = ((h - 1 - y) * w + x) * BYTES_PER_PIXEL;
            out[dst..dst + BYTES_PER_PIXEL].copy_from_slice(&buf[src..src + BYTES_PER_PIXEL]);
        }
    }
    (out, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3×3 fixture, one distinct color per pixel (row-major).
    fn fixture_3x3() -> (Vec<u8>, usize, usize) {
        let colors: [[u8; 4]; 9] = [
            [10, 0, 0, 255],
            [20, 0, 0, 255],
            [30, 0, 0, 255],
            [0, 40, 0, 255],
            [0, 50, 0, 255],
            [0, 60, 0, 255],
            [0, 0, 70, 255],
            [0, 0, 80, 255],
            [0, 0, 90, 255],
        ];
        let mut buf = Vec::with_capacity(9 * 4);
        for c in colors {
            buf.extend_from_slice(&c);
        }
        (buf, 3, 3)
    }

    /// 4×2 fixture, one distinct color per pixel (row-major).
    fn fixture_4x2() -> (Vec<u8>, usize, usize) {
        let colors: [[u8; 4]; 8] = [
            [1, 0, 0, 255],
            [2, 0, 0, 255],
            [3, 0, 0, 255],
            [4, 0, 0, 255],
            [5, 0, 0, 255],
            [6, 0, 0, 255],
            [7, 0, 0, 255],
            [8, 0, 0, 255],
        ];
        let mut buf = Vec::with_capacity(8 * 4);
        for c in colors {
            buf.extend_from_slice(&c);
        }
        (buf, 4, 2)
    }

    fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn rotate_90_cw_maps_pixels() {
        let (buf, w, h) = fixture_3x3();
        let (out, ow, oh) = rotate_90_cw(&buf, w, h);
        assert_eq!((ow, oh), (3, 3));
        assert_eq!(px(&out, ow, 2, 0), [10, 0, 0, 255]);
        assert_eq!(px(&out, ow, 0, 0), [0, 0, 70, 255]);
        assert_eq!(px(&out, ow, 1, 1), [0, 50, 0, 255]);
        assert_eq!(px(&out, ow, 2, 2), [30, 0, 0, 255]);
        assert_eq!(px(&out, ow, 0, 2), [0, 0, 90, 255]);
    }

    #[test]
    fn rotate_90_cw_swaps_dims() {
        let (buf, w, h) = fixture_4x2();
        let (_, ow, oh) = rotate_90_cw(&buf, w, h);
        assert_eq!((ow, oh), (2, 4));
    }

    #[test]
    fn rotate_90_cw_four_times_is_identity() {
        let (buf, w, h) = fixture_3x3();
        let (r1, w1, h1) = rotate_90_cw(&buf, w, h);
        let (r2, w2, h2) = rotate_90_cw(&r1, w1, h1);
        let (r3, w3, h3) = rotate_90_cw(&r2, w2, h2);
        let (r4, w4, h4) = rotate_90_cw(&r3, w3, h3);
        assert_eq!((w4, h4), (w, h));
        assert_eq!(r4, buf);
    }

    #[test]
    fn rotate_90_ccw_maps_pixels() {
        let (buf, w, h) = fixture_3x3();
        let (out, ow, oh) = rotate_90_ccw(&buf, w, h);
        assert_eq!((ow, oh), (3, 3));
        assert_eq!(px(&out, ow, 0, 0), [30, 0, 0, 255]);
        assert_eq!(px(&out, ow, 2, 0), [0, 0, 90, 255]);
        assert_eq!(px(&out, ow, 0, 2), [10, 0, 0, 255]);
        assert_eq!(px(&out, ow, 2, 2), [0, 0, 70, 255]);
        assert_eq!(px(&out, ow, 1, 1), [0, 50, 0, 255]);
    }

    #[test]
    fn rotate_90_cw_then_ccw_is_identity() {
        let (buf, w, h) = fixture_3x3();
        let (r, rw, rh) = rotate_90_cw(&buf, w, h);
        let (back, bw, bh) = rotate_90_ccw(&r, rw, rh);
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, buf);
    }

    #[test]
    fn rotate_90_ccw_then_cw_is_identity() {
        let (buf, w, h) = fixture_3x3();
        let (r, rw, rh) = rotate_90_ccw(&buf, w, h);
        let (back, bw, bh) = rotate_90_cw(&r, rw, rh);
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, buf);
    }

    #[test]
    fn rotate_180_maps_pixels() {
        let (buf, w, h) = fixture_3x3();
        let (out, ow, oh) = rotate_180(&buf, w, h);
        assert_eq!((ow, oh), (3, 3));
        assert_eq!(px(&out, ow, 0, 0), [0, 0, 90, 255]);
        assert_eq!(px(&out, ow, 2, 2), [10, 0, 0, 255]);
        assert_eq!(px(&out, ow, 1, 1), [0, 50, 0, 255]);
        assert_eq!(px(&out, ow, 1, 2), [20, 0, 0, 255]);
        assert_eq!(px(&out, ow, 2, 1), [0, 40, 0, 255]);
    }

    #[test]
    fn rotate_180_twice_is_identity() {
        let (buf, w, h) = fixture_3x3();
        let (r, rw, rh) = rotate_180(&buf, w, h);
        let (back, bw, bh) = rotate_180(&r, rw, rh);
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, buf);
    }

    #[test]
    fn rotate_90_cw_twice_equals_rotate_180_non_square() {
        let (buf, w, h) = fixture_4x2();
        let (r1, w1, h1) = rotate_90_cw(&buf, w, h);
        let (r2, w2, h2) = rotate_90_cw(&r1, w1, h1);
        let (r180, w180, h180) = rotate_180(&buf, w, h);
        assert_eq!((w2, h2), (w180, h180));
        assert_eq!(r2, r180);
    }

    #[test]
    fn flip_h_maps_pixels() {
        let (buf, w, h) = fixture_3x3();
        let (out, ow, oh) = flip_h(&buf, w, h);
        assert_eq!((ow, oh), (3, 3));
        assert_eq!(px(&out, ow, 2, 0), [10, 0, 0, 255]);
        assert_eq!(px(&out, ow, 0, 0), [30, 0, 0, 255]);
        assert_eq!(px(&out, ow, 1, 1), [0, 50, 0, 255]);
    }

    #[test]
    fn flip_h_involution() {
        let (buf, w, h) = fixture_3x3();
        let (f, fw, fh) = flip_h(&buf, w, h);
        let (back, bw, bh) = flip_h(&f, fw, fh);
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, buf);
    }

    #[test]
    fn flip_v_maps_pixels() {
        let (buf, w, h) = fixture_3x3();
        let (out, ow, oh) = flip_v(&buf, w, h);
        assert_eq!((ow, oh), (3, 3));
        assert_eq!(px(&out, ow, 0, 2), [10, 0, 0, 255]);
        assert_eq!(px(&out, ow, 0, 0), [0, 0, 70, 255]);
        assert_eq!(px(&out, ow, 1, 1), [0, 50, 0, 255]);
    }

    #[test]
    fn flip_v_involution() {
        let (buf, w, h) = fixture_3x3();
        let (f, fw, fh) = flip_v(&buf, w, h);
        let (back, bw, bh) = flip_v(&f, fw, fh);
        assert_eq!((bw, bh), (w, h));
        assert_eq!(back, buf);
    }

    #[test]
    fn flip_h_then_flip_v_equals_rotate_180() {
        let (buf, w, h) = fixture_4x2();
        let (fh, fw, fh_h) = flip_h(&buf, w, h);
        let (fhv, fw2, fh2) = flip_v(&fh, fw, fh_h);
        let (r180, rw, rh) = rotate_180(&buf, w, h);
        assert_eq!((fw2, fh2), (rw, rh));
        assert_eq!(fhv, r180);
    }

    #[test]
    fn rotate_90_cw_commutes_with_flip_on_square() {
        let (buf, w, h) = fixture_3x3();
        let (fh, fw, fh_h) = flip_h(&buf, w, h);
        let (a, aw, ah) = rotate_90_cw(&fh, fw, fh_h);
        let (r, rw, rh) = rotate_90_cw(&buf, w, h);
        let (b, bw, bh) = flip_v(&r, rw, rh);
        assert_eq!((aw, ah), (bw, bh));
        assert_eq!(a, b);
    }

    #[test]
    fn empty_buffer_returns_empty() {
        let (out, ow, oh) = rotate_90_cw(&[], 0, 0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
        let (out, ow, oh) = flip_h(&[], 0, 0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
    }

    #[test]
    fn undersized_buffer_returns_empty() {
        let (out, ow, oh) = rotate_180(&[0u8; 8], 3, 3);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
    }
}
