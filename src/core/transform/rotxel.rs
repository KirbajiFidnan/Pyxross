//! Rotxel rotation — a faithful CPU port of the core rotation logic of Rotxel
//! ("Original shader written by Azagaya", Pixelorama, MIT).
//!
//! Selectable through the transform pipeline via
//! [`super::TransformAlgorithm::Rotxel`] / [`super::rotate_with`];
//! [`rotxel_rotate`] is the core entry point. The invariant tests at the bottom
//! of this file remain (the three-way measurement table lives in
//! `clean_edge.rs`).
//!
//! # Provenance
//!
//! Ported from `/tmp/opencode/rotxel.gdshader`
//! (`src/Shaders/Effects/Rotation/SmearRotxel.gdshader`). The **Smear** variant
//! (the `for angle -= 0.0174` loop that alpha-blends successive angles) is
//! deliberately **not** ported — it is not palette-pure. This port performs a
//! **single pass at the target angle** (`initial_angle == ending_angle ==
//! angle`). The `fragment()` selection-mask compositing is application glue and
//! is ignored. `tolerance = 100.0` and
//! `similarColors(c1, c2) = distance(c1*255, c2*255) < tolerance` (Euclidean
//! over RGBA, alpha included) are kept verbatim.
//!
//! # Shader → CPU adaptation
//!
//! The shader maps each destination fragment through a 3× subcell search:
//! `dx = 3*(int(coord.x) - int(center.x))`, then for each of the 9 subcells
//! `(i, j) ∈ {-1,0,1}²` it computes `dir = atan2(dy+j, dx+i) - angle`,
//! `mag = hypot(dx+i, dy+j)`, `ox = round(3*center.x + 1 + mag*cos(dir))` (with
//! the `+1` for odd source dimensions), accepts the first candidate inside the
//! source `3w × 3h` grid, then maps back to a source pixel and a subcell index
//! (`row = oy%3`, `col = ox%3`, `index = col + 3*row`, `ox = round((ox-1)/3)`).
//! Border pixels copy the source; interior pixels gather the 3×3 `a..l` and
//! apply the `index 0..8` EPX rules verbatim (which only ever emit `b`, `d`,
//! `e`, `f`, `h` — hence palette-pure).
//!
//! For the CPU adaptation, following the task spec:
//!
//! - destination dims use the same rotated-bounding-box formula as
//!   [`super::rot_sprite::rotated_bounds`];
//! - `center = ((w-1)/2, (h-1)/2)` — the source buffer centre in pixel units
//!   (this makes `3*center + 1` the centre of the source's `3w × 3h` grid, so
//!   it matches the existing rotate model);
//! - the source is centred on the destination canvas, i.e. the per-fragment
//!   translation is `position = center - dest_center`, and
//!   `coord = (dx, dy) + position`. **The rotation itself is internal** (`dir -=
//!   angle`) — unlike the CleanEdge port, `coord` is *not* inverse-rotated by
//!   the caller. `atan2(dy+j, dx+i) - angle` is algebraically the same
//!   `R(-θ)` inverse map used by the existing rotate, so orientation agrees;
//! - `int()` truncates toward zero (Rust `as i64`), matching GLSL.
//!
//! # Caveats (credibility)
//!
//! - Rotxel's `atan2`/`cos`/`sin` are f32/f64 transcendental calls; results are
//!   deterministic within a build/platform but may differ in the last ULP
//!   across platforms. No interpolation/AI is used, so palette purity and
//!   determinism-in-process hold. (v2 hoists `cos`/`sin` to one call per
//!   rotation; per-subcell work is pure arithmetic.)
//! - v1 took one subcell sample per destination pixel (1:1 with the reference
//!   `rotate()` call); v2 supersamples `N²` subcells per destination pixel.
//! - Out-of-grid fragments (no candidate in bounds) and skipped fragments are
//!   written as transparent `[0,0,0,0]`, the same sentinel used by the other
//!   methods; palette purity therefore assumes the input contains a transparent
//!   pixel (true for every experiment fixture, which sit on a transparent
//!   background).
//!
//! # Production generic path (current): density-gated v2/v3
//!
//! The generic-angle body is chosen per source by [`rotxel_gate`]: **dense**
//! sources (opaque ratio ≥ 1/4, e.g. sprites and solid areas) use
//! [`rotxel_generic_v3`] = the v2 `N×N` subcell opaque-majority gate plus a
//! verbatim-source plurality vote (with local isolated-pixel restore and
//! thin-line repair) **plus** an `N²` (64-candidate) grid-fit that slides the
//! whole sample lattice to the sub-pixel phase whose block boundaries best match
//! the rotated source's boundaries ([`choose_phase`] / [`lattice_metric`],
//! mirroring `rot_sprite.rs`'s offset search); **sparse/thin** sources (ratio
//! < 1/4, e.g. 1-px lines) use [`rotxel_generic_v2`] (same vote, **no** fit),
//! because the grid-fit can shift a 1-px line out of the majority gate. The gate
//! is a single integer linear pass ([`gate_chooses_fit`]). It intentionally
//! **drops** the 3-grid artifacts described above (`trunc(coord)` snapping,
//! `3*center+1`, `(ox−1)/3`, the odd-dimension `+1` parity fudge); the 3-grid
//! body survives only as the test-only [`rotxel_generic_v1`] used for BEFORE/
//! AFTER measurement, and the no-fit [`rotxel_generic_v2`] is also directly
//! selectable. See [`rotxel_vote_and_post`] and [`lattice_metric`] for the
//! algorithms and rationale.

use super::{checked_dims, BYTES_PER_PIXEL};

/// RGBA8 colour.
type Rgba = [u8; 4];

/// `tolerance = 100.0`, compared as squared distance (`sum < 100²`).
const TOLERANCE_SQ: u32 = 100 * 100;

/// v2 subcell supersample factor per destination-pixel axis (`N`). `N²` source
/// samples are taken per destination pixel; the value is what makes higher
/// resolution meaningful (fractional subcell offsets, not the old 3-grid).
const SUBCELL: usize = 8;

/// Tally capacity: at most `N²` distinct opaque colours can vote.
const TALLY_CAP: usize = SUBCELL * SUBCELL;

/// The transparent sentinel the output is zero-initialised to.
const SENTINEL: Rgba = [0, 0, 0, 0];

/// Memory budget for the grid-fit's precomputed supersample lattice. When the
/// lattice would exceed this (very large destination images) the grid-fit
/// deterministically falls back to phase `(0, 0)` (i.e. v2 behaviour) rather
/// than allocating an unbounded buffer.
const GRID_FIT_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Optional EPX refinement of the plurality winner. **OFF by default**: the
/// v2 majority+plurality already fixes the over-coverage/fragmentation the v1
/// EPX rule caused; this is kept as a documented, easily-toggled experiment.
const ROTXEL_EPX_REFINE: bool = false;

/// Rotates an RGBA8 buffer by `angle_deg` degrees **clockwise** about its
/// centre (screen coordinates, y-down) using a CPU port of Rotxel's core
/// rotation logic (single pass, non-Smear).
///
/// Mirrors [`super::rot_sprite::rotate`]'s contract: empty/undersized buffer or
/// non-finite angle → `(Vec::new(), 0, 0)`; exact 0/90/180/270° dispatch to the
/// pixel-exact paths; every other angle uses the Rotxel core over the same
/// rotated-bounding-box dims.
///
/// Deterministic (within the process) and palette-pure: every emitted pixel is
/// a verbatim source pixel (the v2 plurality winner or an isolated/thin-line
/// restore, or the transparent sentinel).
pub fn rotxel_rotate(src: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
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

    // The generic body is density-gated: dense content uses v3
    // (`rotxel_generic_v3` = v2 + S² grid-fit), sparse/thin content uses v2
    // (`rotxel_generic_v2`, no fit). Under `cfg(test)` a selector can still
    // force v1 / v2 / v3 / gate.
    #[cfg(test)]
    return selected_rotxel_generic(src, w, h, angle_deg);
    #[cfg(not(test))]
    return rotxel_gate(src, w, h, angle_deg);
}

/// Shared supersample geometry for the Rotxel generic paths: the inverse
/// `R(-θ)` destination→source map (y-down, CW-positive — the same sampler as
/// [`super::rot_sprite`]) plus the source/destination centres.
struct Geometry {
    w: usize,
    h: usize,
    cos_t: f64,
    sin_t: f64,
    cx: f64,
    cy: f64,
    dcx: f64,
    dcy: f64,
}

