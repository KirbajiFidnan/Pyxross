//! Nearest-neighbour scaling — strict NN, no interpolation.

use super::{checked_dims, BYTES_PER_PIXEL};

/// Nearest-neighbour scale of an RGBA8 buffer to `target_w × target_h`.
///
/// Every output pixel is an exact copy of one source pixel (the nearest by
/// floor division `sx = dx * w / target_w`), so no new colors are created
/// and alpha is preserved. Works for any target size — integer or
/// fractional factors, upscale or downscale.
///
/// An empty source (`w == 0 || h == 0` or an undersized buffer) produces a
/// fully transparent buffer of the target size; a zero target dimension
/// produces an empty buffer. Byte-count overflow or allocation failure also
/// produces an empty buffer.
pub fn scale_nn(buf: &[u8], w: usize, h: usize, target_w: usize, target_h: usize) -> Vec<u8> {
    if target_w == 0 || target_h == 0 {
        return Vec::new();
    }
    let Some(output_len) = target_w
        .checked_mul(target_h)
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
    else {
        return Vec::new();
    };
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return transparent_buffer(output_len);
    };
    let Some(mut out) = allocated_buffer(output_len) else {
        return Vec::new();
    };
    for dy in 0..target_h {
        let sy = ((dy as u128 * h as u128) / target_h as u128) as usize;
        for dx in 0..target_w {
            let sx = ((dx as u128 * w as u128) / target_w as u128) as usize;
            let src = (sy * w + sx) * BYTES_PER_PIXEL;
            let dst = (dy * target_w + dx) * BYTES_PER_PIXEL;
            out[dst..dst + BYTES_PER_PIXEL].copy_from_slice(&buf[src..src + BYTES_PER_PIXEL]);
        }
    }
    out
}

fn transparent_buffer(len: usize) -> Vec<u8> {
    match allocated_buffer(len) {
        Some(buffer) => buffer,
        None => Vec::new(),
    }
}

