//! PyxRot — Crisp, low-deformation pixel-art rotation.
//!
//! Combines center-weighted `4×4` subpixel coverage (without EPX corner
//! rounding/blobbing) with a deterministic pixel-perfect L-corner de-staircase
//! pass and 1px stroke continuity, keeping lines sharp and palette-pure.

use super::{checked_dims, rot_sprite::rotated_bounds, BYTES_PER_PIXEL};

type Rgba = [u8; 4];
const TRANSPARENT: Rgba = [0, 0, 0, 0];

/// Rotates an RGBA8 buffer clockwise by `angle_deg` using PyxRot.
pub fn pyx_rot_rotate(src: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(src, w, h) else {
        return (Vec::new(), 0, 0);
    };
    if !angle_deg.is_finite() {
        return (Vec::new(), 0, 0);
    }
    let normalized = angle_deg.rem_euclid(360.0);
    if normalized == 0.0 {
        return (src[..w * h * BYTES_PER_PIXEL].to_vec(), w, h);
    }
    if normalized == 90.0 {
        return super::exact::rotate_90_cw(src, w, h);
    }
    if normalized == 180.0 {
        return super::exact::rotate_180(src, w, h);
    }
    if normalized == 270.0 {
        return super::exact::rotate_90_ccw(src, w, h);
    }

    let (ow, oh) = rotated_bounds(w, h, angle_deg);
    if ow == 0 || oh == 0 {
        return (Vec::new(), 0, 0);
    }

    let rad = (angle_deg as f64).to_radians();
    let cos_a = rad.cos();
    let sin_a = rad.sin();
    let cx_src = w as f64 * 0.5;
    let cy_src = h as f64 * 0.5;
    let cx_dst = ow as f64 * 0.5;
    let cy_dst = oh as f64 * 0.5;

    let mut out = vec![0u8; ow * oh * BYTES_PER_PIXEL];
    let mut conf = vec![0u8; ow * oh];

    // Pass 1: Center-weighted 4x4 stratified subpixel coverage.
    // Inner 2x2 subpixels get weight 2, outer 12 subpixels get weight 1 (total 20).
    for oy in 0..oh {
        for ox in 0..ow {
            let mut candidates: [(Rgba, u8); 16] = [(TRANSPARENT, 0); 16];
            let mut cand_len = 0usize;

            for sy in 0..4 {
                let dy = (oy as f64 + (sy as f64 + 0.5) * 0.25) - cy_dst;
                for sx in 0..4 {
                    let dx = (ox as f64 + (sx as f64 + 0.5) * 0.25) - cx_dst;
                    let rx = dx * cos_a + dy * sin_a + cx_src;
                    let ry = -dx * sin_a + dy * cos_a + cy_src;

                    let color = if rx >= 0.0 && ry >= 0.0 {
                        let ix = rx.floor() as usize;
                        let iy = ry.floor() as usize;
                        if ix < w && iy < h {
                            let idx = (iy * w + ix) * BYTES_PER_PIXEL;
                            [src[idx], src[idx + 1], src[idx + 2], src[idx + 3]]
                        } else {
                            TRANSPARENT
                        }
                    } else {
                        TRANSPARENT
                    };

                    let is_inner = (1..=2).contains(&sx) && (1..=2).contains(&sy);
                    let weight: u8 = if is_inner { 2 } else { 1 };

                    if let Some(slot) = candidates[..cand_len].iter_mut().find(|(c, _)| *c == color)
                    {
                        slot.1 = slot.1.saturating_add(weight);
                    } else {
                        candidates[cand_len] = (color, weight);
                        cand_len += 1;
                    }
                }
            }

            let mut best_color = TRANSPARENT;
            let mut best_weight = 0u8;
            for &(color, weight) in &candidates[..cand_len] {
                if weight > best_weight || (weight == best_weight && color[3] > best_color[3]) {
                    best_weight = weight;
                    best_color = color;
                }
            }

            let dst_idx = (oy * ow + ox) * BYTES_PER_PIXEL;
            out[dst_idx..dst_idx + 4].copy_from_slice(&best_color);
            conf[oy * ow + ox] = best_weight;
        }
    }

    // Pass 2: Crisp L-corner de-staircase cleanup on low-confidence fringe pixels
    // so 1px diagonal lines don't bloat into 2px staircases.
    if ow >= 3 && oh >= 3 {
        let snap = out.clone();
        let px_at = |x: usize, y: usize| -> Rgba {
            let i = (y * ow + x) * BYTES_PER_PIXEL;
            [snap[i], snap[i + 1], snap[i + 2], snap[i + 3]]
        };
        for y in 1..oh - 1 {
            for x in 1..ow - 1 {
                let c = px_at(x, y);
                if c[3] == 0 || conf[y * ow + x] > 9 {
                    continue;
                }
                let n = px_at(x, y - 1);
                let s = px_at(x, y + 1);
                let w_px = px_at(x - 1, y);
                let e = px_at(x + 1, y);

                let is_l_corner = (n == c && e == c && s[3] == 0 && w_px[3] == 0 && px_at(x + 1, y - 1)[3] == 0)
                    || (n == c && w_px == c && s[3] == 0 && e[3] == 0 && px_at(x - 1, y - 1)[3] == 0)
                    || (s == c && e == c && n[3] == 0 && w_px[3] == 0 && px_at(x + 1, y + 1)[3] == 0)
                    || (s == c && w_px == c && n[3] == 0 && e[3] == 0 && px_at(x - 1, y + 1)[3] == 0);
                if is_l_corner {
                    let i = (y * ow + x) * BYTES_PER_PIXEL;
                    out[i..i + 4].copy_from_slice(&TRANSPARENT);
                }
            }
        }
    }

    (out, ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_angles_match_exact_paths() {
        let buf = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        assert_eq!(pyx_rot_rotate(&buf, 2, 2, 0.0), (buf.clone(), 2, 2));
        assert_eq!(
            pyx_rot_rotate(&buf, 2, 2, 90.0),
            super::super::exact::rotate_90_cw(&buf, 2, 2)
        );
    }

    #[test]
    fn palette_purity_and_determinism_on_free_angle() {
        let mut buf = vec![0u8; 8 * 8 * 4];
        let red = [220u8, 40, 60, 255];
        for i in 1..7 {
            let idx = (i * 8 + 4) * 4;
            buf[idx..idx + 4].copy_from_slice(&red);
        }
        let (a, aw, ah) = pyx_rot_rotate(&buf, 8, 8, 27.0);
        let (b, bw, bh) = pyx_rot_rotate(&buf, 8, 8, 27.0);
        assert_eq!((a.clone(), aw, ah), (b, bw, bh));
        for px in a.chunks_exact(4) {
            assert!(px == red || px == [0, 0, 0, 0], "unexpected color {px:?}");
        }
        assert!(a.chunks_exact(4).any(|px| px == red));
    }
}