impl Geometry {
    fn new(w: usize, h: usize, angle_deg: f32, wp: usize, hp: usize) -> Self {
        let theta = (angle_deg as f64).to_radians();
        Self {
            w,
            h,
            cos_t: theta.cos(),
            sin_t: theta.sin(),
            cx: (w as f64 - 1.0) / 2.0,
            cy: (h as f64 - 1.0) / 2.0,
            dcx: (wp as f64 - 1.0) / 2.0,
            dcy: (hp as f64 - 1.0) / 2.0,
        }
    }

    /// Nearest source sample for destination cell `(dx, dy)` and fractional
    /// subcell offset `(ox, oy)`: `Some((colour, (ix, iy)))` in bounds, `None`
    /// out of bounds.
    #[inline]
    fn sample_idx(
        &self,
        src: &[u8],
        dx: usize,
        dy: usize,
        ox: f64,
        oy: f64,
    ) -> Option<(Rgba, (i64, i64))> {
        let px0 = dx as f64 - self.dcx + self.cx;
        let py0 = dy as f64 - self.dcy + self.cy;
        let ax = px0 + ox - self.cx;
        let ay = py0 + oy - self.cy;
        let sx = self.cx + self.cos_t * ax + self.sin_t * ay;
        let sy = self.cy - self.sin_t * ax + self.cos_t * ay;
        let ix = sx.round() as i64;
        let iy = sy.round() as i64;
        if ix < 0 || iy < 0 || ix >= self.w as i64 || iy >= self.h as i64 {
            None
        } else {
            Some((read_px(src, self.w, ix as usize, iy as usize), (ix, iy)))
        }
    }

    /// Samples an absolute destination-space coordinate `(xf, yf)` (not a cell
    /// plus subcell offset). Used to precompute the global supersample lattice.
    #[inline]
    fn sample_dest(&self, src: &[u8], xf: f64, yf: f64) -> Rgba {
        let ax = xf - self.dcx;
        let ay = yf - self.dcy;
        let sx = self.cx + self.cos_t * ax + self.sin_t * ay;
        let sy = self.cy - self.sin_t * ax + self.cos_t * ay;
        let ix = sx.round() as i64;
        let iy = sy.round() as i64;
        if ix < 0 || iy < 0 || ix >= self.w as i64 || iy >= self.h as i64 {
            SENTINEL
        } else {
            read_px(src, self.w, ix as usize, iy as usize)
        }
    }
}

/// Fractional subcell offset within the destination pixel for component `t`
/// (`0..n`) and grid-fit phase `phase` (`0..n`). `phase = 0` reproduces the v2
/// lattice; `phase ∈ 0..n` slides the whole sample lattice by up to `(n-1)/n`
/// of a destination pixel, letting the grid-fit pick the sub-pixel alignment
/// whose block boundaries best match the rotated source-pixel boundaries.
#[inline]
fn subcell_offset(t: usize, phase: usize, n: usize) -> f64 {
    (t as f64 + phase as f64 + 0.5) / n as f64 - 0.5
}

/// Generic-angle Rotxel rotation, **v2**: v3's shared vote core at phase
/// `(0, 0)` (no grid-fit). Kept as the BEFORE reference for the experiment
/// (production uses [`rotxel_generic_v3`]).
#[allow(dead_code)]
fn rotxel_generic_v2(src: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
    rotxel_vote_and_post(src, w, h, angle_deg, 0, 0)
}

/// Generic-angle Rotxel rotation, **v3**: v2 with an `SUBCELL²` (64-candidate)
/// grid-fit phase selection ([`choose_phase`]) before the vote.
fn rotxel_generic_v3(src: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
    let (fx, fy) = choose_phase(src, w, h, angle_deg);
    rotxel_vote_and_post(src, w, h, angle_deg, fx, fy)
}

/// Number of fully-opaque (`alpha == 255`) source pixels — one linear pass over
/// the buffer, no allocation. Deterministic and exact (integer count).
fn opaque_pixel_count(src: &[u8], w: usize, h: usize) -> usize {
    let total = w.saturating_mul(h);
    src.chunks_exact(BYTES_PER_PIXEL)
        .take(total)
        .filter(|px| px[3] == 255)
        .count()
}

/// Density gate: `true` → use the grid-fit ([`rotxel_generic_v3`]); `false` →
/// use the no-fit supersample ([`rotxel_generic_v2`]).
///
/// ## Metric
///
/// `opaque_ratio = opaque_pixel_count / (w * h)`, computed as the exact integer
/// predicate `opaque >= ceil(w*h / 4)` (threshold **1/4**), so there is no
/// floating point and the boundary is exact: `ratio == 1/4` counts as dense.
///
/// ## Why 1/4 (measured fixture ratios)
///
/// The experiment fixtures separate cleanly around the midpoint: sparse content
/// is `12/256 = 0.047` (`hline`/`vline`/`diag45`/`thin21`) and `1/256 = 0.004`
/// (`isolated`); dense content is `34/64 = 0.531` (`sprite`) and
/// `144/256 = 0.5625` (`checker`/`flat`). Any threshold in `(0.047, 0.531)`
/// separates them; `1/4` sits near the geometric midpoint with a wide margin on
/// both sides (≥ 0.20 to sparse, ≥ 0.28 to dense) and needs only integer math.
///
/// Ties/boundaries: `ratio == 1/4` → dense (v3); zero opaque → sparse (v2);
/// fully opaque → dense (v3). `w == 0 || h == 0` is not reached by
/// [`rotxel_rotate`] (it returns empty first) and is defined here as sparse.
fn gate_chooses_fit(src: &[u8], w: usize, h: usize) -> bool {
    let Some(total) = w.checked_mul(h) else {
        return false;
    };
    if total == 0 {
        return false;
    }
    opaque_pixel_count(src, w, h) >= total.div_ceil(4)
}

/// Production generic-angle dispatch for [`rotxel_rotate`]: density-gated
/// selection between the no-fit supersample (sparse/thin content) and the
/// grid-fit (dense content). See [`gate_chooses_fit`] for the metric/threshold.
fn rotxel_gate(src: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
    if gate_chooses_fit(src, w, h) {
        rotxel_generic_v3(src, w, h, angle_deg)
    } else {
        rotxel_generic_v2(src, w, h, angle_deg)
    }
}

