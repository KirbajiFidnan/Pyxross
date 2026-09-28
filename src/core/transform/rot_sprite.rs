//! RotSprite-style free-angle rotation over RGBA8 buffers.
//!
//! The public [`rotate`] keeps the historical contract: the output is the
//! axis-aligned bounding box of the rotated rect, exact 0/90/180/270° angles
//! dispatch to the pixel-exact paths (bit-identical to the pre-RotSprite
//! implementation), and every output pixel is a verbatim copy of one input
//! pixel — palette-pure, no anti-aliasing, alpha bit-for-bit.
//!
//! For every other angle the core is RotSprite (Xenowhirl):
//!
//! 1. **Adaptive `S×` upscale with Scale2x / EPX** — `S ∈ {8, 16}` is chosen
//!    from the angle (`S = 16` in the 45° worst-skew band, `S = 8` near the
//!    axis multiples); `log2(S)` EPX passes (2 → 4 → 8 → 16).
//! 2. **`S×`-rotated buffer** — the upscaled image is rotated once at `S×`
//!    density into a bounded buffer, so every 1× output pixel owns an aligned
//!    `S×S` subpixel block.
//! 3. **Plurality-vote downscale** — each 1× output pixel is the most frequent
//!    (alpha-weighted) color of its `S×S` block, so a single unlucky NN sample
//!    can no longer drop a 1px line.
//! 4. **Grid-fit (integer + sub-pixel)** — the best of the `S²` block
//!    alignments is chosen by a combined color-seam + edge-direction-continuity
//!    metric (see [`choose_offset`]). At `S = 8` an extra half-subpixel phase
//!    doubles the offset resolution to 1/16 of an output pixel, matching the
//!    native 1/16 of the `S = 16` band.
//! 5. **Restore** — isolated 1px source pixels are re-inserted, and 1px breaks
//!    in thin lines are filled (see [`repair_thin_lines`]).
//!
//! Palette purity is exact-equality based: EPX only ever emits one of the four
//! neighbours or the centre pixel, and the vote/restore only copy existing
//! subpixels, so no color is invented and no blending can occur.
//!
//! # Memory guard / tiling
//!
//! The full `S×` EPX upscale plus the `S×`-rotated buffer grow as `S²·w·h·4`,
//! which is infeasible for large selections. When that peak exceeds
//! [`ROTSPRITE_TILE_BYTES`] the core switches to a **tiled** path that bounds
//! peak memory per tile: it streams `S×`-rotated tiles (built from a small
//! per-tile EPX upscale of just the needed source sub-region, plus a halo) to
//! accumulate the offset metric in a first pass, then votes tile by tile. The
//! global offset, phase, vote, restore and repair are all identical to the
//! whole-image result — only the scheduling differs (this is asserted
//! byte-for-byte in the tests). [`rotate_nn`] remains only as a fallback for
//! allocation/overflow failures, not for large-but-valid sources.

use super::{checked_dims, BYTES_PER_PIXEL};

/// Peak-memory threshold above which the RotSprite core switches to the
/// bounded-memory **tiled** path (see [`rotate_rotsprite_cfg`]). Below it the
/// whole-image path runs, byte-identically to the original implementation.
///
/// Large sources no longer fall back to plain NN: they are processed tile by
/// tile so the `S×` EPX upscale and the `S×`-rotated buffer only ever exist one
/// tile at a time.
const ROTSPRITE_TILE_BYTES: usize = 8 * 1024 * 1024;

/// Output tile edge (1× pixels) used by the tiled path. The per-tile
/// intermediates are the `S×` EPX upscale of the tile's source footprint plus
/// the `S×`-rotated tile (each with a halo), bounded by
/// `O(S²·(tile + halo)²·4)` and independent of the full source size. For
/// `tile = 32, S = 16` the peak is a few MiB.
const ROTSPRITE_TILE: usize = 32;

/// Near-axis sampling density: three EPX passes (2 → 4 → 8).
const SCALE_LOW: usize = 8;
/// Near-45° sampling density: four EPX passes (2 → 4 → 8 → 16). Only powers of
/// two exist because Scale2x/EPX doubles.
const SCALE_HIGH: usize = 16;
/// Distance-to-nearest-90°-multiple threshold in degrees: at or above this the
/// high density is used. `22.5°` is exactly half of the 45° worst-skew span, so
/// angles within `22.5°` of an axis take `S = 8` and the central 45° band takes
/// `S = 16` — deterministic and symmetric across all four quadrants.
const SCALE_HIGH_MIN_DEG: f32 = 22.5;

/// Adaptive sampling density `S ∈ {8, 16}`.
///
/// `d` is the distance from `angle_deg` to the nearest multiple of 90°
/// (`0 ≤ d ≤ 45`); the skew/aliasing is worst at `d = 45` (a 45° diagonal) and
/// vanishes at `d = 0`. `S = 16` for `d ≥ 22.5`, else `S = 8`. Deterministic in
/// the angle and independent of the pixel data.
fn adaptive_scale(angle_deg: f32) -> usize {
    let n = angle_deg.rem_euclid(90.0);
    let d = n.min(90.0 - n);
    if d >= SCALE_HIGH_MIN_DEG {
        SCALE_HIGH
    } else {
        SCALE_LOW
    }
}

/// Half of one `S×` block: original pixel `i` occupies sub-pixels
/// `[S·i, S·(i+1)-1]`, whose centre is `S·i + (S-1)/2` on the `S×` grid.
fn center_offset(scale: usize) -> f64 {
    (scale as f64 - 1.0) / 2.0
}

/// Rotates an RGBA8 buffer by `angle_deg` degrees **clockwise** about its
/// centre (screen coordinates, y-down).
///
/// The output is sized to the axis-aligned bounding box of the rotated
/// rectangle (`w' = round(|w·cos θ| + |h·sin θ|)`, `h' = round(|w·sin θ| +
/// |h·cos θ|)`), so no content is clipped.
///
/// Exact multiples of 90° dispatch to the pixel-exact paths in [`exact`], so
/// `rotate(buf, w, h, 90.0)` is bit-identical to `rotate_90_cw` and
/// `rotate(buf, w, h, 0.0)` is a byte-identical copy (TESTING.md).
///
/// Every other angle uses the RotSprite core (adaptive `S×` Scale2x/EPX
/// upscale, an `S×`-rotated buffer, plurality-vote downscale with integer +
/// half-subpixel grid-fit, and isolated/thin-line restore). Sources whose
/// whole-image intermediates would exceed [`ROTSPRITE_TILE_BYTES`] are
/// processed tile by tile with byte-identical output. A non-finite angle
/// (`NaN`/`±inf`) is rejected with an empty result.
///
/// Deterministic: the same input and angle always produce the same bytes.
/// Palette-pure: output colors are a subset of input colors; alpha is only
/// ever copied verbatim, so an input with alpha ∈ {0, 255} yields output
/// alpha ∈ {0, 255} (no anti-aliasing).
pub fn rotate(buf: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return (Vec::new(), 0, 0);
    };

    // Reject NaN / ±inf before any math: `rem_euclid` would carry NaN into
    // the bounding-box dims and there is no meaningful rotation.
    if !angle_deg.is_finite() {
        return (Vec::new(), 0, 0);
    }

    let normalized = angle_deg.rem_euclid(360.0);
    if normalized == 0.0 {
        return (buf[..w * h * BYTES_PER_PIXEL].to_vec(), w, h);
    }
    if normalized == 90.0 {
        return super::exact::rotate_90_cw(buf, w, h);
    }
    if normalized == 180.0 {
        return super::exact::rotate_180(buf, w, h);
    }
    if normalized == 270.0 {
        return super::exact::rotate_90_ccw(buf, w, h);
    }

    // Generic angle: RotSprite. Small sources use the whole-image path; large
    // sources are processed tile by tile (bounded peak memory). `None` only on
    // allocation/overflow failure, in which case the plain 1× NN reverse-map
    // is used as a last resort.
    match rotate_rotsprite(buf, w, h, angle_deg) {
        Some(out) => out,
        None => rotate_nn(buf, w, h, angle_deg),
    }
}

/// Axis-aligned bounding-box dims of a `w × h` rect rotated by `angle_deg`
/// (identical to what `rotate` produces).
///
/// Validates its inputs consistently with [`rotate`]: a non-finite angle or a
/// zero dimension yields `(0, 0)` (matching `rotate`'s empty result), and a
/// computed dimension that does not fit `usize` also yields `(0, 0)`.
pub fn rotated_bounds(w: usize, h: usize, angle_deg: f32) -> (usize, usize) {
    if w == 0 || h == 0 || !angle_deg.is_finite() {
        return (0, 0);
    }
    let n = angle_deg.rem_euclid(360.0);
    if n == 0.0 || n == 180.0 {
        return (w, h);
    }
    if n == 90.0 || n == 270.0 {
        return (h, w);
    }
    let (wf, hf) = (w as f64, h as f64);
    let theta = (angle_deg as f64).to_radians();
    let (c, s) = (theta.cos().abs(), theta.sin().abs());
    let bw = (wf * c + hf * s).round().max(1.0);
    let bh = (wf * s + hf * c).round().max(1.0);
    if !bw.is_finite() || !bh.is_finite() || bw >= usize::MAX as f64 || bh >= usize::MAX as f64 {
        return (0, 0);
    }
    (bw as usize, bh as usize)
}

/// Plain 1× nearest-neighbour reverse-map rotation — the pre-RotSprite core,
/// kept as the deterministic memory-guard fallback.
///
/// Each destination pixel reverse-maps into source space and copies the
/// nearest source pixel (`f64::round`, round-half-away-from-zero);
/// out-of-bounds samples stay transparent. Allocation is `checked_mul` +
/// `try_reserve_exact`, so oversized requests return `(empty, 0, 0)` instead
/// of panicking.
fn rotate_nn(buf: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
    let (w_prime, h_prime) = rotated_bounds(w, h, angle_deg);
    if w_prime == 0 || h_prime == 0 {
        return (Vec::new(), 0, 0);
    }

    let wf = w as f64;
    let hf = h as f64;
    let theta = (angle_deg as f64).to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    let src_cx = (wf - 1.0) / 2.0;
    let src_cy = (hf - 1.0) / 2.0;
    let dst_cx = (w_prime as f64 - 1.0) / 2.0;
    let dst_cy = (h_prime as f64 - 1.0) / 2.0;

    let Some(out_len) = w_prime
        .checked_mul(h_prime)
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
    else {
        return (Vec::new(), 0, 0);
    };
    let Some(mut out) = alloc_zeroed(out_len) else {
        return (Vec::new(), 0, 0);
    };

    for dy in 0..h_prime {
        for dx in 0..w_prime {
            let rel_x = dx as f64 - dst_cx;
            let rel_y = dy as f64 - dst_cy;

            let sx = cos_t * rel_x + sin_t * rel_y + src_cx;
            let sy = -sin_t * rel_x + cos_t * rel_y + src_cy;

            let nx = sx.round() as isize;
            let ny = sy.round() as isize;

            if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                let src = ((ny as usize) * w + (nx as usize)) * BYTES_PER_PIXEL;
                let dst = (dy * w_prime + dx) * BYTES_PER_PIXEL;
                out[dst..dst + BYTES_PER_PIXEL].copy_from_slice(&buf[src..src + BYTES_PER_PIXEL]);
            }
        }
    }

    (out, w_prime, h_prime)
}

/// Weight of an opaque subpixel in the plurality vote: opaque pixels count
/// double so thin 1px structure is not out-voted by the (weight-1) transparent
/// background, while a boundary block still needs a real opaque presence
/// (≈ 1/3 of the 64 subpixels) to switch — a mild, non-dilating bias.
const OPAQUE_WEIGHT: u64 = 2;
/// Weight of a fully transparent subpixel in the plurality vote.
const TRANSPARENT_WEIGHT: u64 = 1;

