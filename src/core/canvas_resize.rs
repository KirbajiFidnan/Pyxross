//! Pure, pivot-aware canvas resize core.
//!
//! Resizing a canvas changes only the buffer's dimensions: the OLD content is
//! either placed 1:1 at a nine-way [`Pivot`] anchor (grow pads with transparent,
//! shrink crops) or nearest-neighbour resampled to the new size. Everything here
//! is headless and operates on raw straight-alpha RGBA8 bytes — no UI types, no
//! panics on malformed input.
//!
//! [`Pivot`] is defined here, in exactly one place, so the UI resize panels can
//! share it.

use crate::core::transform::scale_nn;

/// Nine-way anchor describing where the OLD content lands in the NEW buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pivot {
    TopLeft,
    TopCenter,
    TopRight,
    MidLeft,
    Center,
    MidRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Pivot {
    /// The horizontal fraction as `numerator / 2` (0 = left, 1 = centre,
    /// 2 = right).
    fn frac_x(self) -> i64 {
        match self {
            Pivot::TopLeft | Pivot::MidLeft | Pivot::BottomLeft => 0,
            Pivot::TopCenter | Pivot::Center | Pivot::BottomCenter => 1,
            Pivot::TopRight | Pivot::MidRight | Pivot::BottomRight => 2,
        }
    }

    /// The vertical fraction as `numerator / 2` (0 = top, 1 = middle,
    /// 2 = bottom).
    fn frac_y(self) -> i64 {
        match self {
            Pivot::TopLeft | Pivot::TopCenter | Pivot::TopRight => 0,
            Pivot::MidLeft | Pivot::Center | Pivot::MidRight => 1,
            Pivot::BottomLeft | Pivot::BottomCenter | Pivot::BottomRight => 2,
        }
    }

    /// Where the old content lands in the new buffer, as a top-left offset
    /// `(dx, dy)`.
    ///
    /// `TopLeft` → `(0, 0)`; `Center` →
    /// `((new_w - old_w) / 2, (new_h - old_h) / 2)` with FLOOR division;
    /// `BottomRight` → `(new_w - old_w, new_h - old_h)`. The offset is negative
    /// when shrinking in the aligned direction (e.g. `Center` on `8 → 4` yields
    /// `(-2, -2)`), meaning the old content is cropped at the top-left.
    pub fn offset(self, old: (u32, u32), new: (u32, u32)) -> (i32, i32) {
        let dw = i64::from(new.0) - i64::from(old.0);
        let dh = i64::from(new.1) - i64::from(old.1);
        // FLOOR division so shrinking stays symmetric (integer division in Rust
        // truncates toward zero, which would bias centre anchors).
        let ox = (dw * self.frac_x()).div_euclid(2);
        let oy = (dh * self.frac_y()).div_euclid(2);
        (ox as i32, oy as i32)
    }
}