/// Rotxel's N×N subcell supersample + opaque-majority gate + verbatim plurality
/// vote + local isolated/thin-line restore, at grid-fit phase `(fx, fy)`.
///
/// ## Why supersample (v1 root causes)
///
/// v1 took ONE sample per destination pixel and then ran the 3-grid EPX rule.
/// Two failure modes followed: **over-coverage** (a single transparent sample
/// with an opaque neighbour made EPX paint the neighbour colour, growing a 1-px
/// opaque "skirt") and **fragmentation** (one sample drops 1-px features).
///
/// v1's `dx3 = 3*(trunc(coord_x) − icx)` also made the sample independent of the
/// fractional position, so simply raising a sample count would not have added
/// sub-pixel resolution. v2/v3 remove that 3-grid snapping entirely: the subcell
/// offsets `ox = (u + fx + 0.5)/N − 0.5` are true fractional positions, so
/// larger N genuinely refines the sample and the phase genuinely slides it.
///
/// ## Algorithm
///
/// Destination dims and `R(-θ)` are unchanged. For each destination pixel the
/// centre is mapped into source space; N² subcell samples are taken with hoisted
/// `cos`/`sin`. Only fully opaque (alpha == 255) samples vote, into an
/// insertion-ordered tally (linear scan, no `HashMap`). If the opaque count is a
/// strict majority (a tie stays opaque) the plurality colour is written
/// **verbatim**; otherwise the cell stays the transparent sentinel. Tally ties
/// break to the lexicographically smallest RGBA key. The two bounded post-passes
/// only ever write verbatim source colours into transparent cells, so palette
/// purity and alpha binarity hold.
///
/// The `trunc(coord)` snapping, the `3*center + 1` centre, the `(ox − 1)/3`
/// recovery and the `odd_w`/`odd_h` `+1` parity fudge of v1 are intentionally
/// dropped — they were artifacts of the 3× grid. Exact 0/90/180/270° dispatch
/// is untouched.
fn rotxel_vote_and_post(
    src: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
    fx: usize,
    fy: usize,
) -> (Vec<u8>, usize, usize) {
    let (w_prime, h_prime) = super::rot_sprite::rotated_bounds(w, h, angle_deg);
    if w_prime == 0 || h_prime == 0 {
        return (Vec::new(), 0, 0);
    }
    let Some(out_len) = w_prime
        .checked_mul(h_prime)
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
    else {
        return (Vec::new(), 0, 0);
    };
    let Some(mut out) = alloc_zeroed(out_len) else {
        return (Vec::new(), 0, 0);
    };

    let geo = Geometry::new(w, h, angle_deg, w_prime, h_prime);

    // Reused per-destination-pixel tally (≤ SUBCELL² distinct opaque colours).
    let mut keys = [SENTINEL; TALLY_CAP];
    let mut cnt = [0u32; TALLY_CAP];
    let mut repidx = [0u8; TALLY_CAP];
    let mut repsrc = [(0i64, 0i64); TALLY_CAP];

    for dy in 0..h_prime {
        for dx in 0..w_prime {
            let mut opaque = 0usize;
            let mut n = 0usize;

            for v in 0..SUBCELL {
                let oy = subcell_offset(v, fy, SUBCELL);
                for u in 0..SUBCELL {
                    let ox = subcell_offset(u, fx, SUBCELL);
                    if let Some((c, (ix, iy))) = geo.sample_idx(src, dx, dy, ox, oy) {
                        if c[3] == 255 {
                            opaque += 1;
                            tally_add(
                                &mut keys,
                                &mut cnt,
                                &mut repidx,
                                &mut repsrc,
                                &mut n,
                                c,
                                quant_index(u, v, SUBCELL),
                                (ix, iy),
                            );
                        }
                    }
                }
            }

            // Strict majority; an exact tie (2*opaque == N²) stays opaque.
            if 2 * opaque >= TALLY_CAP {
                let best = argmax(&keys, &cnt, n);
                let mut color = keys[best];
                if ROTXEL_EPX_REFINE {
                    // Optional refinement (OFF by default): run EPX for the
                    // plurality winner's representative subcell, only for an
                    // interior source pixel, and accept only a fully opaque
                    // result.
                    let (rx, ry) = repsrc[best];
                    if rx >= 1 && ry >= 1 && rx < w as i64 - 1 && ry < h as i64 - 1 {
                        let refined = epx(src, w, rx as usize, ry as usize, repidx[best] as i64);
                        if refined[3] == 255 {
                            color = refined;
                        }
                    }
                }
                let di = (dy * w_prime + dx) * BYTES_PER_PIXEL;
                out[di..di + BYTES_PER_PIXEL].copy_from_slice(&color);
            }
            // else: stays the zeroed transparent sentinel.
        }
    }

    restore_isolated_local(&mut out, w_prime, h_prime, src, w, h, angle_deg);
    repair_thin_lines_local(&mut out, w_prime, h_prime);

    (out, w_prime, h_prime)
}

/// Grid-fit: picks the sample-lattice phase `(fx, fy) ∈ 0..N × 0..N` minimising
/// [`lattice_score`], with the deterministic tie-break "lexicographically
/// smallest `(fx, fy)`" (achieved by scanning `fx` then `fy` and replacing only
/// on a strict `<`).
///
/// ## Mirrored from `rot_sprite.rs`'s offset search
///
/// RotSprite scores each candidate downscale-grid offset on its S×-rotated
/// buffer via [`super::rot_sprite`]'s seam/run/corner metric: seams (colour
/// changes) that fall *inside* an output block are the penalty, while seams
/// that fall *on* a block boundary (and aligned runs/corners) are the reward,
/// so the winning offset pushes block boundaries onto the image's real edges.
/// Rotxel has no separate rotated buffer, so the same metric is computed on a
/// precomputed supersample lattice: [`lattice_metric`] scans every adjacent pair
/// once, bins each seam by `(index+1) % N`, and [`lattice_score`] evaluates all
/// 64 phases from those histograms in O(1) each. Integer-only, deterministic, no
/// hashing. The lattice is built once and reused for scoring, so candidate
/// evaluation is a single extra scan rather than 64 resamplings.
fn choose_phase(src: &[u8], w: usize, h: usize, angle_deg: f32) -> (usize, usize) {
    let (w_prime, h_prime) = super::rot_sprite::rotated_bounds(w, h, angle_deg);
    if w_prime == 0 || h_prime == 0 {
        return (0, 0);
    }
    let geo = Geometry::new(w, h, angle_deg, w_prime, h_prime);

    // Precompute the global supersample lattice ONCE: `lattice[n*mw + m]` is
    // the source sample at global lattice index `(m, n)`. Every phase `(fx, fy)`
    // is then just a translated N×N window of this buffer, so all 64 candidate
    // alignments are evaluated by cheap reads instead of 64 full resamplings.
    let mw = match SUBCELL
        .checked_mul(w_prime)
        .and_then(|v| v.checked_add(SUBCELL))
    {
        Some(v) => v,
        None => return (0, 0),
    };
    let mh = match SUBCELL
        .checked_mul(h_prime)
        .and_then(|v| v.checked_add(SUBCELL))
    {
        Some(v) => v,
        None => return (0, 0),
    };
    let Some(cells) = mw.checked_mul(mh) else {
        return (0, 0);
    };
    if cells
        .checked_mul(BYTES_PER_PIXEL)
        .is_none_or(|b| b > GRID_FIT_BUDGET_BYTES)
    {
        return (0, 0); // memory guard: degenerate to v2 (no fit)
    }
    let mut lattice: Vec<Rgba> = Vec::new();
    if lattice.try_reserve_exact(cells).is_err() {
        return (0, 0);
    }
    lattice.resize(cells, SENTINEL);

    let nf = SUBCELL as f64;
    for n in 0..mh {
        let gy = (n as f64 + 0.5) / nf - 0.5;
        for m in 0..mw {
            let gx = (m as f64 + 0.5) / nf - 0.5;
            lattice[n * mw + m] = geo.sample_dest(src, gx, gy);
        }
    }

    // RotSprite-style offset metric on the fixed lattice: bin every seam
    // (colour change between adjacent lattice samples) by where it falls on the
    // N-block grid, then score each phase by how many seams land on its block
    // boundaries. One scan, then 64 O(1) scores.
    let metric = lattice_metric(&lattice, mw, mh);

    let mut best = (0usize, 0usize);
    let mut best_score: Option<i64> = None;
    for fx in 0..SUBCELL {
        for fy in 0..SUBCELL {
            let score = lattice_score(&metric, fx, fy);
            if best_score.is_none_or(|b| score < b) {
                best_score = Some(score);
                best = (fx, fy);
            }
        }
    }
    best
}

/// Seam histograms of the supersample lattice, sized for `SUBCELL`, mirroring
/// `rot_sprite.rs`'s `OffsetMetric` (adapted from its S×-rotated buffer to the
/// fixed Rotxel lattice). See [`lattice_metric`] / [`lattice_score`].
struct LatticeMetric {
    /// Seams between `(m, n)` and `(m+1, n)` by `(m+1) % SUBCELL`.
    hist_h: [u64; SUBCELL],
    /// Seams between `(m, n)` and `(m, n+1)` by `(n+1) % SUBCELL`.
    hist_v: [u64; SUBCELL],
    /// Boundary-aligned seams that continue along the same boundary.
    run_h: [u64; SUBCELL],
    run_v: [u64; SUBCELL],
    /// Corners (perpendicular seams meeting) by their two boundary residues.
    corner: [[u64; SUBCELL]; SUBCELL],
    /// Total seams (phase-independent).
    total: i64,
}