/// Byte size of the `S×` Scale2x/EPX upscale intermediate for a `w × h`
/// source, or `None` when a dimension overflows. Pure and deterministic in
/// `(w, h, scale)`.
fn rotsprite_bytes(w: usize, h: usize, scale: usize) -> Option<usize> {
    let sw = w.checked_mul(scale)?;
    let sh = h.checked_mul(scale)?;
    sw.checked_mul(sh)?.checked_mul(BYTES_PER_PIXEL)
}

/// Peak byte size of the RotSprite intermediates for `(w, h, angle)`:
/// the `S×` EPX upscale (`Sw·Sh·4`) **plus** the `S×`-rotated buffer
/// (`dest_w·dest_h·4`, each dimension `S·output + S` so every one of the `S²`
/// downscale offsets is in bounds), where `S = adaptive_scale(angle)`.
/// Only ONE rotated buffer is live at a time (phase candidates are evaluated
/// one after another), so the peak is exactly upscale + rotated. `None` on
/// overflow.
///
/// Deterministic in `(w, h, angle)`; `rotate`'s callers — the preview path and
/// the commit path in `object.rs` — always pass identical `(w, h, angle)`, so
/// the guard selects the same branch for both.
fn rotsprite_peak_bytes(w: usize, h: usize, angle_deg: f32) -> Option<usize> {
    let scale = adaptive_scale(angle_deg);
    let upscale = rotsprite_bytes(w, h, scale)?;
    let (wp, hp) = rotated_bounds(w, h, angle_deg);
    if wp == 0 || hp == 0 {
        return None;
    }
    let dw = wp.checked_mul(scale)?.checked_add(scale)?;
    let dh = hp.checked_mul(scale)?.checked_add(scale)?;
    let rotated = dw.checked_mul(dh)?.checked_mul(BYTES_PER_PIXEL)?;
    upscale.checked_add(rotated)
}

/// RotSprite core: adaptive-`S` Scale2x/EPX upscale, an `S×`-rotated buffer,
/// plurality-vote downscale using the best `S`-grid offset (with an optional
/// half-subpixel phase when `S = 8`), then isolated-pixel and thin-line
/// restore.
///
/// Returns `None` (→ caller falls back to [`rotate_nn`]) when the peak
/// intermediate overflows or exceeds [`MAX_ROTSPRITE_BYTES`].
fn rotate_rotsprite(
    buf: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
) -> Option<(Vec<u8>, usize, usize)> {
    rotate_rotsprite_opts(buf, w, h, angle_deg, None, true)
}

/// Implementation behind [`rotate_rotsprite`] with test-visible knobs:
/// `forced_offset` overrides the grid-fit search (the comparison experiment
/// uses `Some((0, 0))` for "majority without offset fit"; it also pins the
/// single phase 0), and `restore` toggles the post-passes.
fn rotate_rotsprite_opts(
    buf: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
    forced_offset: Option<(usize, usize)>,
    restore: bool,
) -> Option<(Vec<u8>, usize, usize)> {
    rotate_rotsprite_cfg(
        buf,
        w,
        h,
        angle_deg,
        forced_offset,
        restore,
        ROTSPRITE_TILE_BYTES,
        ROTSPRITE_TILE,
    )
}

/// Core implementation with an explicit tiling configuration.
///
/// `tile_budget` is the whole-image peak-memory threshold: when
/// `rotsprite_peak_bytes(w, h, angle)` exceeds it (or overflows), the tiled
/// path runs; otherwise the whole-image path runs, byte-identically to the
/// original. `tile` is the output-tile edge (1× pixels) for the tiled path.
/// Tests use a `0` budget to force tiling and a huge budget to force the
/// whole-image path on small fixtures.
#[allow(clippy::too_many_arguments)]
fn rotate_rotsprite_cfg(
    buf: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
    forced_offset: Option<(usize, usize)>,
    restore: bool,
    tile_budget: usize,
    tile: usize,
) -> Option<(Vec<u8>, usize, usize)> {
    let (w_prime, h_prime) = rotated_bounds(w, h, angle_deg);
    if w_prime == 0 || h_prime == 0 {
        return None;
    }

    let scale = adaptive_scale(angle_deg);

    // Decide the path from the whole-image peak. `None` (overflow) always
    // tiles, since the tiled intermediates are bounded regardless of size.
    let use_tiling = match rotsprite_peak_bytes(w, h, angle_deg) {
        Some(peak) => peak > tile_budget,
        None => true,
    };

    // Sub-pixel phase candidates. At S = 8 an extra half-subpixel phase
    // doubles the effective offset resolution to 1/16 of an output pixel,
    // matching the native 1/16 of the S = 16 band. At S = 16 phase 0 already
    // gives that resolution, so only one phase is evaluated. A forced offset
    // pins phase 0.
    let phase_zero = [0.0f64];
    let phase_half = [0.0f64, 0.5];
    let phases: &[f64] = if forced_offset.is_some() {
        &phase_zero
    } else if scale == SCALE_LOW {
        &phase_half
    } else {
        &phase_zero
    };

    let out_len = w_prime.checked_mul(h_prime)?.checked_mul(BYTES_PER_PIXEL)?;

    if !use_tiling {
        // -- Whole-image path (byte-identical to the original) --------------
        let (up, uw, uh) = upscale_scale2x(buf, w, h, scale)?;
        let dw = w_prime.checked_mul(scale)?.checked_add(scale)?;
        let dh = h_prime.checked_mul(scale)?.checked_add(scale)?;

        // Evaluate every phase and keep the best (lowest artifact score). Only
        // one S×-rotated buffer is live at a time.
        let mut best: Option<(i64, Vec<u8>)> = None;
        for &phase in phases {
            let rot = rotate_scaled(
                &up, uw, uh, w, h, w_prime, h_prime, dw, dh, angle_deg, scale, phase,
            )?;
            let (offset, score) = match forced_offset {
                Some(offset) => (
                    offset,
                    offset_score_at(&rot, dw, dh, scale, offset.0, offset.1),
                ),
                None => choose_offset(&rot, dw, dh, scale),
            };
            let voted = downscale_vote(&rot, dw, dh, w_prime, h_prime, offset.0, offset.1, scale)?;
            if best
                .as_ref()
                .is_none_or(|(best_score, _)| score < *best_score)
            {
                best = Some((score, voted));
            }
        }
        let (_, mut out) = best?;
        if restore {
            restore_isolated(&mut out, w_prime, h_prime, buf, w, h, angle_deg);
            repair_thin_lines(&mut out, w_prime, h_prime);
        }
        return Some((out, w_prime, h_prime));
    }

    // -- Tiled path (bounded peak memory) -----------------------------------
    let mut out = alloc_zeroed(out_len)?;
    match forced_offset {
        Some((fox, foy)) => {
            vote_tiled(
                &mut out, w_prime, h_prime, buf, w, h, angle_deg, scale, 0.0, fox, foy, tile,
            )?;
        }
        None => {
            // Pass 1: stream rotated tiles to pick the global phase + offset.
            let mut best_phase: Option<(i64, f64, (usize, usize))> = None;
            for &phase in phases {
                let (offset, score) = choose_offset_tiled(
                    buf, w, h, w_prime, h_prime, angle_deg, scale, phase, tile,
                )?;
                if best_phase
                    .as_ref()
                    .is_none_or(|(best_score, _, _)| score < *best_score)
                {
                    best_phase = Some((score, phase, offset));
                }
            }
            let (_, phase, offset) = best_phase?;

            // Pass 2: vote tile by tile with the chosen phase + offset.
            vote_tiled(
                &mut out, w_prime, h_prime, buf, w, h, angle_deg, scale, phase, offset.0, offset.1,
                tile,
            )?;
        }
    }

    // `restore`/`repair` are global post-passes on the fully-materialised
    // output, so they are identical to the whole-image path.
    if restore {
        restore_isolated(&mut out, w_prime, h_prime, buf, w, h, angle_deg);
        repair_thin_lines(&mut out, w_prime, h_prime);
    }
    Some((out, w_prime, h_prime))
}

/// Builds the `S×`-rotated buffer: every `S×` destination pixel reverse-maps to
/// an original coordinate, which is scaled onto the `S×` EPX grid and sampled
/// nearest — the same f64 reverse map as [`rotate_nn`], just at `S×` density.
///
/// `phase` (subpixel units, `0.0` or `0.5`) shifts the sampling grid by a
/// fraction of one `S×` sub-pixel; it is applied BEFORE the NN round, so every
/// sampled color is still a real upscaled-source pixel (no interpolation).
///
/// Sub-pixel indexing: 1× output pixel `dx` owns the block `[S·dx ..= S·dx+S-1]`
/// of this buffer, whose centre `S·dx + (S-1)/2` reverse-maps (at phase 0) to
/// exactly the original `dx` sample — so the block centre reproduces the plain
/// NN result and the `S²` subpixels around it are the vote's evidence.
#[allow(clippy::too_many_arguments)]
fn rotate_scaled(
    up: &[u8],
    uw: usize,
    uh: usize,
    w: usize,
    h: usize,
    w_prime: usize,
    h_prime: usize,
    dw: usize,
    dh: usize,
    angle_deg: f32,
    scale: usize,
    phase: f64,
) -> Option<Vec<u8>> {
    let len = dw.checked_mul(dh)?.checked_mul(BYTES_PER_PIXEL)?;
    let mut out = alloc_zeroed(len)?;

    let theta = (angle_deg as f64).to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    let sc = scale as f64;
    let center = center_offset(scale);
    // All centres below are in ORIGINAL (1×) pixel units; only the final
    // `sc * sx + center` converts to the S× EPX grid.
    let src_cx = (w as f64 - 1.0) / 2.0;
    let src_cy = (h as f64 - 1.0) / 2.0;
    let dst_cx = (w_prime as f64 - 1.0) / 2.0;
    let dst_cy = (h_prime as f64 - 1.0) / 2.0;

    let uwi = uw as isize;
    let uhi = uh as isize;

    for by in 0..dh {
        let out_y = (by as f64 - center - phase) / sc;
        let rel_y = out_y - dst_cy;
        for bx in 0..dw {
            let out_x = (bx as f64 - center - phase) / sc;
            let rel_x = out_x - dst_cx;

            let sx = cos_t * rel_x + sin_t * rel_y + src_cx;
            let sy = -sin_t * rel_x + cos_t * rel_y + src_cy;

            let gx = (sc * sx + center).round() as isize;
            let gy = (sc * sy + center).round() as isize;

            if gx >= 0 && gy >= 0 && gx < uwi && gy < uhi {
                let si = ((gy as usize) * uw + (gx as usize)) * BYTES_PER_PIXEL;
                let di = (by * dw + bx) * BYTES_PER_PIXEL;
                out[di..di + BYTES_PER_PIXEL].copy_from_slice(&up[si..si + BYTES_PER_PIXEL]);
            }
        }
    }

    Some(out)
}

// -- Bounded-memory tiling -------------------------------------------------

/// Halo (in source pixels) needed so a per-tile EPX upscale is exact for its
/// inner region: the dependency radius of `log2(S)` EPX passes is at most
/// `log2(S)` source pixels, plus one for safety.
fn epx_halo(scale: usize) -> usize {
    scale.trailing_zeros() as usize + 1
}

/// A local `S×` EPX upscale of a source-pixel region.
struct UpRegion {
    buf: Vec<u8>,
    uw: usize,
    uh: usize,
    /// Global `S×` origin (`sx0·S`, `sy0·S`).
    gx0: isize,
    gy0: isize,
}

impl UpRegion {
    /// Global `S×` sample, or transparent when outside this region.
    fn sample(&self, gx: isize, gy: isize) -> [u8; 4] {
        let lx = gx - self.gx0;
        let ly = gy - self.gy0;
        if lx >= 0 && ly >= 0 && (lx as usize) < self.uw && (ly as usize) < self.uh {
            read_px(&self.buf, self.uw, lx as usize, ly as usize)
        } else {
            [0, 0, 0, 0]
        }
    }
}