fn allocated_buffer(len: usize) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    buffer.try_reserve_exact(len).ok()?;
    buffer.resize(len, 0);
    Some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 fixture, one distinct opaque color per pixel (row-major).
    fn fixture_2x2() -> (Vec<u8>, usize, usize) {
        let colors: [[u8; 4]; 4] = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
        ];
        let mut buf = Vec::with_capacity(4 * 4);
        for c in colors {
            buf.extend_from_slice(&c);
        }
        (buf, 2, 2)
    }

    /// 4×4 fixture, one distinct opaque color per pixel (row-major).
    fn fixture_4x4() -> (Vec<u8>, usize, usize) {
        let mut buf = Vec::with_capacity(4 * 4 * 4);
        for i in 0..16u8 {
            buf.extend_from_slice(&[i * 16, 0, 0, 255]);
        }
        (buf, 4, 4)
    }

    fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn scale_nn_upscale_integer_factor_repeats_blocks() {
        let (buf, w, h) = fixture_2x2();
        let out = scale_nn(&buf, w, h, 4, 4);
        assert_eq!(out.len(), 4 * 4 * 4);
        // Each source pixel becomes a 2×2 block.
        assert_eq!(px(&out, 4, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&out, 4, 1, 0), [255, 0, 0, 255]);
        assert_eq!(px(&out, 4, 0, 1), [255, 0, 0, 255]);
        assert_eq!(px(&out, 4, 1, 1), [255, 0, 0, 255]);
        assert_eq!(px(&out, 4, 2, 0), [0, 255, 0, 255]);
        assert_eq!(px(&out, 4, 3, 0), [0, 255, 0, 255]);
        assert_eq!(px(&out, 4, 2, 2), [255, 255, 0, 255]);
        assert_eq!(px(&out, 4, 3, 3), [255, 255, 0, 255]);
    }

    #[test]
    fn scale_nn_downscale_no_blending() {
        let (buf, w, h) = fixture_4x4();
        let out = scale_nn(&buf, w, h, 2, 2);
        assert_eq!(out.len(), 2 * 2 * 4);
        // Each output pixel equals exactly one source pixel (floor sampling).
        assert_eq!(px(&out, 2, 0, 0), px(&buf, 4, 0, 0));
        assert_eq!(px(&out, 2, 1, 0), px(&buf, 4, 2, 0));
        assert_eq!(px(&out, 2, 0, 1), px(&buf, 4, 0, 2));
        assert_eq!(px(&out, 2, 1, 1), px(&buf, 4, 2, 2));
    }

    #[test]
    fn scale_nn_identity_1_to_1() {
        let (buf, w, h) = fixture_4x4();
        let out = scale_nn(&buf, w, h, w, h);
        assert_eq!(out, buf);
    }

    #[test]
    fn scale_nn_upscale_non_integer_factor() {
        let (buf, w, h) = fixture_2x2();
        let out = scale_nn(&buf, w, h, 3, 3);
        // sx = dx*2/3 → 0, 0, 1; sy likewise.
        assert_eq!(px(&out, 3, 0, 0), [255, 0, 0, 255]);
        assert_eq!(px(&out, 3, 1, 0), [255, 0, 0, 255]);
        assert_eq!(px(&out, 3, 2, 0), [0, 255, 0, 255]);
        assert_eq!(px(&out, 3, 0, 1), [255, 0, 0, 255]);
        assert_eq!(px(&out, 3, 2, 2), [255, 255, 0, 255]);
    }

    #[test]
    fn scale_nn_downscale_odd_dims() {
        let (buf, w, h) = fixture_4x4();
        let out = scale_nn(&buf, w, h, 3, 3);
        // sx = dx*4/3 → 0, 1, 2; sy likewise.
        assert_eq!(px(&out, 3, 0, 0), px(&buf, 4, 0, 0));
        assert_eq!(px(&out, 3, 1, 0), px(&buf, 4, 1, 0));
        assert_eq!(px(&out, 3, 2, 0), px(&buf, 4, 2, 0));
        assert_eq!(px(&out, 3, 0, 1), px(&buf, 4, 0, 1));
        assert_eq!(px(&out, 3, 2, 2), px(&buf, 4, 2, 2));
    }

    #[test]
    fn scale_nn_zero_target_dims_returns_empty() {
        let (buf, w, h) = fixture_2x2();
        assert!(scale_nn(&buf, w, h, 0, 4).is_empty());
        assert!(scale_nn(&buf, w, h, 4, 0).is_empty());
    }

    #[test]
    fn scale_nn_output_length_overflow_returns_empty() {
        assert!(scale_nn(&[], 0, 0, usize::MAX, 2).is_empty());
        assert!(scale_nn(&[0; 4], 1, 1, usize::MAX / 4 + 1, 1).is_empty());
    }

    #[test]
    fn scale_nn_empty_source_transparent_target() {
        let out = scale_nn(&[], 0, 0, 3, 3);
        assert_eq!(out.len(), 3 * 3 * 4);
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn scale_nn_undersized_source_transparent_target() {
        let out = scale_nn(&[0u8; 4], 3, 3, 2, 2);
        assert_eq!(out.len(), 2 * 2 * 4);
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn scale_nn_palette_purity() {
        let (buf, w, h) = fixture_4x4();
        let out = scale_nn(&buf, w, h, 7, 5);
        let mut palette = std::collections::HashSet::new();
        for c in buf.chunks_exact(4) {
            palette.insert([c[0], c[1], c[2], c[3]]);
        }
        for c in out.chunks_exact(4) {
            let p = [c[0], c[1], c[2], c[3]];
            assert!(palette.contains(&p), "new color {p:?} created by scale");
        }
    }

    #[test]
    fn scale_nn_preserves_alpha_verbatim() {
        let mut buf = vec![0u8; 4 * 4 * 4];
        for (i, c) in buf.chunks_exact_mut(4).enumerate() {
            c[0] = 100;
            c[1] = 50;
            c[2] = 25;
            c[3] = if i % 2 == 0 { 0 } else { 255 };
        }
        let out = scale_nn(&buf, 4, 4, 9, 9);
        for c in out.chunks_exact(4) {
            assert!(c[3] == 0 || c[3] == 255, "alpha blended: {}", c[3]);
        }
    }
}