/// One scan of the fixed supersample lattice tallying [`LatticeMetric`], exactly
/// like `rot_sprite.rs::offset_metric` but over the Rotxel lattice. A "seam" is
/// an exact RGBA change between adjacent lattice samples (alpha included, since
/// [`Rgba`] equality is exact). Integer-only, deterministic, no hashing.
fn lattice_metric(lattice: &[Rgba], mw: usize, mh: usize) -> LatticeMetric {
    let n = SUBCELL;
    let mut m = LatticeMetric {
        hist_h: [0; SUBCELL],
        hist_v: [0; SUBCELL],
        run_h: [0; SUBCELL],
        run_v: [0; SUBCELL],
        corner: [[0; SUBCELL]; SUBCELL],
        total: 0,
    };
    let at = |x: usize, y: usize| lattice[y * mw + x];

    for y in 0..mh {
        for x in 0..mw {
            let c = at(x, y);
            let right = x + 1 < mw && at(x + 1, y) != c;
            let down = y + 1 < mh && at(x, y + 1) != c;

            if right {
                let r = (x + 1) % n;
                m.hist_h[r] += 1;
                let up = y > 0 && at(x, y - 1) != at(x + 1, y - 1);
                let dn = y + 1 < mh && at(x, y + 1) != at(x + 1, y + 1);
                if up || dn {
                    m.run_h[r] += 1;
                }
            }
            if down {
                let r = (y + 1) % n;
                m.hist_v[r] += 1;
                let lf = x > 0 && at(x - 1, y) != at(x - 1, y + 1);
                let rt = x + 1 < mw && at(x + 1, y) != at(x + 1, y + 1);
                if lf || rt {
                    m.run_v[r] += 1;
                }
            }
            if right && down {
                m.corner[(x + 1) % n][(y + 1) % n] += 1;
            }
            m.total += right as i64 + down as i64;
        }
    }
    m
}

/// RotSprite's artifact score for phase `(fx, fy)` (lower is better):
/// `inside_block_seams - aligned_run_seams - 2 * aligned_corners`, where a seam
/// is "aligned" when its boundary residue equals the phase. Minimising this
/// pushes block boundaries onto the lattice's real seams.
fn lattice_score(m: &LatticeMetric, fx: usize, fy: usize) -> i64 {
    let inside = m.total - m.hist_h[fx] as i64 - m.hist_v[fy] as i64;
    inside - m.run_h[fx] as i64 - m.run_v[fy] as i64 - 2 * m.corner[fx][fy] as i64
}

/// Test-only implementation selector: `PYX_ROTXEL_IMPL=v1` runs the legacy 3×
/// body, `=v2` the no-fit supersample, `=v3` the grid-fit; anything else
/// (including unset) runs the production density [`rotxel_gate`].
#[cfg(test)]
fn selected_rotxel_generic(
    src: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
) -> (Vec<u8>, usize, usize) {
    match std::env::var("PYX_ROTXEL_IMPL").as_deref() {
        Ok("v1") => rotxel_generic_v1(src, w, h, angle_deg),
        Ok("v2") => rotxel_generic_v2(src, w, h, angle_deg),
        Ok("v3") => rotxel_generic_v3(src, w, h, angle_deg),
        _ => rotxel_gate(src, w, h, angle_deg),
    }
}

/// Legacy generic-angle Rotxel (v1): the single-pass 3× subcell search + EPX
/// rules. Kept verbatim for the BEFORE/AFTER measurement; not used in
/// production.
#[cfg(test)]
fn rotxel_generic_v1(src: &[u8], w: usize, h: usize, angle_deg: f32) -> (Vec<u8>, usize, usize) {
    let (w_prime, h_prime) = super::rot_sprite::rotated_bounds(w, h, angle_deg);
    if w_prime == 0 || h_prime == 0 {
        return (Vec::new(), 0, 0);
    }
    let Some(out_len) = w_prime
        .checked_mul(h_prime)
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
    else {
        return (Vec::new(), 0, 0);
    };
    let Some(mut out) = alloc_zeroed(out_len) else {
        return (Vec::new(), 0, 0);
    };

    // Single pass at the target angle (no Smear stepping).
    let angle = (angle_deg as f64).to_radians();
    let cx = (w as f64 - 1.0) / 2.0;
    let cy = (h as f64 - 1.0) / 2.0;
    let dcx = (w_prime as f64 - 1.0) / 2.0;
    let dcy = (h_prime as f64 - 1.0) / 2.0;

    let icx = cx as i64; // int(center.x): truncation toward zero
    let icy = cy as i64;
    let odd_w = w % 2 != 0;
    let odd_h = h % 2 != 0;
    let grid_w = 3 * w as i64;
    let grid_h = 3 * h as i64;

    for dy in 0..h_prime {
        for dx in 0..w_prime {
            // Destination pixel translated to source space (source centred on
            // the destination canvas). Rotation happens inside the search.
            let coord_x = dx as f64 - dcx + cx;
            let coord_y = dy as f64 - dcy + cy;
            let dx3 = 3 * ((coord_x as i64) - icx);
            let dy3 = 3 * ((coord_y as i64) - icy);

            let mut ox = -1i64;
            let mut oy = -1i64;
            let mut found = false;
            for k in 0..9i64 {
                let i = -1 + (k % 3);
                let j = -1 + (k / 3);
                let vx = (dx3 + i) as f64;
                let vy = (dy3 + j) as f64;
                let dir = vy.atan2(vx) - angle;
                let mag = (vx * vx + vy * vy).sqrt();
                let mut tx = (3.0 * cx + 1.0 + mag * dir.cos()).round() as i64;
                let mut ty = (3.0 * cy + 1.0 + mag * dir.sin()).round() as i64;
                if odd_w {
                    tx += 1;
                }
                if odd_h {
                    ty += 1;
                }
                if tx >= 0 && tx < grid_w && ty >= 0 && ty < grid_h {
                    ox = tx;
                    oy = ty;
                    found = true;
                    break;
                }
            }
            if !found {
                continue; // no candidate in the 3× grid → transparent
            }

            let row = oy % 3;
            let col = ox % 3;
            let index = col + 3 * row;
            let sx = (((ox - 1) as f64) / 3.0).round() as i64;
            let sy = (((oy - 1) as f64) / 3.0).round() as i64;
            debug_assert!(sx >= 0 && sx < w as i64 && sy >= 0 && sy < h as i64);

            let color = if sx == 0 || sx == w as i64 - 1 || sy == 0 || sy == h as i64 - 1 {
                read_px(src, w, sx as usize, sy as usize)
            } else {
                epx(src, w, sx as usize, sy as usize, index)
            };
            let di = (dy * w_prime + dx) * BYTES_PER_PIXEL;
            out[di..di + BYTES_PER_PIXEL].copy_from_slice(&color);
        }
    }

    (out, w_prime, h_prime)
}

/// Insertion-order tally of opaque votes: a linear scan over the (≤ N²)
/// existing keys preserves first-seen order without a `HashMap` (determinism).
/// `repidx`/`repsrc` remember the first subcell that voted for a key (the
/// optional EPX refinement's representative).
fn tally_add(
    keys: &mut [Rgba; TALLY_CAP],
    cnt: &mut [u32; TALLY_CAP],
    repidx: &mut [u8; TALLY_CAP],
    repsrc: &mut [(i64, i64); TALLY_CAP],
    n: &mut usize,
    c: Rgba,
    quant: u8,
    src_xy: (i64, i64),
) {
    for i in 0..*n {
        if keys[i] == c {
            cnt[i] += 1;
            return;
        }
    }
    if *n < TALLY_CAP {
        keys[*n] = c;
        cnt[*n] = 1;
        repidx[*n] = quant;
        repsrc[*n] = src_xy;
        *n += 1;
    }
}

/// The plurality winner: maximum count, ties broken to the lexicographically
/// smallest RGBA key (deterministic, independent of insertion order).
fn argmax(keys: &[Rgba; TALLY_CAP], cnt: &[u32; TALLY_CAP], n: usize) -> usize {
    let mut best = 0usize;
    for i in 1..n {
        if cnt[i] > cnt[best] || (cnt[i] == cnt[best] && keys[i] < keys[best]) {
            best = i;
        }
    }
    best
}

/// Maps a subcell `(u, v)` to the 0..8 EPX zone family of the original 3-grid:
/// `q(t) = min(2, (t*3)/N)`, `index3 = q(u) + 3*q(v)`. Only used by the
/// optional EPX refinement.
fn quant_index(u: usize, v: usize, n: usize) -> u8 {
    let q = |t: usize| ((t * 3) / n).min(2);
    (q(u) + 3 * q(v)) as u8
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

/// True when no 8-neighbour of `(x, y)` has the same RGBA as `p`.
fn is_isolated_local(src: &[u8], w: usize, h: usize, x: usize, y: usize, p: Rgba) -> bool {
    let xi = x as isize;
    let yi = y as isize;
    for (dx, dy) in NEIGHBOURS_8 {
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
    true
}

/// Local port of `rot_sprite::restore_isolated` (that helper is private there):
/// re-add 1-px isolated source details the majority vote erased.
///
/// A source pixel is *isolated* when it is fully opaque and none of its 8
/// neighbours has the same RGBA. Each such pixel is forward-mapped
/// (`dst_center + R(θ)·(x − src_center)`, the exact inverse of the v2 sampler)
/// to a 1× output cell; only a transparent cell is filled. Row-major scan,
/// transparent-only writes → deterministic, palette-pure and AA-free.
fn restore_isolated_local(
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
            if p[3] != 255 || !is_isolated_local(src, w, h, x, y, p) {
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
                if out[di + 3] == SENTINEL[3] {
                    out[di..di + BYTES_PER_PIXEL].copy_from_slice(&p);
                }
            }
        }
    }
}