/// Builds an [`UpRegion`] for source pixels `[rx0, rx1) × [ry0, ry1)`, padded
/// by `pad` source pixels (clamped to the image) so the inner region's EPX
/// values match the whole-image upscale exactly. The padded border is treated
/// as an image border by [`upscale_scale2x`], which is harmless because the
/// dependency radius is `< pad`.
fn build_up_region(
    buf: &[u8],
    w: usize,
    h: usize,
    scale: usize,
    rx0: usize,
    rx1: usize,
    ry0: usize,
    ry1: usize,
    pad: usize,
) -> Option<UpRegion> {
    if rx0 >= rx1 || ry0 >= ry1 {
        return None;
    }
    let bx0 = rx0.saturating_sub(pad);
    let bx1 = (rx1 + pad).min(w);
    let by0 = ry0.saturating_sub(pad);
    let by1 = (ry1 + pad).min(h);
    let bw = bx1 - bx0;
    let bh = by1 - by0;
    debug_assert!(bw > 0 && bh > 0);

    let mut sub = alloc_zeroed(bw.checked_mul(bh)?.checked_mul(BYTES_PER_PIXEL)?)?;
    for y in 0..bh {
        for x in 0..bw {
            let di = (y * bw + x) * BYTES_PER_PIXEL;
            let si = ((by0 + y) * w + (bx0 + x)) * BYTES_PER_PIXEL;
            sub[di..di + BYTES_PER_PIXEL].copy_from_slice(&buf[si..si + BYTES_PER_PIXEL]);
        }
    }

    let (up, uw, uh) = upscale_scale2x(&sub, bw, bh, scale)?;
    Some(UpRegion {
        buf: up,
        uw,
        uh,
        gx0: (bx0 * scale) as isize,
        gy0: (by0 * scale) as isize,
    })
}

/// Source-pixel bbox `[x0,x1) × [y0,y1)` whose reverse-mapped `S×` samples can
/// land inside the rotated-buffer rectangle `[ex0, ex1) × [ey0, ey1)`. The map
/// is affine, so its extrema are at the rectangle corners; a small margin
/// absorbs rounding.
#[allow(clippy::too_many_arguments)]
fn source_bbox_for_region(
    ex0: f64,
    ey0: f64,
    ex1: f64,
    ey1: f64,
    w: usize,
    h: usize,
    w_prime: usize,
    h_prime: usize,
    angle_deg: f32,
    scale: usize,
    phase: f64,
) -> (usize, usize, usize, usize) {
    let theta = (angle_deg as f64).to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();
    let sc = scale as f64;
    let center = center_offset(scale);
    let src_cx = (w as f64 - 1.0) / 2.0;
    let src_cy = (h as f64 - 1.0) / 2.0;
    let dst_cx = (w_prime as f64 - 1.0) / 2.0;
    let dst_cy = (h_prime as f64 - 1.0) / 2.0;

    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for &bx in &[ex0, ex1] {
        for &by in &[ey0, ey1] {
            let out_x = (bx - center - phase) / sc;
            let out_y = (by - center - phase) / sc;
            let rel_x = out_x - dst_cx;
            let rel_y = out_y - dst_cy;
            let sx = cos_t * rel_x + sin_t * rel_y + src_cx;
            let sy = -sin_t * rel_x + cos_t * rel_y + src_cy;
            min_x = min_x.min(sx);
            max_x = max_x.max(sx);
            min_y = min_y.min(sy);
            max_y = max_y.max(sy);
        }
    }

    // Margin covers rounding plus any affine extremum nuance.
    const MARGIN: f64 = 2.0;
    let wf = w as f64;
    let hf = h as f64;
    let raw_x0 = (min_x - MARGIN).floor().max(0.0).min(wf) as usize;
    let raw_x1 = (((max_x + MARGIN).ceil().max(0.0) as usize) + 1).min(w);
    let raw_y0 = (min_y - MARGIN).floor().max(0.0).min(hf) as usize;
    let raw_y1 = (((max_y + MARGIN).ceil().max(0.0) as usize) + 1).min(h);

    // Guarantee a non-empty region: a region mapping entirely outside the
    // source maps to all-transparent samples, so a 1-pixel probe region is
    // sufficient (the global `S×` bounds check then yields transparent).
    let (x0, x1) = if raw_x1 <= raw_x0 {
        let a = raw_x0.min(w - 1);
        (a, a + 1)
    } else {
        (raw_x0, raw_x1)
    };
    let (y0, y1) = if raw_y1 <= raw_y0 {
        let a = raw_y0.min(h - 1);
        (a, a + 1)
    } else {
        (raw_y0, raw_y1)
    };
    (x0, x1, y0, y1)
}

/// Fills a rotated-buffer region `[ex0, ex0+rw) × [ey0, ey0+rh)` (global
/// coords) exactly as [`rotate_scaled`] would, but from a small per-region
/// [`UpRegion`] instead of the whole `S×` upscale. Out-of-image samples are
/// transparent, matching [`rotate_scaled`].
#[allow(clippy::too_many_arguments)]
fn rotate_scaled_region(
    up: &UpRegion,
    w: usize,
    h: usize,
    w_prime: usize,
    h_prime: usize,
    angle_deg: f32,
    scale: usize,
    phase: f64,
    ex0: isize,
    ey0: isize,
    rw: usize,
    rh: usize,
) -> Option<Vec<u8>> {
    let len = rw.checked_mul(rh)?.checked_mul(BYTES_PER_PIXEL)?;
    let mut out = alloc_zeroed(len)?;

    let theta = (angle_deg as f64).to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();
    let sc = scale as f64;
    let center = center_offset(scale);
    let src_cx = (w as f64 - 1.0) / 2.0;
    let src_cy = (h as f64 - 1.0) / 2.0;
    let dst_cx = (w_prime as f64 - 1.0) / 2.0;
    let dst_cy = (h_prime as f64 - 1.0) / 2.0;

    let gw = (w as isize).checked_mul(scale as isize)?;
    let gh = (h as isize).checked_mul(scale as isize)?;

    for ly in 0..rh {
        let by = ey0 + ly as isize;
        let out_y = (by as f64 - center - phase) / sc;
        let rel_y = out_y - dst_cy;
        for lx in 0..rw {
            let bx = ex0 + lx as isize;
            let out_x = (bx as f64 - center - phase) / sc;
            let rel_x = out_x - dst_cx;

            let sx = cos_t * rel_x + sin_t * rel_y + src_cx;
            let sy = -sin_t * rel_x + cos_t * rel_y + src_cy;

            let gx = (sc * sx + center).round() as isize;
            let gy = (sc * sy + center).round() as isize;

            let col = if gx >= 0 && gy >= 0 && gx < gw && gy < gh {
                up.sample(gx, gy)
            } else {
                [0, 0, 0, 0]
            };
            write_px(&mut out, rw, lx, ly, col);
        }
    }
    Some(out)
}

/// Builds a rotated-buffer region `[ex0, ex0+rw) × [ey0, ey0+rh)` by (a)
/// bounding the source pixels it needs, (b) EPX-upscaling just that region with
/// a halo, and (c) reverse-mapping the region into it. Byte-identical to the
/// corresponding slice of the whole-image [`rotate_scaled`].
#[allow(clippy::too_many_arguments)]
fn build_rotated_tile(
    buf: &[u8],
    w: usize,
    h: usize,
    w_prime: usize,
    h_prime: usize,
    angle_deg: f32,
    scale: usize,
    phase: f64,
    ex0: isize,
    ey0: isize,
    rw: usize,
    rh: usize,
) -> Option<Vec<u8>> {
    let (rx0, rx1, ry0, ry1) = source_bbox_for_region(
        ex0 as f64,
        ey0 as f64,
        (ex0 + rw as isize) as f64,
        (ey0 + rh as isize) as f64,
        w,
        h,
        w_prime,
        h_prime,
        angle_deg,
        scale,
        phase,
    );
    let up = build_up_region(buf, w, h, scale, rx0, rx1, ry0, ry1, epx_halo(scale))?;
    rotate_scaled_region(
        &up, w, h, w_prime, h_prime, angle_deg, scale, phase, ex0, ey0, rw, rh,
    )
}

/// Seam / continuity histograms for the offset search, sized for `S = 16`.
struct OffsetMetric {
    /// Boundary-aligned horizontal seams (vertical edges) by `(bx+1) % S`.
    hist_h: [u64; SCALE_HIGH],
    /// Boundary-aligned vertical seams (horizontal edges) by `(by+1) % S`.
    hist_v: [u64; SCALE_HIGH],
    /// Aligned seams that continue along the same boundary (run length ≥ 2).
    run_h: [u64; SCALE_HIGH],
    run_v: [u64; SCALE_HIGH],
    /// Edge corners (perpendicular seams meeting at one subpixel) by the
    /// offset that puts both arms on block boundaries.
    corner: [[u64; SCALE_HIGH]; SCALE_HIGH],
    /// Total number of seams (offset-independent).
    total: i64,
}

impl OffsetMetric {
    /// Empty histogram accumulator.
    fn new() -> Self {
        OffsetMetric {
            hist_h: [0; SCALE_HIGH],
            hist_v: [0; SCALE_HIGH],
            run_h: [0; SCALE_HIGH],
            run_v: [0; SCALE_HIGH],
            corner: [[0; SCALE_HIGH]; SCALE_HIGH],
            total: 0,
        }
    }
}

/// Accumulates the offset metric over the `own` rectangle of a rotated-buffer
/// region.
///
/// `region` covers global rotated pixels `[ex0, ex0+rw) × [ey0, ey0+rh)` and
/// must include a 1-pixel halo around `own`; `own` is the sub-rectangle
/// attributed to this call (global coords). `dw`/`dh` are the full
/// rotated-buffer dims, so seam existence at the global edges matches the
/// whole-image scan exactly. Accumulating every disjoint `own` once reproduces
/// the whole-buffer scan (`offset_metric`) exactly — this is what makes the
/// tiled offset identical to the un-tiled one.
#[allow(clippy::too_many_arguments)]
fn accumulate_metric_region(
    region: &[u8],
    rw: usize,
    rh: usize,
    ex0: isize,
    ey0: isize,
    own_x0: usize,
    own_x1: usize,
    own_y0: usize,
    own_y1: usize,
    dw: usize,
    dh: usize,
    scale: usize,
    m: &mut OffsetMetric,
) {
    debug_assert!(own_y1.saturating_sub(own_y0) <= rh);
    debug_assert!(own_x1.saturating_sub(own_x0) <= rw);
    let at = |bx: isize, by: isize| -> [u8; 4] {
        let lx = (bx - ex0) as usize;
        let ly = (by - ey0) as usize;
        read_px(region, rw, lx, ly)
    };

    for by in own_y0..own_y1 {
        let byi = by as isize;
        for bx in own_x0..own_x1 {
            let bxi = bx as isize;
            let c = at(bxi, byi);
            let right = bx + 1 < dw && at(bxi + 1, byi) != c;
            let down = by + 1 < dh && at(bxi, byi + 1) != c;

            if right {
                let r = (bx + 1) % scale;
                m.hist_h[r] += 1;
                let up_seam = by > 0 && at(bxi, byi - 1) != at(bxi + 1, byi - 1);
                let dn_seam = by + 1 < dh && at(bxi, byi + 1) != at(bxi + 1, byi + 1);
                if up_seam || dn_seam {
                    m.run_h[r] += 1;
                }
            }
            if down {
                let r = (by + 1) % scale;
                m.hist_v[r] += 1;
                let lf_seam = bx > 0 && at(bxi - 1, byi) != at(bxi - 1, byi + 1);
                let rt_seam = bx + 1 < dw && at(bxi + 1, byi) != at(bxi + 1, byi + 1);
                if lf_seam || rt_seam {
                    m.run_v[r] += 1;
                }
            }
            if right && down {
                m.corner[(bx + 1) % scale][(by + 1) % scale] += 1;
            }
            m.total += right as i64 + down as i64;
        }
    }
}