/// Places `src` (`w × h` straight-alpha RGBA8) into a new `new_w × new_h` RGBA8
/// buffer.
///
/// * `resample == false`: the content is copied 1:1 at the pivot's
///   [`Pivot::offset`] — NO scaling. Overflow is clipped and any new area is
///   left fully transparent.
/// * `resample == true`: the content is nearest-neighbour scaled (via
///   [`scale_nn`]) to fill the new size. Because a full stretch maps every pivot
///   fraction to itself (`0 → 0`, `½ → ½`, `1 → 1`), pivot alignment is
///   preserved for all nine pivots; the anchor therefore needs no separate
///   application. The `pivot` argument is accepted so both modes share one call
///   shape.
///
/// Degenerate input never panics: a zero `w`/`h`/`new_w`/`new_h` or a `src`
/// whose length is not `w * h * 4` yields a fully transparent new buffer (and a
/// zero new dimension yields an empty `Vec`). A byte-count overflow also yields
/// an empty `Vec`.
pub fn resize_pixels(
    src: &[u8],
    w: u32,
    h: u32,
    new_w: u32,
    new_h: u32,
    pivot: Pivot,
    resample: bool,
) -> Vec<u8> {
    let new_len = (new_w as usize)
        .checked_mul(new_h as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    let Some(new_len) = new_len else {
        return Vec::new();
    };
    // New buffer starts fully transparent; grow areas stay that way.
    let mut out = vec![0u8; new_len];
    if w == 0 || h == 0 || new_w == 0 || new_h == 0 {
        return out;
    }
    let Some(src_len) = (w as usize)
        .checked_mul(h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return out;
    };
    // Malformed buffer: stay safe rather than panic (transparent new buffer).
    if src.len() != src_len {
        return out;
    }

    if resample {
        return scale_nn(src, w as usize, h as usize, new_w as usize, new_h as usize);
    }

    // 1:1 pivot placement: every old pixel lands at its offset position, with
    // clipping on both sides (negative offset crops the leading edge; a too-far
    // positive offset clips the trailing edge).
    let (ox, oy) = pivot.offset((w, h), (new_w, new_h));
    let new_w_i = i64::from(new_w);
    let new_h_i = i64::from(new_h);
    for sy in 0..h as i64 {
        let dy = sy + i64::from(oy);
        if dy < 0 || dy >= new_h_i {
            continue;
        }
        for sx in 0..w as i64 {
            let dx = sx + i64::from(ox);
            if dx < 0 || dx >= new_w_i {
                continue;
            }
            let s = ((sy as usize * w as usize) + sx as usize) * 4;
            let d = ((dy as usize * new_w as usize) + dx as usize) * 4;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    out
}

/// The canvas size implied by a `region_w × region_h` frame repeated
/// `count_w × count_h` times, using saturating multiplication (never panics).
pub fn canvas_size(region_w: u32, region_h: u32, count_w: u32, count_h: u32) -> (u32, u32) {
    (region_w.saturating_mul(count_w), region_h.saturating_mul(count_h))
}

/// Resizes each `(old_tile.0 × old_tile.1)` region in a `count.0 × count.1`
/// grid to `(new_tile.0 × new_tile.1)` in place within its own cell, returning
/// a `(new_tile.0 * count.0) × (new_tile.1 * count.1)` straight-alpha RGBA8
/// buffer.
///
/// Unlike [`resize_pixels`], which treats the whole canvas as a single image,
/// this keeps every region's pixels inside that region's new cell:
///
/// * `resample == false`: within each cell `(col, row)`, the old region's
///   pixels are placed 1:1 at [`Pivot::offset(old_tile, new_tile)`] relative
///   to the new cell's top-left corner (grow pads that cell with transparent;
///   shrink crops to that cell without bleeding into neighbours).
/// * `resample == true`: within each cell `(col, row)`, the old region's
///   pixels are nearest-neighbour scaled (via [`scale_nn`]) to `new_tile`.
#[allow(clippy::too_many_arguments)]
pub fn resize_regions_pixels(
    src: &[u8],
    old_w: u32,
    old_h: u32,
    old_tile: (u32, u32),
    new_tile: (u32, u32),
    count: (u32, u32),
    pivot: Pivot,
    resample: bool,
) -> Vec<u8> {
    let Some(new_w) = new_tile.0.checked_mul(count.0) else {
        return Vec::new();
    };
    let Some(new_h) = new_tile.1.checked_mul(count.1) else {
        return Vec::new();
    };
    let Some(new_len) = (new_w as usize)
        .checked_mul(new_h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return Vec::new();
    };
    let mut out = vec![0u8; new_len];
    if old_w == 0
        || old_h == 0
        || old_tile.0 == 0
        || old_tile.1 == 0
        || new_tile.0 == 0
        || new_tile.1 == 0
        || count.0 == 0
        || count.1 == 0
    {
        return out;
    }
    let Some(src_len) = (old_w as usize)
        .checked_mul(old_h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return out;
    };
    if src.len() != src_len {
        return out;
    }

    let old_stride = old_w as usize * 4;
    let new_stride = new_w as usize * 4;

    if resample {
        let Some(cell_len) = (old_tile.0 as usize)
            .checked_mul(old_tile.1 as usize)
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            return out;
        };
        let mut cell_src = vec![0u8; cell_len];
        let cell_stride = old_tile.0 as usize * 4;
        let scaled_stride = new_tile.0 as usize * 4;
        for row in 0..count.1 {
            let sy0 = row.saturating_mul(old_tile.1);
            let dy0 = row.saturating_mul(new_tile.1);
            let copy_h = old_tile.1.min(old_h.saturating_sub(sy0)) as usize;
            for col in 0..count.0 {
                let sx0 = col.saturating_mul(old_tile.0);
                let dx0 = col.saturating_mul(new_tile.0);
                let copy_w = old_tile.0.min(old_w.saturating_sub(sx0)) as usize;
                cell_src.fill(0);
                if copy_w > 0 && copy_h > 0 {
                    let row_bytes = copy_w * 4;
                    for ly in 0..copy_h {
                        let s_off = (sy0 as usize + ly) * old_stride + sx0 as usize * 4;
                        let c_off = ly * cell_stride;
                        cell_src[c_off..c_off + row_bytes]
                            .copy_from_slice(&src[s_off..s_off + row_bytes]);
                    }
                }
                let scaled = scale_nn(
                    &cell_src,
                    old_tile.0 as usize,
                    old_tile.1 as usize,
                    new_tile.0 as usize,
                    new_tile.1 as usize,
                );
                if scaled.len() == scaled_stride * new_tile.1 as usize {
                    for dy in 0..new_tile.1 as usize {
                        let s_off = dy * scaled_stride;
                        let d_off = (dy0 as usize + dy) * new_stride + dx0 as usize * 4;
                        out[d_off..d_off + scaled_stride]
                            .copy_from_slice(&scaled[s_off..s_off + scaled_stride]);
                    }
                }
            }
        }
        return out;
    }

    let (ox, oy) = pivot.offset(old_tile, new_tile);
    let ox = i64::from(ox);
    let oy = i64::from(oy);
    let new_tw_i = i64::from(new_tile.0);
    let new_th_i = i64::from(new_tile.1);

    for row in 0..count.1 {
        let sy0 = row.saturating_mul(old_tile.1);
        let dy0 = row.saturating_mul(new_tile.1);
        let max_ly = i64::from(old_tile.1.min(old_h.saturating_sub(sy0)));
        let ly_start = (-oy).max(0);
        let ly_end = (new_th_i - oy).min(max_ly);
        if ly_start >= ly_end {
            continue;
        }
        for col in 0..count.0 {
            let sx0 = col.saturating_mul(old_tile.0);
            let dx0 = col.saturating_mul(new_tile.0);
            let max_lx = i64::from(old_tile.0.min(old_w.saturating_sub(sx0)));
            let lx_start = (-ox).max(0);
            let lx_end = (new_tw_i - ox).min(max_lx);
            if lx_start >= lx_end {
                continue;
            }
            let row_bytes = (lx_end - lx_start) as usize * 4;
            let sx_start = (i64::from(sx0) + lx_start) as usize;
            let dx_start = (i64::from(dx0) + lx_start + ox) as usize;
            for ly in ly_start..ly_end {
                let sy = (i64::from(sy0) + ly) as usize;
                let dy = (i64::from(dy0) + ly + oy) as usize;
                let s_off = sy * old_stride + sx_start * 4;
                let d_off = dy * new_stride + dx_start * 4;
                out[d_off..d_off + row_bytes].copy_from_slice(&src[s_off..s_off + row_bytes]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `w × h` buffer where pixel `(x, y)` has the distinct opaque
    /// colour `(x*40+1, y*40+2, 200, 255)`.
    fn fixture(w: u32, h: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                buf.extend_from_slice(&[(x * 40 + 1) as u8, (y * 40 + 2) as u8, 200, 255]);
            }
        }
        buf
    }

    fn px(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * w + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn transparent() -> [u8; 4] {
        [0, 0, 0, 0]
    }

    const ALL_PIVOTS: [Pivot; 9] = [
        Pivot::TopLeft,
        Pivot::TopCenter,
        Pivot::TopRight,
        Pivot::MidLeft,
        Pivot::Center,
        Pivot::MidRight,
        Pivot::BottomLeft,
        Pivot::BottomCenter,
        Pivot::BottomRight,
    ];

    // -----------------------------------------------------------------------
    // Pivot::offset
    // -----------------------------------------------------------------------

    #[test]
    fn pivot_offsets_grow_four_to_eight() {
        let old = (4, 4);
        let new = (8, 8);
        assert_eq!(Pivot::TopLeft.offset(old, new), (0, 0));
        assert_eq!(Pivot::TopCenter.offset(old, new), (2, 0));
        assert_eq!(Pivot::TopRight.offset(old, new), (4, 0));
        assert_eq!(Pivot::MidLeft.offset(old, new), (0, 2));
        assert_eq!(Pivot::Center.offset(old, new), (2, 2));
        assert_eq!(Pivot::MidRight.offset(old, new), (4, 2));
        assert_eq!(Pivot::BottomLeft.offset(old, new), (0, 4));
        assert_eq!(Pivot::BottomCenter.offset(old, new), (2, 4));
        assert_eq!(Pivot::BottomRight.offset(old, new), (4, 4));
    }

    #[test]
    fn pivot_offsets_shrink_are_negative_and_floored() {
        // 8 → 4: centre halves must be negative (content crops at top-left).
        let old = (8, 8);
        let new = (4, 4);
        assert_eq!(Pivot::TopLeft.offset(old, new), (0, 0));
        assert_eq!(Pivot::Center.offset(old, new), (-2, -2));
        assert_eq!(Pivot::BottomRight.offset(old, new), (-4, -4));
        assert_eq!(Pivot::TopRight.offset(old, new), (-4, 0));
        assert_eq!(Pivot::BottomLeft.offset(old, new), (0, -4));

        // Odd delta → FLOOR division (symmetric centre), not truncation.
        assert_eq!(Pivot::Center.offset((2, 2), (5, 5)), (1, 1)); // floor(3/2)
        assert_eq!(Pivot::Center.offset((5, 5), (2, 2)), (-2, -2)); // floor(-3/2)
        assert_eq!(Pivot::BottomRight.offset((2, 2), (5, 5)), (3, 3));
        assert_eq!(Pivot::BottomRight.offset((5, 5), (2, 2)), (-3, -3));
    }

    #[test]
    fn pivot_offsets_are_zero_when_size_is_unchanged() {
        for p in ALL_PIVOTS {
            assert_eq!(p.offset((6, 9), (6, 9)), (0, 0));
        }
    }

    // -----------------------------------------------------------------------
    // resize_pixels — no resample
    // -----------------------------------------------------------------------

    #[test]
    fn resize_no_resample_grow_pads_and_places_at_pivot() {
        let src = fixture(2, 2);
        // TopLeft: content at (0,0)..(1,1), rest transparent.
        let out = resize_pixels(&src, 2, 2, 4, 4, Pivot::TopLeft, false);
        assert_eq!(out.len(), 4 * 4 * 4);
        assert_eq!(px(&out, 4, 0, 0), px(&src, 2, 0, 0));
        assert_eq!(px(&out, 4, 1, 1), px(&src, 2, 1, 1));
        assert_eq!(px(&out, 4, 2, 0), transparent());
        assert_eq!(px(&out, 4, 0, 2), transparent());

        // Center: offset (1,1).
        let centered = resize_pixels(&src, 2, 2, 4, 4, Pivot::Center, false);
        assert_eq!(px(&centered, 4, 0, 0), transparent());
        assert_eq!(px(&centered, 4, 1, 1), px(&src, 2, 0, 0));
        assert_eq!(px(&centered, 4, 2, 2), px(&src, 2, 1, 1));

        // BottomRight: offset (2,2).
        let br = resize_pixels(&src, 2, 2, 4, 4, Pivot::BottomRight, false);
        assert_eq!(px(&br, 4, 2, 2), px(&src, 2, 0, 0));
        assert_eq!(px(&br, 4, 3, 3), px(&src, 2, 1, 1));
        assert_eq!(px(&br, 4, 1, 1), transparent());
    }

    #[test]
    fn resize_no_resample_shrink_crops_at_pivot() {
        let src = fixture(4, 4);
        // TopLeft keeps the top-left 2×2.
        let tl = resize_pixels(&src, 4, 4, 2, 2, Pivot::TopLeft, false);
        assert_eq!(px(&tl, 2, 0, 0), px(&src, 4, 0, 0));
        assert_eq!(px(&tl, 2, 1, 1), px(&src, 4, 1, 1));

        // Center offset (-1,-1): the new top-left samples old (1,1).
        let center = resize_pixels(&src, 4, 4, 2, 2, Pivot::Center, false);
        assert_eq!(px(&center, 2, 0, 0), px(&src, 4, 1, 1));
        assert_eq!(px(&center, 2, 1, 1), px(&src, 4, 2, 2));

        // BottomRight offset (-2,-2): the new top-left samples old (2,2).
        let br = resize_pixels(&src, 4, 4, 2, 2, Pivot::BottomRight, false);
        assert_eq!(px(&br, 2, 0, 0), px(&src, 4, 2, 2));
        assert_eq!(px(&br, 2, 1, 1), px(&src, 4, 3, 3));
    }

    #[test]
    fn resize_no_resample_identity_is_verbatim() {
        for p in ALL_PIVOTS {
            let src = fixture(3, 5);
            let out = resize_pixels(&src, 3, 5, 3, 5, p, false);
            assert_eq!(out, src, "identity resize must be byte-identical for {p:?}");
        }
    }

    // -----------------------------------------------------------------------
    // resize_pixels — resample (nearest-neighbour)
    // -----------------------------------------------------------------------

    #[test]
    fn resize_resample_upscale_2x_repeats_blocks() {
        let src = fixture(2, 2);
        let out = resize_pixels(&src, 2, 2, 4, 4, Pivot::TopLeft, true);
        assert_eq!(out.len(), 4 * 4 * 4);
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(px(&out, 4, x, y), px(&src, 2, 0, 0));
        }
        for (x, y) in [(2, 0), (3, 0), (2, 1), (3, 1)] {
            assert_eq!(px(&out, 4, x, y), px(&src, 2, 1, 0));
        }
        assert_eq!(px(&out, 4, 3, 3), px(&src, 2, 1, 1));
    }

    #[test]
    fn resize_resample_downscale_half_samples_nearest() {
        let src = fixture(4, 4);
        let out = resize_pixels(&src, 4, 4, 2, 2, Pivot::TopLeft, true);
        assert_eq!(out.len(), 2 * 2 * 4);
        // sx = dx * 4 / 2 → 0, 2; no blending, exact source pixels.
        assert_eq!(px(&out, 2, 0, 0), px(&src, 4, 0, 0));
        assert_eq!(px(&out, 2, 1, 0), px(&src, 4, 2, 0));
        assert_eq!(px(&out, 2, 0, 1), px(&src, 4, 0, 2));
        assert_eq!(px(&out, 2, 1, 1), px(&src, 4, 2, 2));
    }

    #[test]
    fn resize_resample_identity_is_verbatim() {
        let src = fixture(3, 3);
        let out = resize_pixels(&src, 3, 3, 3, 3, Pivot::Center, true);
        assert_eq!(out, src);
    }

    #[test]
    fn resize_resample_preserves_alignment_for_every_pivot() {
        // A full stretch maps every pivot fraction to itself, so all nine
        // anchors produce the SAME resampled buffer.
        let src = fixture(2, 2);
        let reference = resize_pixels(&src, 2, 2, 5, 5, Pivot::TopLeft, true);
        for p in ALL_PIVOTS {
            assert_eq!(
                resize_pixels(&src, 2, 2, 5, 5, p, true),
                reference,
                "resample must be pivot-invariant under a full stretch ({p:?})"
            );
        }
    }

    #[test]
    fn resize_resample_creates_no_new_colors() {
        let src = fixture(4, 4);
        let out = resize_pixels(&src, 4, 4, 7, 5, Pivot::Center, true);
        let palette: std::collections::HashSet<[u8; 4]> =
            src.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        for c in out.chunks_exact(4) {
            assert!(
                palette.contains(&[c[0], c[1], c[2], c[3]]),
                "resample must not create new colors"
            );
        }
    }

    // -----------------------------------------------------------------------
    // edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn resize_zero_new_dimension_is_empty() {
        let src = fixture(2, 2);
        assert!(resize_pixels(&src, 2, 2, 0, 4, Pivot::Center, false).is_empty());
        assert!(resize_pixels(&src, 2, 2, 4, 0, Pivot::Center, true).is_empty());
    }

    #[test]
    fn resize_empty_source_is_transparent_new_buffer() {
        // Zero old dimensions.
        let out = resize_pixels(&[], 0, 0, 3, 2, Pivot::Center, false);
        assert_eq!(out.len(), 3 * 2 * 4);
        assert!(out.iter().all(|&b| b == 0));
        let out = resize_pixels(&[], 0, 0, 3, 2, Pivot::Center, true);
        assert_eq!(out.len(), 3 * 2 * 4);
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn resize_malformed_source_length_does_not_panic() {
        // Declared 3×3 but only one pixel of data.
        let out = resize_pixels(&[1, 2, 3, 255], 3, 3, 2, 2, Pivot::Center, false);
        assert_eq!(out.len(), 2 * 2 * 4);
        assert!(out.iter().all(|&b| b == 0));
        let out = resize_pixels(&[1, 2, 3, 255], 3, 3, 2, 2, Pivot::Center, true);
        assert_eq!(out.len(), 2 * 2 * 4);
        assert!(out.iter().all(|&b| b == 0));
    }

    // -----------------------------------------------------------------------
    // canvas_size
    // -----------------------------------------------------------------------

    #[test]
    fn canvas_size_multiplies_region_by_count() {
        assert_eq!(canvas_size(16, 16, 4, 2), (64, 32));
        assert_eq!(canvas_size(8, 12, 1, 1), (8, 12));
        assert_eq!(canvas_size(16, 16, 0, 5), (0, 80));
    }

    #[test]
    fn canvas_size_saturates_instead_of_overflowing() {
        assert_eq!(canvas_size(u32::MAX, 1, 2, 1), (u32::MAX, 1));
        assert_eq!(canvas_size(1, u32::MAX, 1, 3), (1, u32::MAX));
    }

    // -----------------------------------------------------------------------
    // resize_regions_pixels
    // -----------------------------------------------------------------------

    #[test]
    fn resize_regions_keeps_each_region_pixels_in_own_cell_for_all_pivots() {
        // 2x2 grid of 2x2 regions -> 4x4 canvas.
        // Put a distinct pixel at local (0, 0) of each region:
        // (0,0)->(10,0,0,255), (1,0)->(20,0,0,255), (0,1)->(30,0,0,255), (1,1)->(40,0,0,255).
        let mut src = vec![0u8; 4 * 4 * 4];
        let set = |buf: &mut [u8], x: u32, y: u32, c: [u8; 4]| {
            let i = ((y * 4 + x) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&c);
        };
        let c00 = [10, 0, 0, 255];
        let c10 = [20, 0, 0, 255];
        let c01 = [30, 0, 0, 255];
        let c11 = [40, 0, 0, 255];
        set(&mut src, 0, 0, c00);
        set(&mut src, 2, 0, c10);
        set(&mut src, 0, 2, c01);
        set(&mut src, 2, 2, c11);

        // Grow each region 2x2 -> 4x4 with Center pivot -> 8x8 canvas.
        // Within each 4x4 cell, old 2x2 is centered at local offset (1, 1).
        let out = resize_regions_pixels(&src, 4, 4, (2, 2), (4, 4), (2, 2), Pivot::Center, false);
        assert_eq!(out.len(), 8 * 8 * 4);
        assert_eq!(px(&out, 8, 1, 1), c00, "region (0,0) centered in 0..4 x 0..4");
        assert_eq!(px(&out, 8, 5, 1), c10, "region (1,0) centered in 4..8 x 0..4");
        assert_eq!(px(&out, 8, 1, 5), c01, "region (0,1) centered in 0..4 x 4..8");
        assert_eq!(px(&out, 8, 5, 5), c11, "region (1,1) centered in 4..8 x 4..8");
        // Borders of each cell are transparent.
        assert_eq!(px(&out, 8, 0, 0), transparent());
        assert_eq!(px(&out, 8, 4, 0), transparent());

        // TopLeft pivot: local offset (0, 0), expands right and bottom.
        let tl = resize_regions_pixels(&src, 4, 4, (2, 2), (4, 4), (2, 2), Pivot::TopLeft, false);
        assert_eq!(px(&tl, 8, 0, 0), c00);
        assert_eq!(px(&tl, 8, 4, 0), c10);
        assert_eq!(px(&tl, 8, 0, 4), c01);
        assert_eq!(px(&tl, 8, 4, 4), c11);

        // MidRight pivot: local offset (2, 1), expands left, top, and bottom.
        let mr = resize_regions_pixels(&src, 4, 4, (2, 2), (4, 4), (2, 2), Pivot::MidRight, false);
        assert_eq!(px(&mr, 8, 2, 1), c00);
        assert_eq!(px(&mr, 8, 6, 1), c10);
        assert_eq!(px(&mr, 8, 2, 5), c01);
        assert_eq!(px(&mr, 8, 6, 5), c11);
    }

    #[test]
    fn resize_regions_shrink_clips_per_cell_without_bleeding() {
        // 2x1 grid of 4x4 regions -> 8x4 canvas.
        // Put pixel at (3, 3) of region (0, 0) and (4, 0) of region (1, 0).
        let mut src = vec![0u8; 8 * 4 * 4];
        let set = |buf: &mut [u8], x: u32, y: u32, c: [u8; 4]| {
            let i = ((y * 8 + x) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&c);
        };
        set(&mut src, 3, 3, [99, 0, 0, 255]); // region (0,0) bottom-right
        set(&mut src, 4, 0, [55, 0, 0, 255]); // region (1,0) top-left

        // Shrink 4x4 -> 2x2 with TopLeft pivot -> 4x2 canvas.
        // Region (0,0)'s (3,3) is cropped; region (1,0)'s (0,0) lands at (2,0).
        let out = resize_regions_pixels(&src, 8, 4, (4, 4), (2, 2), (2, 1), Pivot::TopLeft, false);
        assert_eq!(px(&out, 4, 0, 0), transparent());
        assert_eq!(px(&out, 4, 1, 1), transparent());
        assert_eq!(px(&out, 4, 2, 0), [55, 0, 0, 255]);
    }

    #[test]
    fn resize_regions_resample_scales_each_cell_independently() {
        // 2x1 grid of 2x2 regions -> 4x2 canvas.
        let mut src = vec![0u8; 4 * 2 * 4];
        let set = |buf: &mut [u8], x: u32, y: u32, c: [u8; 4]| {
            let i = ((y * 4 + x) * 4) as usize;
            buf[i..i + 4].copy_from_slice(&c);
        };
        let r0 = [11, 22, 33, 255];
        let r1 = [44, 55, 66, 255];
        set(&mut src, 0, 0, r0);
        set(&mut src, 2, 0, r1);

        // Scale each 2x2 region to 4x4 -> 8x4 canvas.
        let out = resize_regions_pixels(&src, 4, 2, (2, 2), (4, 4), (2, 1), Pivot::Center, true);
        assert_eq!(px(&out, 8, 0, 0), r0);
        assert_eq!(px(&out, 8, 1, 1), r0);
        assert_eq!(px(&out, 8, 2, 0), transparent());
        assert_eq!(px(&out, 8, 4, 0), r1);
        assert_eq!(px(&out, 8, 5, 1), r1);
        assert_eq!(px(&out, 8, 6, 0), transparent());
    }
}