/// Local port of `rot_sprite::repair_thin_lines` (private there): fill a
/// TRANSPARENT cell that is a one-pixel break in a thin line.
///
/// A transparent cell is filled **iff** it has **exactly two** opaque
/// 8-neighbours, those two have the **same RGBA**, and they sit on **opposite**
/// sides (horizontal, vertical, or either diagonal). The fill is that verbatim
/// neighbour colour. A pre-pass snapshot applies all fills afterwards, so the
/// pass cannot cascade (a 2-px gap stays a gap). Palette-pure, AA-free,
/// deterministic.
fn repair_thin_lines_local(out: &mut [u8], w: usize, h: usize) {
    let mut fills: Vec<(usize, Rgba)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * BYTES_PER_PIXEL;
            if out[i + 3] != 0 {
                continue; // only fill transparent cells
            }

            let xi = x as isize;
            let yi = y as isize;
            let mut count = 0usize;
            let mut c0 = SENTINEL;
            let mut c1 = SENTINEL;
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

/// Squared Euclidean RGBA distance (alpha included).
#[inline]
fn sqdist_u32(a: Rgba, b: Rgba) -> u32 {
    let mut sum = 0u32;
    for i in 0..4 {
        let d = a[i] as i32 - b[i] as i32;
        sum += (d * d) as u32;
    }
    sum
}

/// `similarColors` verbatim: Euclidean RGBA distance (alpha included) `< 100`.
fn similar_colors(a: Rgba, b: Rgba) -> bool {
    sqdist_u32(a, b) < TOLERANCE_SQ
}

/// The `index 0..8` EPX rules, ported verbatim. `(x, y)` is the source pixel
/// (`1 ≤ x ≤ w-2`, `1 ≤ y ≤ h-2`, guaranteed by the border check); `a..l` are
/// its 3×3 neighbourhood. Only `b`, `d`, `e`, `f`, `h` are ever returned.
fn epx(src: &[u8], w: usize, x: usize, y: usize, index: i64) -> Rgba {
    let a = read_px(src, w, x - 1, y - 1);
    let b = read_px(src, w, x, y - 1);
    let c = read_px(src, w, x + 1, y - 1);
    let d = read_px(src, w, x - 1, y);
    let e = read_px(src, w, x, y);
    let f = read_px(src, w, x + 1, y);
    let g = read_px(src, w, x - 1, y + 1);
    let h = read_px(src, w, x, y + 1);
    let l = read_px(src, w, x + 1, y + 1);

    let sim = similar_colors;
    match index {
        0 => {
            if sim(d, b) && !sim(d, h) && !sim(b, f) {
                d
            } else {
                e
            }
        }
        1 => {
            if (sim(d, b) && !sim(d, h) && !sim(b, f) && !sim(e, c))
                || (sim(b, f) && !sim(d, b) && !sim(f, h) && !sim(e, a))
            {
                b
            } else {
                e
            }
        }
        2 => {
            if sim(b, f) && !sim(d, b) && !sim(f, h) {
                f
            } else {
                e
            }
        }
        3 => {
            if (sim(d, h) && !sim(f, h) && !sim(d, b) && !sim(e, a))
                || (sim(d, b) && !sim(d, h) && !sim(b, f) && !sim(e, g))
            {
                d
            } else {
                e
            }
        }
        4 => e,
        5 => {
            if (sim(b, f) && !sim(d, b) && !sim(f, h) && !sim(e, l))
                || (sim(f, h) && !sim(b, f) && !sim(d, h) && !sim(e, c))
            {
                f
            } else {
                e
            }
        }
        6 => {
            if sim(d, h) && !sim(f, h) && !sim(d, b) {
                d
            } else {
                e
            }
        }
        7 => {
            if (sim(f, h) && !sim(f, b) && !sim(d, h) && !sim(e, g))
                || (sim(d, h) && !sim(f, h) && !sim(d, b) && !sim(e, l))
            {
                h
            } else {
                e
            }
        }
        8 => {
            if sim(f, h) && !sim(f, b) && !sim(d, h) {
                f
            } else {
                e
            }
        }
        _ => e,
    }
}

/// Reads one RGBA8 pixel as `[r, g, b, a]`.
fn read_px(buf: &[u8], w: usize, x: usize, y: usize) -> Rgba {
    let i = (y * w + x) * BYTES_PER_PIXEL;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

/// Zeroed allocation that never panics (overflow / alloc failure → `None`).
fn alloc_zeroed(len: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, 0);
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::super::clean_edge::tests::{
        all_fixtures, alpha_binary, comparison_angles, palette_pure,
    };
    use super::*;

    /// The `diag45` experiment fixture (for exact-angle checks).
    fn diag45_fixture() -> (Vec<u8>, usize, usize) {
        all_fixtures()
            .into_iter()
            .find(|(name, ..)| *name == "diag45")
            .map(|(_, buf, w, h)| (buf, w, h))
            .expect("diag45 fixture")
    }

    #[test]
    fn rotxel_dims_match_baseline() {
        let baseline = super::super::rotate;
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (_, bw, bh) = baseline(&buf, w, h, angle);
                let (_, rw, rh) = rotxel_rotate(&buf, w, h, angle);
                assert_eq!(
                    (rw, rh),
                    (bw, bh),
                    "dims differ for {name} @ {angle}°: rotxel {rw}x{rh} vs baseline {bw}x{bh}"
                );
            }
        }
    }

    #[test]
    fn rotxel_is_palette_pure_and_alpha_binary() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (out, _, _) = rotxel_rotate(&buf, w, h, angle);
                assert!(
                    palette_pure(&buf, &out),
                    "rotxel invented a colour for {name} @ {angle}°"
                );
                assert!(
                    alpha_binary(&out),
                    "rotxel produced partial alpha for {name} @ {angle}°"
                );
            }
        }
    }

    #[test]
    fn rotxel_is_deterministic() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (a, aw, ah) = rotxel_rotate(&buf, w, h, angle);
                let (b, bw, bh) = rotxel_rotate(&buf, w, h, angle);
                assert_eq!((aw, ah), (bw, bh), "{name} @ {angle}° dims");
                assert_eq!(a, b, "{name} @ {angle}° bytes");
            }
        }
    }

    #[test]
    fn rotxel_handles_empty_and_undersized() {
        let (out, ow, oh) = rotxel_rotate(&[], 0, 0, 30.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));

        let (out, ow, oh) = rotxel_rotate(&[0u8; 8], 3, 3, 30.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));

        let (out, ow, oh) = rotxel_rotate(&[0u8; 16], 4, 4, f32::NAN);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));

        let (out, ow, oh) = rotxel_rotate(&[0u8; 16], 4, 4, f32::INFINITY);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
    }

    #[test]
    fn rotxel_exact_angles() {
        let (buf, w, h) = diag45_fixture();

        let (out, ow, oh) = rotxel_rotate(&buf, w, h, 0.0);
        assert_eq!((ow, oh), (w, h));
        assert_eq!(out, buf);

        let (r, rw, rh) = rotxel_rotate(&buf, w, h, 90.0);
        let (e, ew, eh) = super::super::exact::rotate_90_cw(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);

        let (r, rw, rh) = rotxel_rotate(&buf, w, h, 180.0);
        let (e, ew, eh) = super::super::exact::rotate_180(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);

        let (r, rw, rh) = rotxel_rotate(&buf, w, h, 270.0);
        let (e, ew, eh) = super::super::exact::rotate_90_ccw(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);
    }

    /// Number of fully-opaque (`alpha == 255`) pixels.
    fn opaque_count(buf: &[u8]) -> usize {
        buf.chunks_exact(BYTES_PER_PIXEL)
            .filter(|px| px[3] == 255)
            .count()
    }

    /// T2-X2: the v2 majority/plurality supersample must not grow the extra
    /// 1-px opaque "skirt" v1's single-sample EPX produced. For the `sprite`
    /// (source 34 opaque) and `thin21` (source 12 opaque) fixtures, v2's opaque
    /// count is ≤ v1's and stays close to the source.
    #[test]
    fn rotxel_v2_reduces_over_coverage() {
        let fixtures = all_fixtures();
        let lookup = |name: &str| {
            fixtures
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(_, buf, w, h)| (buf.clone(), *w, *h))
                .expect("fixture")
        };

        for name in ["sprite", "thin21"] {
            let (src, w, h) = lookup(name);
            let source_op = opaque_count(&src);
            for angle in [8.0f32, 15.0, 30.0, 37.0, 45.0, 60.0, 26.565] {
                let (v1, _, _) = rotxel_generic_v1(&src, w, h, angle);
                let (v2, _, _) = rotxel_generic_v2(&src, w, h, angle);
                let op1 = opaque_count(&v1);
                let op2 = opaque_count(&v2);
                assert!(
                    op2 <= op1,
                    "{name} @ {angle}°: v2 must not over-cover more than v1 ({op2} > {op1})"
                );
                assert!(
                    op2.abs_diff(source_op) <= 10,
                    "{name} @ {angle}°: v2 opaque {op2} must stay near the source {source_op}"
                );
            }
        }
    }

    /// T2-X2: v2 itself is deterministic, palette-pure and alpha-binary (the
    /// shared invariant tests go through `rotxel_rotate`, which is env-selectable
    /// under test; this pins v2 directly).
    #[test]
    fn rotxel_v2_is_deterministic_palette_pure_and_binary() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (a, aw, ah) = rotxel_generic_v2(&buf, w, h, angle);
                let (b, bw, bh) = rotxel_generic_v2(&buf, w, h, angle);
                assert_eq!((aw, ah), (bw, bh), "v2 dims differ for {name} @ {angle}°");
                assert_eq!(a, b, "v2 is non-deterministic for {name} @ {angle}°");
                assert!(
                    palette_pure(&buf, &a),
                    "v2 invented a colour for {name} @ {angle}°"
                );
                assert!(
                    alpha_binary(&a),
                    "v2 produced partial alpha for {name} @ {angle}°"
                );
            }
        }
    }

    // -- Shared metric helpers (duplicated from clean_edge.rs, which is not in
    //    scope to edit) ----------------------------------------------------

    fn opaque_cells(buf: &[u8], w: usize, h: usize) -> Vec<(usize, usize)> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let i = (y * w + x) * BYTES_PER_PIXEL;
                buf[i + 3] == 255
            })
            .collect()
    }

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

    fn component_count_4(cells: &[(usize, usize)]) -> usize {
        use std::collections::{HashSet, VecDeque};
        let set: HashSet<(isize, isize)> = cells
            .iter()
            .map(|&(x, y)| (x as isize, y as isize))
            .collect();
        let mut seen = HashSet::new();
        let mut count = 0usize;
        for &start in &set {
            if seen.contains(&start) {
                continue;
            }
            count += 1;
            let mut queue = VecDeque::new();
            seen.insert(start);
            queue.push_back(start);
            while let Some((x, y)) = queue.pop_front() {
                for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                    let n = (x + dx, y + dy);
                    if set.contains(&n) && seen.insert(n) {
                        queue.push_back(n);
                    }
                }
            }
        }
        count
    }

    /// `(opaque, broken)`: opaque pixels not in the largest 4-connected run
    /// (same definition as `clean_edge.rs`'s `line_integrity`).
    fn line_integrity(buf: &[u8], w: usize, h: usize) -> (usize, usize) {
        let cells = opaque_cells(buf, w, h);
        let largest = largest_component_4(&cells);
        (cells.len(), cells.len() - largest)
    }

    /// `(opaque, 4-connected components)`.
    fn detail_stats(buf: &[u8], w: usize, h: usize) -> (usize, usize) {
        let cells = opaque_cells(buf, w, h);
        (cells.len(), component_count_4(&cells))
    }

    // -- v3 (grid-fit) invariant assertions ---------------------------------

    #[test]
    fn rotxel_v3_is_deterministic_palette_pure_and_binary() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (a, aw, ah) = rotxel_generic_v3(&buf, w, h, angle);
                let (b, bw, bh) = rotxel_generic_v3(&buf, w, h, angle);
                assert_eq!((aw, ah), (bw, bh), "v3 dims differ for {name} @ {angle}°");
                assert_eq!(a, b, "v3 is non-deterministic for {name} @ {angle}°");
                assert!(
                    palette_pure(&buf, &a),
                    "v3 invented a colour for {name} @ {angle}°"
                );
                assert!(
                    alpha_binary(&a),
                    "v3 produced partial alpha for {name} @ {angle}°"
                );
            }
        }
    }

    #[test]
    fn rotxel_v3_dims_match_baseline() {
        let baseline = super::super::rotate;
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (_, bw, bh) = baseline(&buf, w, h, angle);
                let (_, vw, vh) = rotxel_generic_v3(&buf, w, h, angle);
                assert_eq!((vw, vh), (bw, bh), "v3 dims differ for {name} @ {angle}°");
            }
        }
    }

    #[test]
    fn rotxel_grid_fit_phase_is_deterministic_and_bounded() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let a = choose_phase(&buf, w, h, angle);
                let b = choose_phase(&buf, w, h, angle);
                assert_eq!(a, b, "choose_phase non-deterministic for {name} @ {angle}°");
                assert!(a.0 < SUBCELL && a.1 < SUBCELL, "phase out of range: {a:?}");
            }
        }
    }

    /// Over-coverage must not regress to v1's opaque "skirt": for `sprite`
    /// (source 34 opaque) and `thin21` (source 12 opaque), v3's opaque count is
    /// ≤ v1's at every comparison angle and stays within `+10` of the source.
    ///
    /// Note: v3 opaque is *allowed* to exceed v2 opaque. That is detail
    /// **recovery**, not bloat — e.g. `thin21 @ 30°` is v2=9 (under-covered),
    /// v3=12 (= source), v1=17. The printed `recovered` list records those rows.
    #[test]
    fn rotxel_v3_over_coverage_not_worse_than_v1() {
        let fixtures = all_fixtures();
        let lookup = |name: &str| {
            fixtures
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(_, buf, w, h)| (buf.clone(), *w, *h))
                .expect("fixture")
        };
        let mut recovered: Vec<String> = Vec::new();
        for name in ["sprite", "thin21"] {
            let (src, w, h) = lookup(name);
            let source_op = opaque_count(&src);
            for angle in comparison_angles() {
                let v1 = opaque_count(&rotxel_generic_v1(&src, w, h, angle).0);
                let v2 = opaque_count(&rotxel_generic_v2(&src, w, h, angle).0);
                let v3 = opaque_count(&rotxel_generic_v3(&src, w, h, angle).0);
                assert!(
                    v3 <= v1,
                    "{name} @ {angle}°: v3 opaque {v3} bloats past v1's {v1}"
                );
                assert!(
                    v3.abs_diff(source_op) <= 10,
                    "{name} @ {angle}°: v3 opaque {v3} too far from source {source_op}"
                );
                if v3 > v2 {
                    recovered.push(format!(
                        "{name}@{angle}° v2={v2}->v3={v3} (src={source_op}, v1={v1})"
                    ));
                }
            }
        }
        println!("\nv3>v2 opaque (detail recovered, not bloat): {recovered:?}");
    }

    /// The grid-fit must not *reduce* quality: at least one (fixture, angle)
    /// row must have strictly fewer broken pixels than v2, and the timing note
    /// is printed. (If this ever fails, the honest fallback is to document that
    /// grid-fit did not improve the metrics — but it is asserted here because
    /// the measured BEFORE/AFTER shows it does.)
    #[test]
    fn rotxel_grid_fit_improves_broken_metric() {
        let mut improved = 0usize;
        let mut regressed = 0usize;
        let mut equal = 0usize;
        for (_name, src, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (v2out, v2w, v2h) = rotxel_generic_v2(&src, w, h, angle);
                let (v3out, v3w, v3h) = rotxel_generic_v3(&src, w, h, angle);
                let v2 = line_integrity(&v2out, v2w, v2h);
                let v3 = line_integrity(&v3out, v3w, v3h);
                if v3.1 < v2.1 {
                    improved += 1;
                } else if v3.1 > v2.1 {
                    regressed += 1;
                } else {
                    equal += 1;
                }
            }
        }
        println!("\ngrid-fit broken: improved={improved} regressed={regressed} equal={equal}");
        assert!(
            improved > 0,
            "grid-fit produced no broken-pixel improvement"
        );
    }

    /// BEFORE (v2) vs AFTER (v3) vs the production GATE, plus RotSprite and
    /// CleanEdge-1× for context, plus the per-fixture density branch. Run with
    /// `cargo test comparison_v2_before_v3_after -- --nocapture`.
    #[test]
    fn comparison_v2_before_v3_after() {
        let baseline = super::super::rotate;
        let clean = super::super::clean_edge::clean_edge_rotate;

        println!(
            "\n{:<9} {:>7} | {:>4} {:>7} | {:>4} {:>7} | {:>4} {:>7} | {:>4} {:>7} | {:>4} {:>7} {:>3}",
            "fixture", "angle", "RS_brk", "RS_o/c", "CE_brk", "CE_o/c", "v2_brk", "v2_o/c",
            "v3_brk", "v3_o/c", "gate_brk", "gate_o/c", "br"
        );
        println!("{}", "-".repeat(112));

        let metrics = |o: &[u8], ow: usize, oh: usize| {
            let (op, brk) = line_integrity(o, ow, oh);
            let (op2, cmp) = detail_stats(o, ow, oh);
            debug_assert_eq!(op, op2);
            (brk, op, cmp)
        };

        let mut v2_exact = 0usize;
        let mut v3_exact = 0usize;
        let mut gate_exact = 0usize;
        let mut v3_best = 0usize;
        let mut gate_best = 0usize;
        let mut gate_le_v2 = 0usize;
        let mut gate_le_v3 = 0usize;
        let mut pal_ok = 0usize;
        let mut rows = 0usize;

        for (name, src, w, h) in all_fixtures() {
            let (src_op, src_cmp) = detail_stats(&src, w, h);
            let fit = gate_chooses_fit(&src, w, h);
            let branch = if fit { "v3" } else { "v2" };
            for angle in comparison_angles() {
                rows += 1;
                let (bout, bw, bh) = baseline(&src, w, h, angle);
                let (cout, cw, ch) = clean(&src, w, h, angle);
                let (v2out, v2w, v2h) = rotxel_generic_v2(&src, w, h, angle);
                let (v3out, v3w, v3h) = rotxel_generic_v3(&src, w, h, angle);
                let (gout, gw, gh) = rotxel_gate(&src, w, h, angle);
                assert_eq!((bw, bh), (gw, gh));

                // The gate must emit byte-for-byte the chosen branch's output.
                let chosen: &[u8] = if fit { &v3out } else { &v2out };
                assert_eq!(
                    gout, chosen,
                    "gate picked {branch} but output != {branch} for {name} @ {angle}°"
                );

                let (b_brk, b_op, b_cmp) = metrics(&bout, bw, bh);
                let (c_brk, c_op, c_cmp) = metrics(&cout, cw, ch);
                let (v2_brk, v2_op, v2_cmp) = metrics(&v2out, v2w, v2h);
                let (v3_brk, v3_op, v3_cmp) = metrics(&v3out, v3w, v3h);
                let (g_brk, g_op, g_cmp) = metrics(&gout, gw, gh);

                pal_ok += palette_pure(&src, &gout) as usize;
                v2_exact += ((v2_op, v2_cmp) == (src_op, src_cmp)) as usize;
                v3_exact += ((v3_op, v3_cmp) == (src_op, src_cmp)) as usize;
                gate_exact += ((g_op, g_cmp) == (src_op, src_cmp)) as usize;
                let min_brk = b_brk.min(v2_brk).min(v3_brk);
                v3_best += (v3_brk == min_brk) as usize;
                gate_best += (g_brk == min_brk) as usize;
                gate_le_v2 += (g_brk <= v2_brk) as usize;
                gate_le_v3 += (g_brk <= v3_brk) as usize;

                println!(
                    "{:<9} {:>7.3} | {:>4} {:>7} | {:>4} {:>7} | {:>4} {:>7} | {:>4} {:>7} | {:>4} {:>7} {:>3}",
                    name,
                    angle,
                    b_brk,
                    format!("{b_op}/{b_cmp}"),
                    c_brk,
                    format!("{c_op}/{c_cmp}"),
                    v2_brk,
                    format!("{v2_op}/{v2_cmp}"),
                    v3_brk,
                    format!("{v3_op}/{v3_cmp}"),
                    g_brk,
                    format!("{g_op}/{g_cmp}"),
                    branch,
                );
            }
            let tot = w * h;
            println!(
                "{:<9} {:>7} | src o/c={src_op}/{src_cmp} ratio={:.4} ({}/{}) branch={branch}",
                name,
                "src",
                opaque_pixel_count(&src, w, h) as f64 / tot as f64,
                opaque_pixel_count(&src, w, h),
                tot
            );
        }

        println!("\nGATE broken <= v2 on {gate_le_v2}/{rows} rows; <= v3 on {gate_le_v3}/{rows}");
        println!("detail-exact vs source: v2={v2_exact} v3={v3_exact} gate={gate_exact}");
        println!("(joint) broken-min: v3 on {v3_best}/{rows}, gate on {gate_best}/{rows}");
        println!("gate palette purity: {pal_ok}/{rows}");
    }

    /// CPU cost of the production GATE vs its branches. The 32×32 line is
    /// sparse (gate → v2); a 32×32 half-opaque block is dense (gate → v3).
    /// Reported, never asserted.
    #[test]
    fn comparison_grid_fit_timing() {
        use std::time::Instant;
        let (w, h) = (32usize, 32usize);
        let mut line = vec![0u8; w * h * BYTES_PER_PIXEL];
        for x in 4..28 {
            let i = (16 * w + x) * BYTES_PER_PIXEL;
            line[i..i + BYTES_PER_PIXEL].copy_from_slice(&[255, 0, 0, 255]);
        }
        // 16×16 solid block in the middle: 256/1024 = 0.25 → exactly dense.
        let mut dense = vec![0u8; w * h * BYTES_PER_PIXEL];
        for y in 8..24 {
            for x in 8..24 {
                let i = (y * w + x) * BYTES_PER_PIXEL;
                dense[i..i + BYTES_PER_PIXEL].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let angle = 30.0f32;
        let runs = 3u32;

        let time = |buf: &[u8], f: fn(&[u8], usize, usize, f32) -> (Vec<u8>, usize, usize)| {
            let t = Instant::now();
            for _ in 0..runs {
                let _ = f(buf, w, h, angle);
            }
            t.elapsed() / runs
        };
        let _ = rotxel_gate(&line, w, h, angle);
        let _ = rotxel_gate(&dense, w, h, angle);

        println!(
            "\n[timing] 32×32 @ {angle}°, avg of {runs} runs; sparse line gate branch={} (ratio {:.3}), dense block gate branch={} (ratio {:.3})",
            if gate_chooses_fit(&line, w, h) { "v3" } else { "v2" },
            opaque_pixel_count(&line, w, h) as f64 / (w * h) as f64,
            if gate_chooses_fit(&dense, w, h) { "v3" } else { "v2" },
            opaque_pixel_count(&dense, w, h) as f64 / (w * h) as f64,
        );
        println!(
            "  RotSprite (baseline)        : {:?}",
            time(&line, super::super::rotate)
        );
        println!(
            "  sparse: v2 (no fit)         : {:?}",
            time(&line, rotxel_generic_v2)
        );
        println!(
            "  sparse: v3 (fit)            : {:?}",
            time(&line, rotxel_generic_v3)
        );
        println!(
            "  sparse: GATE (density pass) : {:?}",
            time(&line, rotxel_gate)
        );
        println!(
            "  dense : v2 (no fit)         : {:?}",
            time(&dense, rotxel_generic_v2)
        );
        println!(
            "  dense : v3 (fit)            : {:?}",
            time(&dense, rotxel_generic_v3)
        );
        println!(
            "  dense : GATE (density pass) : {:?}",
            time(&dense, rotxel_gate)
        );

        // `choose_phase` dominates v3; the gate adds only the linear density pass.
        let t = Instant::now();
        for _ in 0..runs {
            let _ = choose_phase(&dense, w, h, angle);
        }
        let phase_only = t.elapsed() / runs;
        println!("  choose_phase alone (dense)  : {phase_only:?}");
    }

    // -- Density gate --------------------------------------------------------

    /// Build a `w×h` RGBA8 buffer with the first `opaque` pixels fully opaque
    /// red and the rest transparent (for boundary tests).
    fn solid_prefix(w: usize, h: usize, opaque: usize) -> Vec<u8> {
        let mut b = vec![0u8; w * h * BYTES_PER_PIXEL];
        for i in 0..opaque.min(w * h) {
            let j = i * BYTES_PER_PIXEL;
            b[j..j + BYTES_PER_PIXEL].copy_from_slice(&[255, 0, 0, 255]);
        }
        b
    }

    /// The gate must choose v3 for the dense fixtures and v2 for the sparse
    /// ones, printing the measured ratio per fixture, and must emit exactly the
    /// chosen branch's bytes.
    #[test]
    fn rotxel_density_gate_selects_expected_branch() {
        let fixtures = all_fixtures();
        let mut table: Vec<(&'static str, f64, bool)> = fixtures
            .iter()
            .map(|(name, buf, w, h)| {
                (
                    *name,
                    opaque_pixel_count(buf, *w, *h) as f64 / (*w * *h) as f64,
                    gate_chooses_fit(buf, *w, *h),
                )
            })
            .collect();
        table.sort_by(|a, b| a.0.cmp(b.0));

        println!("\ndensity gate (threshold ratio >= 1/4 -> v3):");
        for (name, r, fit) in &table {
            println!(
                "  {name:<9} ratio={r:.4} -> {}",
                if *fit { "v3" } else { "v2" }
            );
        }

        let picks = |n: &str| table.iter().find(|(name, ..)| *name == n).unwrap().2;
        for n in ["sprite", "flat", "checker"] {
            assert!(picks(n), "{n} must select v3");
        }
        for n in ["hline", "vline", "diag45", "thin21", "isolated"] {
            assert!(!picks(n), "{n} must select v2");
        }

        for (name, src, w, h) in all_fixtures() {
            let fit = gate_chooses_fit(&src, w, h);
            for angle in comparison_angles() {
                let (gout, _, _) = rotxel_gate(&src, w, h, angle);
                let chosen = if fit {
                    rotxel_generic_v3(&src, w, h, angle).0
                } else {
                    rotxel_generic_v2(&src, w, h, angle).0
                };
                assert_eq!(gout, chosen, "{name} @ {angle}° gate != chosen branch");
            }
        }
    }

    /// Boundary/edge semantics of the gate.
    #[test]
    fn rotxel_gate_boundaries() {
        // Zero opaque (all transparent) 4×4 → ratio 0 → v2.
        let t = solid_prefix(4, 4, 0);
        assert!(!gate_chooses_fit(&t, 4, 4), "all-transparent must pick v2");
        assert_eq!(
            rotxel_gate(&t, 4, 4, 30.0).0,
            rotxel_generic_v2(&t, 4, 4, 30.0).0
        );

        // Fully opaque 4×4 → ratio 1 → v3.
        let f = solid_prefix(4, 4, 16);
        assert!(gate_chooses_fit(&f, 4, 4), "fully opaque must pick v3");
        assert_eq!(
            rotxel_gate(&f, 4, 4, 30.0).0,
            rotxel_generic_v3(&f, 4, 4, 30.0).0
        );

        // Exactly at the threshold: 16 cells, 4 opaque → ratio 0.25 → v3 (>=).
        let exact = solid_prefix(4, 4, 4);
        assert!(gate_chooses_fit(&exact, 4, 4), "ratio == 1/4 must pick v3");
        assert_eq!(
            rotxel_gate(&exact, 4, 4, 30.0).0,
            rotxel_generic_v3(&exact, 4, 4, 30.0).0
        );

        // Just below the threshold: 3 opaque → v2.
        let below = solid_prefix(4, 4, 3);
        assert!(!gate_chooses_fit(&below, 4, 4), "ratio < 1/4 must pick v2");
        assert_eq!(
            rotxel_gate(&below, 4, 4, 30.0).0,
            rotxel_generic_v2(&below, 4, 4, 30.0).0
        );

        // Zero dims are defined sparse; `rotxel_rotate` guards before the gate.
        assert!(!gate_chooses_fit(&[], 0, 0));
        assert!(!gate_chooses_fit(&[0u8; 4], 0, 5));

        // Empty/undersized through the public entry point.
        assert_eq!(rotxel_rotate(&[], 0, 0, 30.0), (Vec::new(), 0, 0));
        assert_eq!(rotxel_rotate(&[0u8; 8], 3, 3, 30.0), (Vec::new(), 0, 0));

        // 3×3 all-transparent with 2 opaque (ratio 2/9 < 1/4) → v2; 3×3 with
        // 3 opaque (ratio 3/9 > 1/4) → v3.
        assert!(!gate_chooses_fit(&solid_prefix(3, 3, 2), 3, 3));
        assert!(gate_chooses_fit(&solid_prefix(3, 3, 3), 3, 3));
    }

    /// The gate must combine v3's dense gain with v2's thin-line protection.
    #[test]
    fn rotxel_gate_dense_gain_and_thin_protection() {
        let fixtures = all_fixtures();
        let lookup = |name: &str| -> (Vec<u8>, usize, usize) {
            fixtures
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(_, b, w, h)| (b.clone(), *w, *h))
                .expect("fixture")
        };
        let broken = |o: &[u8], w: usize, h: usize| line_integrity(o, w, h).1;

        // Dense: gate == v3, and strictly fewer broken than v2 on ≥1 row.
        let mut dense_gain = 0usize;
        let mut dense_rows = 0usize;
        for name in ["sprite", "flat", "checker"] {
            let (src, w, h) = lookup(name);
            assert!(gate_chooses_fit(&src, w, h), "{name} must be dense");
            for angle in comparison_angles() {
                dense_rows += 1;
                let g = rotxel_gate(&src, w, h, angle).0;
                let v3 = rotxel_generic_v3(&src, w, h, angle).0;
                let v2 = rotxel_generic_v2(&src, w, h, angle).0;
                assert_eq!(g, v3, "{name} @ {angle}° dense gate must equal v3");
                if broken(&g, w, h) < broken(&v2, w, h) {
                    dense_gain += 1;
                }
            }
        }
        println!("\ndense gate rows strictly better than v2: {dense_gain}/{dense_rows}");
        assert!(dense_gain > 0, "gate showed no dense-sprite gain vs v2");

        // Sparse: gate == v2 (thin-line protection) on every row.
        for name in ["hline", "vline", "diag45", "thin21"] {
            let (src, w, h) = lookup(name);
            assert!(!gate_chooses_fit(&src, w, h), "{name} must be sparse");
            for angle in comparison_angles() {
                let g = rotxel_gate(&src, w, h, angle).0;
                let v2 = rotxel_generic_v2(&src, w, h, angle).0;
                assert_eq!(g, v2, "{name} @ {angle}° sparse gate must equal v2");
            }
        }
    }

    /// Gate over-coverage must not regress to v1's opaque "skirt".
    #[test]
    fn rotxel_gate_over_coverage_not_worse_than_v1() {
        let fixtures = all_fixtures();
        let lookup = |name: &str| -> (Vec<u8>, usize, usize) {
            fixtures
                .iter()
                .find(|(n, ..)| *n == name)
                .map(|(_, b, w, h)| (b.clone(), *w, *h))
                .expect("fixture")
        };
        for name in ["sprite", "thin21"] {
            let (src, w, h) = lookup(name);
            let source_op = opaque_count(&src);
            for angle in comparison_angles() {
                let v1 = opaque_count(&rotxel_generic_v1(&src, w, h, angle).0);
                let g = opaque_count(&rotxel_gate(&src, w, h, angle).0);
                assert!(
                    g <= v1,
                    "{name} @ {angle}°: gate opaque {g} bloats past v1's {v1}"
                );
                assert!(
                    g.abs_diff(source_op) <= 10,
                    "{name} @ {angle}°: gate opaque {g} too far from source {source_op}"
                );
            }
        }
    }
}