/// Scans a whole `S×`-rotated buffer once and tallies the offset metric.
/// Equivalent to a single [`accumulate_metric_region`] call covering `dw × dh`.
fn offset_metric(rot: &[u8], dw: usize, dh: usize, scale: usize) -> OffsetMetric {
    let mut m = OffsetMetric::new();
    accumulate_metric_region(rot, dw, dh, 0, 0, 0, dw, 0, dh, dw, dh, scale, &mut m);
    m
}

/// Artifact score for one offset (lower is better):
///
/// ```text
/// score = inside_block_seams
///         - boundary_aligned_run_seams        (edge continues along the seam)
///         - 2 * aligned_edge_corners          (perpendicular edges meet crisply)
/// ```
///
/// The first term is the T1 in-block color-seam penalty; the run and corner
/// terms are the **edge-direction continuity** reward (a seam that runs along a
/// block boundary, or a corner whose two arms both land on boundaries, keeps
/// its gradient direction crisp through the vote). Integer-only, deterministic.
fn offset_score(m: &OffsetMetric, ox: usize, oy: usize) -> i64 {
    let inside = m.total - m.hist_h[ox] as i64 - m.hist_v[oy] as i64;
    inside - m.run_h[ox] as i64 - m.run_v[oy] as i64 - 2 * m.corner[ox][oy] as i64
}

/// Best offset for an accumulated metric: exhaustive over all `S²` candidates;
/// ties keep the lowest `(ox, oy)` in row-major order (deterministic).
fn best_offset(m: &OffsetMetric, scale: usize) -> ((usize, usize), i64) {
    let mut best_score: Option<i64> = None;
    let mut best = (0usize, 0usize);
    for ox in 0..scale {
        for oy in 0..scale {
            let score = offset_score(m, ox, oy);
            if best_score.is_none_or(|b| score < b) {
                best_score = Some(score);
                best = (ox, oy);
            }
        }
    }
    (best, best_score.unwrap_or(0))
}

/// Picks the downscale grid offset `(ox, oy) ∈ 0..S × 0..S` minimising the
/// combined artifact score (see [`offset_score`]) for a whole-image rotated
/// buffer. Ties keep the lowest `(ox, oy)`, so the choice is a deterministic
/// function of the rotated buffer.
fn choose_offset(rot: &[u8], dw: usize, dh: usize, scale: usize) -> ((usize, usize), i64) {
    best_offset(&offset_metric(rot, dw, dh, scale), scale)
}

/// Tiled offset search: streams `S×`-rotated tiles (with a 1-pixel halo) over
/// the whole rotated buffer and accumulates the same histograms as
/// [`offset_metric`], then applies [`best_offset`]. Because the tiles partition
/// the buffer and the halo supplies every cross-tile neighbour, the accumulated
/// metric — and therefore the chosen offset and score — is identical to the
/// whole-image search.
#[allow(clippy::too_many_arguments)]
fn choose_offset_tiled(
    buf: &[u8],
    w: usize,
    h: usize,
    w_prime: usize,
    h_prime: usize,
    angle_deg: f32,
    scale: usize,
    phase: f64,
    tile: usize,
) -> Option<((usize, usize), i64)> {
    let dw = w_prime.checked_mul(scale)?.checked_add(scale)?;
    let dh = h_prime.checked_mul(scale)?.checked_add(scale)?;
    let rstep = tile.checked_mul(scale)?.max(1);

    let mut m = OffsetMetric::new();
    let mut by0 = 0usize;
    while by0 < dh {
        let by1 = (by0 + rstep).min(dh);
        let ey0 = by0.saturating_sub(1);
        let ey1 = (by1 + 1).min(dh);
        let mut bx0 = 0usize;
        while bx0 < dw {
            let bx1 = (bx0 + rstep).min(dw);
            let ex0 = bx0.saturating_sub(1);
            let ex1 = (bx1 + 1).min(dw);
            let rw = ex1 - ex0;
            let rh = ey1 - ey0;
            let region = build_rotated_tile(
                buf,
                w,
                h,
                w_prime,
                h_prime,
                angle_deg,
                scale,
                phase,
                ex0 as isize,
                ey0 as isize,
                rw,
                rh,
            )?;
            accumulate_metric_region(
                &region,
                rw,
                rh,
                ex0 as isize,
                ey0 as isize,
                bx0,
                bx1,
                by0,
                by1,
                dw,
                dh,
                scale,
                &mut m,
            );
            bx0 = bx1;
        }
        by0 = by1;
    }
    Some(best_offset(&m, scale))
}

/// The combined artifact score of one specific offset (used by the comparison
/// experiment's forced-offset baseline).
fn offset_score_at(rot: &[u8], dw: usize, dh: usize, scale: usize, ox: usize, oy: usize) -> i64 {
    let m = offset_metric(rot, dw, dh, scale);
    offset_score(&m, ox, oy)
}

/// Plurality-votes one `S×S` block of a rotated buffer (row stride `rw`) whose
/// top-left is `(x0, y0)`.
///
/// Each subpixel contributes [`OPAQUE_WEIGHT`] if it has any alpha, else
/// [`TRANSPARENT_WEIGHT`] ("alpha-weighted"). The color with the largest total
/// weight wins; ties go to the lowest `[r, g, b, a]` key. Because the winner is
/// one of the block's own subpixels, the result stays palette-pure with no
/// blending/AA.
fn vote_block(rot: &[u8], rw: usize, x0: usize, y0: usize, scale: usize) -> [u8; 4] {
    // Reused per-block tally (≤ S² distinct colors, S ≤ 16).
    let mut keys = [[0u8; 4]; SCALE_HIGH * SCALE_HIGH];
    let mut weights = [0u64; SCALE_HIGH * SCALE_HIGH];
    let mut n = 0usize;

    for l in 0..scale {
        let by = y0 + l;
        for k in 0..scale {
            let bx = x0 + k;
            let c = read_px(rot, rw, bx, by);
            let w = if c[3] == 0 {
                TRANSPARENT_WEIGHT
            } else {
                OPAQUE_WEIGHT
            };
            let mut found = false;
            for i in 0..n {
                if keys[i] == c {
                    weights[i] += w;
                    found = true;
                    break;
                }
            }
            if !found {
                keys[n] = c;
                weights[n] = w;
                n += 1;
            }
        }
    }

    let mut best = 0usize;
    for i in 1..n {
        if weights[i] > weights[best] || (weights[i] == weights[best] && keys[i] < keys[best]) {
            best = i;
        }
    }
    keys[best]
}

/// Whole-image plurality-vote downscale: each 1× output pixel `(dx, dy)` is the
/// plurality of its `S×S` rotated block at grid offset `(ox, oy)`.
#[allow(clippy::too_many_arguments)]
fn downscale_vote(
    rot: &[u8],
    dw: usize,
    dh: usize,
    w_prime: usize,
    h_prime: usize,
    ox: usize,
    oy: usize,
    scale: usize,
) -> Option<Vec<u8>> {
    let len = w_prime.checked_mul(h_prime)?.checked_mul(BYTES_PER_PIXEL)?;
    let mut out = alloc_zeroed(len)?;

    // The +S slack on the rotated buffer keeps every block in bounds.
    debug_assert!(scale * (h_prime - 1) + oy + (scale - 1) < dh);
    debug_assert!(scale * (w_prime - 1) + ox + (scale - 1) < dw);

    for dy in 0..h_prime {
        for dx in 0..w_prime {
            let col = vote_block(rot, dw, scale * dx + ox, scale * dy + oy, scale);
            let di = (dy * w_prime + dx) * BYTES_PER_PIXEL;
            out[di..di + BYTES_PER_PIXEL].copy_from_slice(&col);
        }
    }

    Some(out)
}

/// Tiled plurality-vote downscale: streams 1× output tiles, building only the
/// `S×`-rotated sub-region each tile's blocks need, and writes the plurality
/// into `out` (full `w_prime × h_prime`). Byte-identical to
/// [`downscale_vote`] for the same phase/offset.
#[allow(clippy::too_many_arguments)]
fn vote_tiled(
    out: &mut [u8],
    w_prime: usize,
    h_prime: usize,
    buf: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
    scale: usize,
    phase: f64,
    ox: usize,
    oy: usize,
    tile: usize,
) -> Option<()> {
    let tile = tile.max(1);
    let mut dy0 = 0usize;
    while dy0 < h_prime {
        let dy1 = (dy0 + tile).min(h_prime);
        let mut dx0 = 0usize;
        while dx0 < w_prime {
            let dx1 = (dx0 + tile).min(w_prime);

            // Rotated-buffer bbox covering every block of this output tile.
            let bxmin = scale * dx0 + ox;
            let bxmax = scale * (dx1 - 1) + ox + scale - 1;
            let bymin = scale * dy0 + oy;
            let bymax = scale * (dy1 - 1) + oy + scale - 1;
            let rw = bxmax - bxmin + 1;
            let rh = bymax - bymin + 1;

            let region = build_rotated_tile(
                buf,
                w,
                h,
                w_prime,
                h_prime,
                angle_deg,
                scale,
                phase,
                bxmin as isize,
                bymin as isize,
                rw,
                rh,
            )?;

            for dy in dy0..dy1 {
                for dx in dx0..dx1 {
                    let lx = scale * dx + ox - bxmin;
                    let ly = scale * dy + oy - bymin;
                    let col = vote_block(&region, rw, lx, ly, scale);
                    let di = (dy * w_prime + dx) * BYTES_PER_PIXEL;
                    out[di..di + BYTES_PER_PIXEL].copy_from_slice(&col);
                }
            }

            dx0 = dx1;
        }
        dy0 = dy1;
    }
    Some(())
}

/// Restores isolated 1px source details erased by the vote.
///
/// A source pixel is *isolated* when it is opaque and none of its 8 neighbours
/// has the same RGBA. Each such pixel is forward-mapped (`dst_center + R·(x −
/// src_center)`, the exact inverse of the sampler) to a 1× output cell; if that
/// cell is transparent it is replaced by the source color. Scanning the source
/// in row-major order and only ever filling transparent cells makes the pass
/// deterministic, palette-pure and AA-free.
fn restore_isolated(
    out: &mut [u8],
    w_prime: usize,
    h_prime: usize,
    src: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
) {
    let theta = (angle_deg as f64).to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    let src_cx = (w as f64 - 1.0) / 2.0;
    let src_cy = (h as f64 - 1.0) / 2.0;
    let dst_cx = (w_prime as f64 - 1.0) / 2.0;
    let dst_cy = (h_prime as f64 - 1.0) / 2.0;

    for y in 0..h {
        for x in 0..w {
            let p = read_px(src, w, x, y);
            if p[3] == 0 || !is_isolated(src, w, h, x, y, p) {
                continue;
            }
            let vx = x as f64 - src_cx;
            let vy = y as f64 - src_cy;
            let fx = dst_cx + cos_t * vx - sin_t * vy;
            let fy = dst_cy + sin_t * vx + cos_t * vy;

            let ox = fx.round() as isize;
            let oy = fy.round() as isize;
            if ox >= 0 && oy >= 0 && (ox as usize) < w_prime && (oy as usize) < h_prime {
                let di = ((oy as usize) * w_prime + ox as usize) * BYTES_PER_PIXEL;
                if out[di + 3] == 0 {
                    out[di..di + BYTES_PER_PIXEL].copy_from_slice(&p);
                }
            }
        }
    }
}

/// True when no 8-neighbour of `(x, y)` has the same RGBA as `p`.
fn is_isolated(src: &[u8], w: usize, h: usize, x: usize, y: usize, p: [u8; 4]) -> bool {
    let xi = x as isize;
    let yi = y as isize;
    for dy in -1..=1isize {
        for dx in -1..=1isize {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = xi + dx;
            let ny = yi + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let (ux, uy) = (nx as usize, ny as usize);
            if ux < w && uy < h && read_px(src, w, ux, uy) == p {
                return false;
            }
        }
    }
    true
}

/// The 8 neighbour offsets, in a fixed order (determinism).
const NEIGHBOURS_8: [(isize, isize); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// 1px thin-line repair: fills a TRANSPARENT output cell that is a one-pixel
/// break in a thin line.
///
/// Rule (conservative, so it cannot merge regions or fill blobs):
/// a transparent cell is filled **iff** it has **exactly two** opaque
/// 8-neighbours, those two have the **same RGBA**, and they sit on **opposite**
/// sides of the cell (horizontal `A . B`, vertical, or either diagonal). The
/// fill color is that neighbour color.
///
/// Rationale: a genuine 1px break leaves exactly the two line ends as opaque
/// neighbours; a longer gap has fewer than two, an endpoint has fewer than two,
/// and a solid-region hole, a deliberate 1px separator, or a boundary corner
/// has more than two (the extra diagonal/axis neighbours make `count != 2`), so
/// those are never touched. Only transparent cells are written; the pass reads
/// a pre-pass snapshot and applies all fills afterwards, so it cannot cascade
/// (a 2px gap stays a gap). Palette-pure, AA-free, deterministic.
fn repair_thin_lines(out: &mut [u8], w: usize, h: usize) {
    let mut fills: Vec<(usize, [u8; 4])> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * BYTES_PER_PIXEL;
            if out[i + 3] != 0 {
                continue; // only fill transparent cells
            }

            let xi = x as isize;
            let yi = y as isize;
            let mut count = 0usize;
            let mut c0 = [0u8; 4];
            let mut c1 = [0u8; 4];
            let mut d0 = (0isize, 0isize);
            let mut d1 = (0isize, 0isize);
            for (dx, dy) in NEIGHBOURS_8 {
                let nx = xi + dx;
                let ny = yi + dy;
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    continue;
                }
                let p = read_px(out, w, nx as usize, ny as usize);
                if p[3] == 0 {
                    continue;
                }
                match count {
                    0 => {
                        c0 = p;
                        d0 = (dx, dy);
                    }
                    1 => {
                        c1 = p;
                        d1 = (dx, dy);
                    }
                    _ => {}
                }
                count += 1;
            }

            if count == 2 && c0 == c1 && d0.0 + d1.0 == 0 && d0.1 + d1.1 == 0 {
                fills.push((i, c0));
            }
        }
    }
    for (i, c) in fills {
        out[i..i + BYTES_PER_PIXEL].copy_from_slice(&c);
    }
}

/// `log2(scale)` Scale2x/EPX passes:
/// `w × h` → `2w × 2h` → … → `scale·w × scale·h`. `scale` must be a power of
/// two in `{8, 16}`.
fn upscale_scale2x(
    buf: &[u8],
    w: usize,
    h: usize,
    scale: usize,
) -> Option<(Vec<u8>, usize, usize)> {
    debug_assert!(scale.is_power_of_two() && scale >= SCALE_LOW);
    let needed = w.checked_mul(h)?.checked_mul(BYTES_PER_PIXEL)?;
    let mut cur = buf.get(..needed)?.to_vec();
    let (mut cw, mut ch) = (w, h);
    for _ in 0..scale.trailing_zeros() {
        let nw = cw.checked_mul(2)?;
        let nh = ch.checked_mul(2)?;
        cur = scale2x_epx(&cur, cw, ch)?;
        cw = nw;
        ch = nh;
    }
    Some((cur, cw, ch))
}

/// One Scale2x / EPX pass: `w × h` → `2w × 2h`.
///
/// Canonical AdvMAME2× / Scale2× rule (see the Wikipedia "Pixel-art scaling
/// algorithms" entry), including the `!=` guards. Those guards are what keep
/// an isolated pixel (all four orthogonal neighbours equal) at `P` instead of
/// erasing it — the original rule states "if three or more of A, B, C, D are
/// identical, 1 = 2 = 3 = 4 = P". Neighbours are edge-clamped to `P` at the
/// border, and the equality test is EXACT RGBA (alpha included), so the pass
/// is palette-pure and never blends.
fn scale2x_epx(src: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let dw = w.checked_mul(2)?;
    let dh = h.checked_mul(2)?;
    let len = dw.checked_mul(dh)?.checked_mul(BYTES_PER_PIXEL)?;
    let mut out = alloc_zeroed(len)?;

    for y in 0..h {
        for x in 0..w {
            let p = read_px(src, w, x, y);
            let a = if y > 0 { read_px(src, w, x, y - 1) } else { p };
            let b = if x + 1 < w {
                read_px(src, w, x + 1, y)
            } else {
                p
            };
            let c = if y + 1 < h {
                read_px(src, w, x, y + 1)
            } else {
                p
            };
            let d = if x > 0 { read_px(src, w, x - 1, y) } else { p };

            // Top-left, top-right, bottom-left, bottom-right quadrants.
            let e0 = if a == d && d != c && a != b { d } else { p };
            let e1 = if a == b && a != d && b != c { b } else { p };
            let e2 = if d == c && d != a && c != b { d } else { p };
            let e3 = if b == c && b != a && c != d { b } else { p };

            let ox = 2 * x;
            let oy = 2 * y;
            write_px(&mut out, dw, ox, oy, e0);
            write_px(&mut out, dw, ox + 1, oy, e1);
            write_px(&mut out, dw, ox, oy + 1, e2);
            write_px(&mut out, dw, ox + 1, oy + 1, e3);
        }
    }

    Some(out)
}

/// Reads one RGBA8 pixel as `[r, g, b, a]`.
fn read_px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
    let i = (y * w + x) * BYTES_PER_PIXEL;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// Writes one RGBA8 pixel.
fn write_px(buf: &mut [u8], w: usize, x: usize, y: usize, px: [u8; 4]) {
    let i = (y * w + x) * BYTES_PER_PIXEL;
    buf[i..i + BYTES_PER_PIXEL].copy_from_slice(&px);
}

/// Zeroed buffer allocation that never panics: returns `None` on overflow or
/// allocation failure.
fn alloc_zeroed(len: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, 0);
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::super::exact::{flip_h, flip_v, rotate_180, rotate_90_ccw, rotate_90_cw};
    use super::*;

    /// 3×3 fixture, one distinct opaque color per pixel (row-major).
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

    /// 4×4 fixture: transparent background, one distinct opaque color per
    /// pixel. The transparent pixel is part of the input palette.
    fn fixture_4x4() -> (Vec<u8>, usize, usize) {
        let mut buf = vec![0u8; 4 * 4 * 4];
        let colors: [[u8; 4]; 12] = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [255, 255, 0, 255],
            [255, 0, 255, 255],
            [0, 255, 255, 255],
            [128, 64, 32, 255],
            [32, 64, 128, 255],
            [10, 20, 30, 255],
            [40, 50, 60, 255],
            [70, 80, 90, 255],
            [100, 110, 120, 255],
        ];
        for (i, c) in colors.iter().enumerate() {
            let x = i % 4;
            let y = i / 4;
            let j = (y * 4 + x) * 4;
            buf[j..j + 4].copy_from_slice(c);
        }
        (buf, 4, 4)
    }

    /// 4×2 fixture, one distinct opaque color per pixel (row-major).
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

    /// 3×3 transparent field with a single opaque red pixel at the centre.
    fn single_pixel_3x3() -> (Vec<u8>, usize, usize) {
        let mut buf = vec![0u8; 3 * 3 * 4];
        buf[(1 * 3 + 1) * 4..(1 * 3 + 1) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
        (buf, 3, 3)
    }

    /// 5×5 transparent field with a single opaque red pixel at the centre.
    fn single_pixel_5x5() -> (Vec<u8>, usize, usize) {
        let mut buf = vec![0u8; 5 * 5 * 4];
        buf[(2 * 5 + 2) * 4..(2 * 5 + 2) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
        (buf, 5, 5)
    }

    fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// 8-connected connectivity of a set of cell coordinates.
    fn is_connected_8(cells: &[(usize, usize)]) -> bool {
        use std::collections::{HashSet, VecDeque};
        if cells.is_empty() {
            return true;
        }
        let set: HashSet<(isize, isize)> = cells
            .iter()
            .map(|&(x, y)| (x as isize, y as isize))
            .collect();
        let start = *set.iter().next().unwrap();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        seen.insert(start);
        queue.push_back(start);
        while let Some((x, y)) = queue.pop_front() {
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let n = (x + dx, y + dy);
                    if set.contains(&n) && seen.insert(n) {
                        queue.push_back(n);
                    }
                }
            }
        }
        seen.len() == set.len()
    }

    // -- Exact 90° dispatch / palette purity (unchanged contract) ------------

    #[test]
    fn rotate_0_deg_is_byte_identical_copy() {
        let (buf, w, h) = fixture_4x4();
        let (out, ow, oh) = rotate(&buf, w, h, 0.0);
        assert_eq!((ow, oh), (w, h));
        assert_eq!(out, buf);
    }

    #[test]
    fn rotate_360_deg_equals_0_deg() {
        let (buf, w, h) = fixture_4x4();
        let (a, aw, ah) = rotate(&buf, w, h, 0.0);
        let (b, bw, bh) = rotate(&buf, w, h, 360.0);
        assert_eq!((aw, ah), (bw, bh));
        assert_eq!(a, b);
    }

    #[test]
    fn rotate_is_deterministic() {
        let (buf, w, h) = fixture_4x4();
        let (a, aw, ah) = rotate(&buf, w, h, 37.0);
        let (b, bw, bh) = rotate(&buf, w, h, 37.0);
        assert_eq!((aw, ah), (bw, bh));
        assert_eq!(a, b);
    }

    #[test]
    fn rotate_90_deg_matches_exact_cw() {
        let (buf, w, h) = fixture_4x4();
        let (r, rw, rh) = rotate(&buf, w, h, 90.0);
        let (e, ew, eh) = rotate_90_cw(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);
    }

    #[test]
    fn rotate_180_deg_matches_exact_180() {
        let (buf, w, h) = fixture_4x4();
        let (r, rw, rh) = rotate(&buf, w, h, 180.0);
        let (e, ew, eh) = rotate_180(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);
    }

    #[test]
    fn rotate_270_deg_matches_exact_ccw() {
        let (buf, w, h) = fixture_4x4();
        let (r, rw, rh) = rotate(&buf, w, h, 270.0);
        let (e, ew, eh) = rotate_90_ccw(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);
    }

    #[test]
    fn rotate_neg_90_deg_matches_exact_ccw() {
        let (buf, w, h) = fixture_4x4();
        let (r, rw, rh) = rotate(&buf, w, h, -90.0);
        let (e, ew, eh) = rotate_90_ccw(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);
    }

    #[test]
    fn rotate_90_deg_non_square_dims() {
        let (buf, w, h) = fixture_4x2();
        let (_, ow, oh) = rotate(&buf, w, h, 90.0);
        assert_eq!((ow, oh), (2, 4));
    }

    #[test]
    fn rotate_45_deg_dims_match_bounding_box() {
        let (buf, w, h) = fixture_3x3();
        let (_, ow, oh) = rotate(&buf, w, h, 45.0);
        assert_eq!((ow, oh), (4, 4));
    }

    #[test]
    fn rotate_45_deg_rect_dims_match_bounding_box() {
        let (buf, w, h) = fixture_4x2();
        let (_, ow, oh) = rotate(&buf, w, h, 45.0);
        assert_eq!((ow, oh), (4, 4));
    }

    #[test]
    fn rotate_45_deg_content_present() {
        let (buf, w, h) = fixture_3x3();
        let (out, ow, oh) = rotate(&buf, w, h, 45.0);
        assert_ne!((ow, oh), (w, h), "dims must change at 45°");
        let opaque = out.chunks_exact(4).filter(|c| c[3] == 255).count();
        assert!(opaque > 0, "rotated content must be present");
    }

    #[test]
    fn rotate_45_deg_center_region_opaque_for_solid_square() {
        let (buf, w, h) = fixture_3x3();
        let (out, ow, _) = rotate(&buf, w, h, 45.0);
        for y in 1..3 {
            for x in 1..3 {
                assert_eq!(px(&out, ow, x, y)[3], 255, "hole at ({x},{y})");
            }
        }
    }

    #[test]
    fn rotate_45_deg_palette_purity() {
        let (buf, w, h) = fixture_4x4();
        let (out, _, _) = rotate(&buf, w, h, 45.0);
        let mut palette = std::collections::HashSet::new();
        for c in buf.chunks_exact(4) {
            palette.insert([c[0], c[1], c[2], c[3]]);
        }
        for c in out.chunks_exact(4) {
            let p = [c[0], c[1], c[2], c[3]];
            assert!(palette.contains(&p), "new color {p:?} created at 45°");
        }
    }

    #[test]
    fn rotate_no_aa_alpha_only_0_or_255() {
        let (buf, w, h) = fixture_4x4();
        let (out, _, _) = rotate(&buf, w, h, 45.0);
        for c in out.chunks_exact(4) {
            assert!(
                c[3] == 0 || c[3] == 255,
                "partial alpha {} (anti-aliasing) at 45°",
                c[3]
            );
        }
    }

    #[test]
    fn rotate_alpha_copied_verbatim_no_blending() {
        // 2×1: one opaque pixel, one transparent. At 45° every output pixel
        // must be either fully transparent or the exact opaque source color.
        let buf = [255u8, 0, 0, 255, 0, 0, 0, 0];
        let (out, ow, oh) = rotate(&buf, 2, 1, 45.0);
        assert_eq!((ow, oh), (2, 2));
        for c in out.chunks_exact(4) {
            assert!(c[3] == 0 || c[3] == 255, "blended alpha {}", c[3]);
            if c[3] == 255 {
                assert_eq!([c[0], c[1], c[2], c[3]], [255, 0, 0, 255]);
            }
        }
    }

    #[test]
    fn rotate_empty_buffer_returns_empty() {
        let (out, ow, oh) = rotate(&[], 0, 0, 45.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
    }

    #[test]
    fn rotate_undersized_buffer_returns_empty() {
        let (out, ow, oh) = rotate(&[0u8; 8], 3, 3, 45.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
    }

    #[test]
    fn rotate_45_deg_flip_commutes_with_rotate_90() {
        // flip_h ∘ rotate(90°) == rotate(90°) ∘ flip_v on a square.
        let (buf, w, h) = fixture_3x3();
        let (fh, fw, fh_h) = flip_h(&buf, w, h);
        let (a, aw, ah) = rotate(&fh, fw, fh_h, 90.0);
        let (r, rw, rh) = rotate(&buf, w, h, 90.0);
        let (b, bw, bh) = flip_v(&r, rw, rh);
        assert_eq!((aw, ah), (bw, bh));
        assert_eq!(a, b);
    }

    // -- Hardening: non-finite angle, bounds validation ---------------------

    #[test]
    fn rotate_non_finite_angle_returns_empty() {
        let (buf, w, h) = fixture_4x4();
        for angle in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let (out, ow, oh) = rotate(&buf, w, h, angle);
            assert!(out.is_empty(), "angle {angle} must return empty");
            assert_eq!((ow, oh), (0, 0));
        }
    }

    #[test]
    fn rotated_bounds_validates_inputs() {
        assert_eq!(rotated_bounds(0, 5, 30.0), (0, 0));
        assert_eq!(rotated_bounds(5, 0, 30.0), (0, 0));
        for angle in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(rotated_bounds(4, 4, angle), (0, 0));
        }
    }

    #[test]
    fn rotated_bounds_agrees_with_rotate_dims() {
        let (buf, w, h) = fixture_4x4();
        for angle in [1.0f32, 17.0, 30.0, 45.0, 123.0, 200.0, -40.0] {
            let (_, ow, oh) = rotate(&buf, w, h, angle);
            assert_eq!((ow, oh), rotated_bounds(w, h, angle), "angle {angle}");
        }
    }

    // -- Scale2x / EPX upscale ----------------------------------------------

    #[test]
    fn scale2x_solid_stays_solid() {
        let p = [12u8, 34, 56, 255];
        let mut buf = Vec::new();
        for _ in 0..(4 * 4) {
            buf.extend_from_slice(&p);
        }
        let out = scale2x_epx(&buf, 4, 4).expect("2x");
        assert_eq!(out.len(), 8 * 8 * 4);
        for c in out.chunks_exact(4) {
            assert_eq!([c[0], c[1], c[2], c[3]], p);
        }
    }

    #[test]
    fn scale2x_single_pixel_is_preserved() {
        let (buf, w, h) = single_pixel_3x3();
        let out = scale2x_epx(&buf, w, h).expect("2x");
        assert_eq!((w, h), (3, 3));
        assert_eq!(out.len(), 6 * 6 * 4);
        let p = [255u8, 0, 0, 255];
        let t = [0u8, 0, 0, 0];
        // Centre source pixel (1,1) → sub-pixels (2..=3, 2..=3).
        for y in 0..6 {
            for x in 0..6 {
                let expected = if (2..=3).contains(&x) && (2..=3).contains(&y) {
                    p
                } else {
                    t
                };
                assert_eq!(px(&out, 6, x, y), expected, "at ({x},{y})");
            }
        }
    }

    /// Hand-computed 2× output for a 2×2 diagonal fixture. This exercises all
    /// four EPX quadrants and the border-clamp case where a neighbour equals
    /// its diagonal counterpart (A == D).
    #[test]
    fn scale2x_diagonal_matches_hand_computed_output() {
        let p = [255u8, 0, 0, 255];
        let t = [0u8, 0, 0, 0];
        // P at (0,0) and (1,1), transparent elsewhere.
        let mut buf = Vec::new();
        for c in [p, t, t, p] {
            buf.extend_from_slice(&c);
        }
        let out = scale2x_epx(&buf, 2, 2).expect("2x");
        assert_eq!(out.len(), 4 * 4 * 4);
        let expect = [[p, p, t, t], [p, t, p, t], [t, p, t, p], [t, t, p, p]];
        for (y, row) in expect.iter().enumerate() {
            for (x, e) in row.iter().enumerate() {
                assert_eq!(px(&out, 4, x, y), *e, "at ({x},{y})");
            }
        }
    }

    #[test]
    fn upscale8x_dims_and_solid_fill() {
        let p = [9u8, 8, 7, 255];
        let mut buf = Vec::new();
        for _ in 0..(5 * 3) {
            buf.extend_from_slice(&p);
        }
        let (out, ow, oh) = upscale_scale2x(&buf, 5, 3, SCALE_LOW).expect("upscale");
        assert_eq!((ow, oh), (5 * 8, 3 * 8));
        assert_eq!(out.len(), ow * oh * 4);
        for c in out.chunks_exact(4) {
            assert_eq!([c[0], c[1], c[2], c[3]], p);
        }
    }

    #[test]
    fn upscale8x_is_palette_pure_and_alpha_binary() {
        let (buf, w, h) = fixture_4x4();
        let (out, ow, oh) = upscale_scale2x(&buf, w, h, SCALE_LOW).expect("upscale");
        assert_eq!((ow, oh), (w * 8, h * 8));
        let mut palette = std::collections::HashSet::new();
        for c in buf.chunks_exact(4) {
            palette.insert([c[0], c[1], c[2], c[3]]);
        }
        for c in out.chunks_exact(4) {
            let p = [c[0], c[1], c[2], c[3]];
            assert!(palette.contains(&p), "new color {p:?} from Scale2x");
            assert!(
                c[3] == 0 || c[3] == 255,
                "partial alpha {} from Scale2x",
                c[3]
            );
        }
    }

    /// The high-density upscale (4 EPX passes) doubles again and stays pure.
    #[test]
    fn upscale16x_dims_and_palette_purity() {
        let (buf, w, h) = fixture_4x4();
        let (out, ow, oh) = upscale_scale2x(&buf, w, h, SCALE_HIGH).expect("upscale");
        assert_eq!((ow, oh), (w * 16, h * 16));
        let mut palette = std::collections::HashSet::new();
        for c in buf.chunks_exact(4) {
            palette.insert([c[0], c[1], c[2], c[3]]);
        }
        for c in out.chunks_exact(4) {
            let p = [c[0], c[1], c[2], c[3]];
            assert!(palette.contains(&p), "new color {p:?} from Scale2x@16");
            assert!(c[3] == 0 || c[3] == 255, "partial alpha from Scale2x@16");
        }
    }

    // -- RotSprite behavioral guarantees ------------------------------------

    /// A 1px straight line rotated by a non-90° angle stays a single
    /// 8-connected run — no interior gap introduced by the rotation.
    #[test]
    fn rotate_one_px_line_stays_connected() {
        let w = 12;
        let h = 12;
        let mut buf = vec![0u8; w * h * 4];
        for x in 2..=10 {
            let i = (6 * w + x) * 4;
            buf[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        for angle in [30.0f32, 45.0] {
            let (out, ow, oh) = rotate(&buf, w, h, angle);
            let opaque: Vec<(usize, usize)> = (0..oh)
                .flat_map(|y| (0..ow).map(move |x| (x, y)))
                .filter(|&(x, y)| px(&out, ow, x, y)[3] == 255)
                .collect();
            assert!(opaque.len() >= 2, "line vanished at {angle}°");
            assert!(
                is_connected_8(&opaque),
                "rotated 1px line is disconnected at {angle}°: {opaque:?}"
            );
        }
    }

    /// An isolated opaque pixel maps to at least one output pixel.
    #[test]
    fn rotate_isolated_pixel_survives() {
        let (buf, w, h) = single_pixel_5x5();
        for angle in [30.0f32, 45.0, 60.0] {
            let (out, _, _) = rotate(&buf, w, h, angle);
            let opaque = out.chunks_exact(4).filter(|c| c[3] == 255).count();
            assert!(opaque >= 1, "isolated pixel vanished at {angle}°");
            for c in out.chunks_exact(4) {
                assert!(
                    c[3] == 0 || (c[0], c[1], c[2]) == (255, 0, 0),
                    "unexpected color at {angle}°: {c:?}"
                );
            }
        }
    }

    // -- Memory guard + fallback --------------------------------------------

    #[test]
    fn rotsprite_bytes_is_deterministic_and_overflow_safe() {
        assert_eq!(
            rotsprite_bytes(128, 128, SCALE_LOW),
            Some(128 * 128 * 64 * 4)
        );
        assert!(rotsprite_bytes(128, 128, SCALE_LOW).unwrap() <= ROTSPRITE_TILE_BYTES);
        // 725² needs 725·725·256 ≈ 134.56 MB > 8 MiB tile budget.
        assert!(rotsprite_bytes(725, 725, SCALE_LOW).unwrap() > ROTSPRITE_TILE_BYTES);
        // Overflowing dims must not panic.
        assert_eq!(rotsprite_bytes(usize::MAX, 1, SCALE_LOW), None);
        assert_eq!(rotsprite_bytes(1, usize::MAX, SCALE_LOW), None);
    }

    #[test]
    fn memory_guard_fallback_is_deterministic() {
        // The `rotate_nn` fallback now exists only for inputs the tiled core
        // cannot represent (invalid/zero dims) or allocation failure — large
        // but valid sources are tiled instead (see the large-source test).
        // `rotate_rotsprite` reports those cases with `None`...
        assert!(rotate_rotsprite(&[], 0, 0, 30.0).is_none());
        // ...and the public entry point still degrades safely.
        let (out, ow, oh) = rotate(&[], 0, 0, 30.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
        let (out, ow, oh) = rotate(&[0u8; 8], 3, 3, 30.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
    }

    #[test]
    fn small_source_uses_rotsprite_path() {
        // A small source uses the whole-image RotSprite core.
        let (buf, w, h) = fixture_4x4();
        assert!(rotate_rotsprite(&buf, w, h, 30.0).is_some());
    }

    // -- Y1: bounded-memory tiling ------------------------------------------

    /// Whole-image path, forced (huge tile budget).
    fn untiled(
        buf: &[u8],
        w: usize,
        h: usize,
        angle: f32,
        forced: Option<(usize, usize)>,
        restore: bool,
    ) -> Option<(Vec<u8>, usize, usize)> {
        rotate_rotsprite_cfg(buf, w, h, angle, forced, restore, usize::MAX, 8)
    }

    /// Tiled path, forced (zero tile budget) with a given output tile edge.
    fn tiled(
        buf: &[u8],
        w: usize,
        h: usize,
        angle: f32,
        forced: Option<(usize, usize)>,
        restore: bool,
        tile: usize,
    ) -> Option<(Vec<u8>, usize, usize)> {
        rotate_rotsprite_cfg(buf, w, h, angle, forced, restore, 0, tile)
    }

    /// The tiled path must be BYTE-IDENTICAL to the whole-image path: the
    /// global phase, offset, vote, restore and repair are all reproduced
    /// exactly, independent of the tile size.
    #[test]
    fn tiled_equals_untiled_byte_identical() {
        let fixtures = [
            fixture_3x3(),
            fixture_4x4(),
            fixture_4x2(),
            single_pixel_5x5(),
            single_pixel_3x3(),
        ];
        for (buf, w, h) in fixtures {
            for angle in [8.0f32, 17.0, 30.0, 37.0, 45.0, 53.0, 123.0, -40.0] {
                for forced in [None, Some((0, 0)), Some((3, 5))] {
                    for restore in [false, true] {
                        let a = untiled(&buf, w, h, angle, forced, restore).expect("untiled");
                        for tile in [2usize, 3, 4, 7, 32] {
                            let b = tiled(&buf, w, h, angle, forced, restore, tile).expect("tiled");
                            assert_eq!(
                                a, b,
                                "tile={tile} angle={angle} forced={forced:?} restore={restore} {w}x{h}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// A long 1px diagonal crossing many forced tiles has no seams: the tiled
    /// result equals the whole-image result exactly.
    #[test]
    fn no_tile_boundary_artifacts() {
        let (w, h) = (24usize, 24usize);
        let mut buf = vec![0u8; w * h * 4];
        for i in 0..w.min(h) {
            let j = (i * w + i) * 4;
            buf[j..j + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        for angle in [30.0f32, 45.0] {
            let a = untiled(&buf, w, h, angle, None, true).expect("untiled");
            for tile in [5usize, 10] {
                let b = tiled(&buf, w, h, angle, None, true, tile).expect("tiled");
                assert_eq!(a, b, "tile={tile} angle={angle} (seam)");
            }
        }
    }

    /// A source that exceeds the tiling budget no longer falls back to plain
    /// NN: it tiles and stays RotSprite, deterministically. 64² at 30° has a
    /// whole-image peak of ~12.9 MB (> the 8 MiB tiling budget), so the default
    /// `rotate` takes the tiled path.
    #[test]
    fn large_source_uses_rotsprite_not_nn_fallback() {
        let side = 64usize;
        let mut buf = vec![0u8; side * side * 4];
        for i in 0..side {
            let j = (i * side + i) * 4;
            buf[j..j + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        assert!(rotsprite_peak_bytes(side, side, 30.0).unwrap() > ROTSPRITE_TILE_BYTES);

        let (a, aw, ah) = rotate(&buf, side, side, 30.0);
        let (b, bw, bh) = rotate(&buf, side, side, 30.0);
        assert_eq!((aw, ah), (bw, bh));
        assert_eq!(a, b, "tiled large source must be deterministic");
        assert_eq!((aw, ah), rotated_bounds(side, side, 30.0));

        // The default path is the tiled RotSprite core, identical to forcing
        // tiling with a zero budget.
        let forced =
            tiled(&buf, side, side, 30.0, None, true, ROTSPRITE_TILE).expect("forced tiled");
        assert_eq!(a, forced.0, "default path must equal forced tiling");
        assert_eq!((aw, ah), (forced.1, forced.2));

        let (nn, ..) = rotate_nn(&buf, side, side, 30.0);
        assert_ne!(
            a, nn,
            "large source must use RotSprite, not the NN fallback"
        );
    }

    // -- Phase 2: plurality vote + grid-fit + restore -----------------------

    #[test]
    fn rotsprite_peak_bytes_covers_rotated_buffer() {
        // Peak is strictly larger than the plain upscale (it adds the S×
        // rotated buffer) and overflows safely. 30° selects S = 16.
        assert_eq!(adaptive_scale(30.0), SCALE_HIGH);
        assert!(
            rotsprite_peak_bytes(128, 128, 30.0).unwrap()
                > rotsprite_bytes(128, 128, SCALE_HIGH).unwrap()
        );
        assert!(rotsprite_peak_bytes(128, 128, 30.0).unwrap() > ROTSPRITE_TILE_BYTES);
        // A small source is under the tiling threshold.
        assert!(rotsprite_peak_bytes(20, 20, 30.0).unwrap() <= ROTSPRITE_TILE_BYTES);
        // A huge source is over budget for any S.
        assert!(rotsprite_peak_bytes(725, 725, 30.0).unwrap() > ROTSPRITE_TILE_BYTES);
        assert_eq!(rotsprite_peak_bytes(usize::MAX, 1, 30.0), None);
        // Deterministic in (w, h, angle).
        assert_eq!(
            rotsprite_peak_bytes(64, 32, 17.0),
            rotsprite_peak_bytes(64, 32, 17.0)
        );
        // S = 16 costs strictly more than S = 8 at the same dims.
        assert!(
            rotsprite_peak_bytes(64, 64, 45.0).unwrap()
                > rotsprite_peak_bytes(64, 64, 5.0).unwrap()
        );
    }

    // -- Adaptive sampling density S ----------------------------------------

    #[test]
    fn adaptive_scale_is_8_near_axis_and_16_near_45() {
        // Near the axis multiples (d < 22.5°) → S = 8.
        for angle in [0.1f32, 5.0, 20.0, 22.4, -5.0, 95.0, 180.1, 269.0] {
            assert_eq!(adaptive_scale(angle), SCALE_LOW, "angle {angle}");
        }
        // Near 45° (d >= 22.5°) → S = 16.
        for angle in [22.5f32, 30.0, 45.0, 60.0, 67.5, -45.0, 135.0, 225.0, 315.0] {
            assert_eq!(adaptive_scale(angle), SCALE_HIGH, "angle {angle}");
        }
        // Deterministic.
        for angle in [0.0f32, 22.5, 45.0, 123.456] {
            assert_eq!(adaptive_scale(angle), adaptive_scale(angle));
        }
    }

    /// Synthetic check of the block indexing: an 8×8 red block in an otherwise
    /// transparent 16×16 rotated buffer votes red at offset (0,0) and
    /// transparent at offset (8,8).
    #[test]
    fn downscale_vote_picks_block_plurality() {
        let d = 16usize;
        let p = [9u8, 8, 7, 255];
        let mut rot = vec![0u8; d * d * 4];
        for y in 0..8 {
            for x in 0..8 {
                write_px(&mut rot, d, x, y, p);
            }
        }
        let out = downscale_vote(&rot, d, d, 1, 1, 0, 0, SCALE_LOW).expect("vote");
        assert_eq!(px(&out, 1, 0, 0), p);
        let out = downscale_vote(&rot, d, d, 1, 1, 8, 8, SCALE_LOW).expect("vote");
        assert_eq!(px(&out, 1, 0, 0), [0, 0, 0, 0]);

        // A 2:1 opaque/transparent split with opaque weighting: 24 red (weight
        // 48) vs 40 transparent (weight 40) → red.
        let mut rot2 = vec![0u8; d * d * 4];
        for y in 0..3 {
            for x in 0..8 {
                write_px(&mut rot2, d, x, y, p);
            }
        }
        let out = downscale_vote(&rot2, d, d, 1, 1, 0, 0, SCALE_LOW).expect("vote");
        assert_eq!(px(&out, 1, 0, 0), p, "opaque weighting must win 24-vs-40");
    }

    #[test]
    fn choose_offset_is_bounded_and_deterministic() {
        let (buf, w, h) = fixture_4x4();
        let scale = adaptive_scale(30.0);
        let (up, uw, uh) = upscale_scale2x(&buf, w, h, scale).unwrap();
        let (wp, hp) = rotated_bounds(w, h, 30.0);
        let dw = wp * scale + scale;
        let dh = hp * scale + scale;
        let rot = rotate_scaled(&up, uw, uh, w, h, wp, hp, dw, dh, 30.0, scale, 0.0).unwrap();
        let (a, sa) = choose_offset(&rot, dw, dh, scale);
        let (b, sb) = choose_offset(&rot, dw, dh, scale);
        assert_eq!((a, sa), (b, sb), "offset choice must be deterministic");
        assert!(a.0 < scale && a.1 < scale, "offset out of range: {a:?}");
    }

    #[test]
    fn majority_vote_is_palette_pure_and_alpha_binary() {
        let (buf, w, h) = fixture_4x4();
        let mut palette = std::collections::HashSet::new();
        for c in buf.chunks_exact(4) {
            palette.insert([c[0], c[1], c[2], c[3]]);
        }
        for angle in [7.0f32, 17.0, 30.0, 45.0, 123.0] {
            let (out, _, _) = rotate(&buf, w, h, angle);
            for c in out.chunks_exact(4) {
                let p = [c[0], c[1], c[2], c[3]];
                assert!(palette.contains(&p), "new color {p:?} at {angle}°");
                assert!(c[3] == 0 || c[3] == 255, "partial alpha at {angle}°");
            }
        }
    }

    /// `restore_isolated` recovers the centre pixel even when the plurality
    /// vote (no restore) erases it.
    #[test]
    fn restore_recovers_isolated_pixel_erased_by_vote() {
        let (buf, w, h) = single_pixel_5x5();
        let mut gained_any = false;
        for angle in [8.0f32, 20.0, 30.0, 37.0, 45.0, 53.0, 60.0, 77.0] {
            let (voted, ..) = rotate_rotsprite_opts(&buf, w, h, angle, None, false).unwrap();
            let (restored, ..) = rotate_rotsprite_opts(&buf, w, h, angle, None, true).unwrap();
            let vote_opaque = voted.chunks_exact(4).filter(|c| c[3] == 255).count();
            let restore_opaque = restored.chunks_exact(4).filter(|c| c[3] == 255).count();
            println!("isolated angle={angle:>4}: vote={vote_opaque} restore={restore_opaque}");
            assert!(
                restore_opaque >= vote_opaque && restore_opaque >= 1,
                "restore must never lose the lone pixel (angle {angle}°)"
            );
            if restore_opaque > vote_opaque {
                gained_any = true;
            }
        }
        assert!(
            gained_any,
            "restore must recover detail the vote alone dropped"
        );
    }

    // -- Thin-line repair ---------------------------------------------------

    #[test]
    fn thin_line_repair_fills_single_gap_only() {
        let p = [255u8, 0, 0, 255];
        let t = [0u8, 0, 0, 0];

        // A 1px diagonal with a single-pixel break at (2,2).
        let mut buf = vec![0u8; 5 * 5 * 4];
        for (x, y) in [(0usize, 0usize), (1, 1), (3, 3), (4, 4)] {
            write_px(&mut buf, 5, x, y, p);
        }
        repair_thin_lines(&mut buf, 5, 5);
        assert_eq!(px(&buf, 5, 2, 2), p, "1px diagonal gap must be repaired");

        // A 2px gap must NOT be repaired (conservative: gap cells have < 2
        // opposite opaque neighbours).
        let mut buf = vec![0u8; 5 * 5 * 4];
        for (x, y) in [(0usize, 0usize), (1, 1), (4, 4)] {
            write_px(&mut buf, 5, x, y, p);
        }
        repair_thin_lines(&mut buf, 5, 5);
        assert_eq!(px(&buf, 5, 2, 2), t, "2px gap must not be bridged");
        assert_eq!(px(&buf, 5, 3, 3), t, "2px gap must not be bridged");

        // A solid blob with a 1px hole must NOT be filled (hole has 8 opaque
        // neighbours, not 2).
        let mut buf = Vec::new();
        for _ in 0..(3 * 3) {
            buf.extend_from_slice(&p);
        }
        write_px(&mut buf, 3, 1, 1, t);
        repair_thin_lines(&mut buf, 3, 3);
        assert_eq!(px(&buf, 3, 1, 1), t, "blob hole must not be filled");

        // A deliberate 1px vertical separator between two same-colored columns
        // must NOT be filled (gap cells have 6 opaque neighbours, not 2).
        let mut buf = vec![0u8; 3 * 3 * 4];
        for y in 0..3 {
            write_px(&mut buf, 3, 0, y, p);
            write_px(&mut buf, 3, 2, y, p);
        }
        repair_thin_lines(&mut buf, 3, 3);
        for y in 0..3 {
            assert_eq!(px(&buf, 3, 1, y), t, "separator column must stay open");
        }

        // A horizontal line break is repaired too.
        let mut buf = vec![0u8; 5 * 3 * 4];
        for x in [0usize, 1, 3, 4] {
            write_px(&mut buf, 5, x, 1, p);
        }
        repair_thin_lines(&mut buf, 5, 3);
        assert_eq!(px(&buf, 5, 2, 1), p, "horizontal 1px gap must be repaired");
    }

    #[test]
    fn thin_line_repair_is_palette_pure_and_deterministic() {
        let (buf, w, h) = fixture_4x4();
        let mut a = buf.clone();
        let mut b = buf.clone();
        repair_thin_lines(&mut a, w, h);
        repair_thin_lines(&mut b, w, h);
        assert_eq!(a, b, "repair must be deterministic");
        let palette: std::collections::HashSet<[u8; 4]> = buf
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        for c in a.chunks_exact(4) {
            assert!(palette.contains(&[c[0], c[1], c[2], c[3]]), "new color");
            assert!(c[3] == 0 || c[3] == 255, "partial alpha");
        }
    }

    /// The half-subpixel phase shifts the sampling grid (distinct NN result)
    /// and stays deterministic and palette-pure (NN-only, no interpolation).
    #[test]
    fn subpixel_phase_changes_sampling_and_is_deterministic() {
        let (buf, w, h) = line_fixture_32x32();
        let angle = 8.0f32; // near-axis: S = 8, so the phase search is active
        let scale = adaptive_scale(angle);
        assert_eq!(scale, SCALE_LOW);
        let (up, uw, uh) = upscale_scale2x(&buf, w, h, scale).unwrap();
        let (wp, hp) = rotated_bounds(w, h, angle);
        let dw = wp * scale + scale;
        let dh = hp * scale + scale;

        let p0 = rotate_scaled(&up, uw, uh, w, h, wp, hp, dw, dh, angle, scale, 0.0).unwrap();
        let p0_again = rotate_scaled(&up, uw, uh, w, h, wp, hp, dw, dh, angle, scale, 0.0).unwrap();
        assert_eq!(p0, p0_again, "phase 0 must be deterministic");
        let p_half = rotate_scaled(&up, uw, uh, w, h, wp, hp, dw, dh, angle, scale, 0.5).unwrap();
        assert_ne!(p0, p_half, "half phase must change the sampled grid");

        // Every sampled color is a real upscale color (NN-only).
        let up_palette: std::collections::HashSet<[u8; 4]> = up
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        for c in p_half.chunks_exact(4) {
            assert!(up_palette.contains(&[c[0], c[1], c[2], c[3]]));
        }

        // The full path (which evaluates both phases and picks by metric) is
        // deterministic.
        let a = rotate_rotsprite_opts(&buf, w, h, angle, None, true);
        let b = rotate_rotsprite_opts(&buf, w, h, angle, None, true);
        assert_eq!(a, b);
    }

    /// Deterministic quality metrics for the comparison experiment.
    fn opaque_cells(out: &[u8], w: usize, h: usize) -> Vec<(usize, usize)> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| px(out, w, x, y)[3] == 255)
            .collect()
    }

    /// Size of the largest **4-connected** component of `cells`.
    fn largest_component_4(cells: &[(usize, usize)]) -> usize {
        use std::collections::{HashSet, VecDeque};
        let set: HashSet<(isize, isize)> = cells
            .iter()
            .map(|&(x, y)| (x as isize, y as isize))
            .collect();
        let mut seen = HashSet::new();
        let mut largest = 0usize;
        for &start in &set {
            if seen.contains(&start) {
                continue;
            }
            let mut size = 0usize;
            let mut queue = VecDeque::new();
            seen.insert(start);
            queue.push_back(start);
            while let Some((x, y)) = queue.pop_front() {
                size += 1;
                for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                    let n = (x + dx, y + dy);
                    if set.contains(&n) && seen.insert(n) {
                        queue.push_back(n);
                    }
                }
            }
            largest = largest.max(size);
        }
        largest
    }

    /// `(opaque, broken)`: number of opaque output pixels, and the number not
    /// reachable from the largest **4-connected** run. A rotated 1px line must
    /// stay orthogonally contiguous; plain NN leaves diagonal-only steps
    /// (jaggies), while the vote + grid-fit repair them. Lower `broken` is
    /// better and `0` means one solid orthogonal run.
    fn line_integrity(out: &[u8], w: usize, h: usize) -> (usize, usize) {
        let cells = opaque_cells(out, w, h);
        let largest = largest_component_4(&cells);
        (cells.len(), cells.len() - largest)
    }

    fn line_fixture_32x32() -> (Vec<u8>, usize, usize) {
        let (w, h) = (32usize, 32usize);
        let mut buf = vec![0u8; w * h * 4];
        for x in 8..24 {
            let i = (16 * w + x) * 4;
            buf[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        (buf, w, h)
    }

    /// **Comparison experiment** (run with `--nocapture`): plain NN vs
    /// plurality vote (forced offset 0, no restore) vs vote + offset-fit +
    /// restore on a 16px 1px line.
    ///
    /// Metric: `broken` = opaque pixels not in the largest 4-connected run
    /// (orthogonal line continuity, lower is better). The strict ordering
    /// `NN ≥ majority ≥ vote+offset+restore` is asserted on the reference
    /// angles 8/15/30/45°; on every probed angle the final result must not be
    /// worse than either baseline.
    #[test]
    fn comparison_nn_majority_offset_restore() {
        let (buf, w, h) = line_fixture_32x32();
        for angle in [7.0f32, 8.0, 15.0, 22.0, 30.0, 37.0, 45.0, 53.0] {
            let (nn, nw, nh) = rotate_nn(&buf, w, h, angle);
            let (maj, mw, mh) =
                rotate_rotsprite_opts(&buf, w, h, angle, Some((0, 0)), false).expect("majority");
            let (opt, ow, oh) = rotate_rotsprite(&buf, w, h, angle).expect("rotsprite");
            assert_eq!((nw, nh), (mw, mh));
            assert_eq!((nw, nh), (ow, oh));

            let (n_opaque, n_broken) = line_integrity(&nn, nw, nh);
            let (m_opaque, m_broken) = line_integrity(&maj, mw, mh);
            let (o_opaque, o_broken) = line_integrity(&opt, ow, oh);

            println!(
                "line angle={angle:>4}: NN opaque={n_opaque:>3} broken={n_broken:>3} | \
                 majority opaque={m_opaque:>3} broken={m_broken:>3} | \
                 vote+offset+restore opaque={o_opaque:>3} broken={o_broken:>3}"
            );

            assert!(
                o_broken <= n_broken && o_broken <= m_broken,
                "final regressed at {angle}°: NN={n_broken} maj={m_broken} opt={o_broken}"
            );
            assert!(o_opaque >= 2, "line vanished at {angle}°");

            if matches!(angle, 8.0 | 15.0 | 30.0 | 45.0) {
                assert!(
                    n_broken >= m_broken && m_broken >= o_broken,
                    "ordering NN ≥ majority ≥ final failed at {angle}°"
                );
            }
        }
    }

    /// Comparison on isolated-pixel survival (0 = lost, 1 = kept).
    #[test]
    fn comparison_isolated_pixel_survival() {
        let (buf, w, h) = single_pixel_5x5();
        for angle in [30.0f32, 45.0, 60.0] {
            let (nn, ..) = rotate_nn(&buf, w, h, angle);
            let (maj, ..) = rotate_rotsprite_opts(&buf, w, h, angle, Some((0, 0)), false).unwrap();
            let (opt, ..) = rotate_rotsprite(&buf, w, h, angle).unwrap();
            let survival = |o: &[u8]| usize::from(o.chunks_exact(4).any(|c| c[3] == 255));
            let (s_nn, s_maj, s_opt) = (survival(&nn), survival(&maj), survival(&opt));
            println!(
                "isolated angle={angle:>4}: NN={s_nn} majority={s_maj} vote+offset+restore={s_opt}"
            );
            assert!(
                s_maj >= s_nn && s_opt >= s_maj,
                "isolated survival regressed at {angle}°: {s_nn} {s_maj} {s_opt}"
            );
            assert_eq!(s_opt, 1, "restore must keep the isolated pixel at {angle}°");
        }
    }

    /// Determinism guard: the RotSprite path (adaptive-S upscale →
    /// S×-rotated buffer → plurality vote + S-grid/half-phase offset fit +
    /// isolated/thin-line restore) must return BYTE-IDENTICAL output for the
    /// same input+angle across repeated runs.
    ///
    /// The production path uses only fixed-size stack arrays for the vote
    /// tallies and the residue histograms (no `HashMap`/`HashSet`), scans in
    /// row-major order, and breaks ties with order-independent rules (a strict
    /// `<` keeps the lowest grid offset; an equal-weight vote picks the
    /// smallest `[r, g, b, a]` key). This test pins that contract so a future
    /// switch to a hash-ordered structure cannot silently reintroduce
    /// non-determinism.
    #[test]
    fn rotsprite_is_byte_identical_across_repeated_runs() {
        for (buf, w, h) in [
            single_pixel_5x5(),
            fixture_3x3(),
            fixture_4x4(),
            fixture_4x2(),
        ] {
            for angle in [
                0.0f32, 8.0, 20.0, 22.5, 30.0, 45.0, 53.0, 60.0, 67.5, 90.0, 112.5, 150.0, 157.5,
                200.0, 270.0, 315.0, 337.5,
            ] {
                for restore in [false, true] {
                    // Whole-image and tiled (small/large tile) configurations
                    // must each be deterministic across repeated runs.
                    for &(budget, tile) in
                        &[(usize::MAX, 8usize), (0usize, 3usize), (0usize, 7usize)]
                    {
                        let first =
                            rotate_rotsprite_cfg(&buf, w, h, angle, None, restore, budget, tile);
                        for _ in 0..3 {
                            let again = rotate_rotsprite_cfg(
                                &buf, w, h, angle, None, restore, budget, tile,
                            );
                            assert_eq!(
                                first, again,
                                "RotSprite must be deterministic for {w}x{h} @ {angle}° (restore={restore}, budget={budget}, tile={tile})"
                            );
                        }
                    }
                }
            }
        }
    }
}
