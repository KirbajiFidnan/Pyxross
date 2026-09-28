//! CleanEdge rotation — a faithful CPU port of torcado's `cleanEdge`
//! pixel-art upscaling/rotation shader (MIT, 2022).
//!
//! Selectable through the transform pipeline via
//! [`super::TransformAlgorithm::CleanEdge`] / [`super::rotate_with`];
//! [`clean_edge_rotate`] is the core entry point. The measurement tests at the
//! bottom of this file remain as the data-driven comparison harness.
//!
//! # Provenance
//!
//! The algorithm is ported from the Shadertoy reference saved at
//! `/tmp/opencode/cleanEdge_shadertoy.glsl`. `similar`/`similar3`/`similar4`,
//! `higher`, `cd`, `distToLine` and `sliceDist` mirror the GLSL statements
//! one-for-one (including the `SLOPE` 2:1-slant branches, the two "far corner"
//! branches and the `#ifdef CLEANUP` slant-transition cleanups, which are
//! enabled here — see [`ENABLE_CLEANUP`]).
//!
//! # Shader → CPU adaptation
//!
//! The shader is a per-fragment routine: for a fragment at some sub-pixel
//! location it derives `local = fract(px)`, the containing cell centre
//! `px = ceil(px) - 0.5` and a quadrant sign `pointDir = round(local)*2 - 1`,
//! then reconstructs an edge from the 21 nearest neighbours and decides which
//! neighbour colour (if any) that quadrant should snap to.
//!
//! For a CPU rotation we need a *destination* pixel map. Each destination pixel
//! is covered by a `4×4` grid of fragment positions ([`SUPERSAMPLE`]); every
//! fragment is inverse-rotated into source space to a continuous coordinate
//! `(sx, sy)`, which plays the role of the shader's `px`, so `local =
//! fract(sx, sy)` carries that fragment's sub-pixel phase and the 21 neighbours
//! are sampled from the **source** buffer at integer offsets `px +
//! offset*pointDir` (out-of-bounds → transparent). The 16 fragment colours are
//! merged into one output pixel by a palette-pure plurality vote. The inverse
//! map and the rotated-bounding-box dims match [`super::rot_sprite::rotate`]'s
//! model exactly, so the output geometry is identical.
//!
//! # Fidelity caveats (read before trusting a comparison)
//!
//! - The shader samples colours in normalised `[0,1]` space, so `cd` and the
//!   `distAgainst < distTowards + 0.001` epsilon are evaluated over
//!   `u8 / 255` values here. Computing them in raw `u8` space would change the
//!   epsilon's meaning; normalising keeps the branch decisions faithful.
//! - The reference runs at a sub-pixel `scale = 4` density and downsamples; this
//!   port matches that with a `4×4 = 16` fragment supersample per output pixel
//!   ([`SUPERSAMPLE`]), merged by plurality. A `1×` variant (`ss = 1`) remains
//!   available to the comparison harness as the pre-supersample baseline.
//! - `similarThreshold = 0.0` means `similar` is "exact RGBA equality OR both
//!   alpha == 0"; the port uses that equality directly (mathematically
//!   identical to `cd <= 0.0`, without a needless `sqrt`).
//! - `lineWidth = 1.0`, clamped to the SLOPE `[0.45, 1.142]`.
//! - No interpolation is ever performed: every output byte is copied verbatim
//!   from a sampled source pixel (or is the transparent sentinel), and the vote
//!   only selects one fragment colour, so the result is palette-pure and
//!   contains no anti-aliasing.

use super::{checked_dims, BYTES_PER_PIXEL};

/// RGBA8 colour, the only colour representation this module emits.
type Rgba = [u8; 4];

/// `lineWidth` uniform from the reference.
const LINE_WIDTH: f64 = 1.0;
/// `similarThreshold` uniform from the reference (0 → exact-equality matching).
const SIMILAR_THRESHOLD: f64 = 0.0;
/// SLOPE-branch lower clamp on the slice line width.
const MIN_WIDTH: f64 = 0.45;
/// SLOPE-branch upper clamp on the slice line width.
const MAX_WIDTH: f64 = 1.142;

/// Rotates an RGBA8 buffer by `angle_deg` degrees **clockwise** about its
/// centre (screen coordinates, y-down) using a CPU port of the CleanEdge
/// algorithm.
///
/// Mirrors [`super::rot_sprite::rotate`]'s contract:
///
/// - empty/undersized buffer or a non-finite angle → `(Vec::new(), 0, 0)`;
/// - exact 0°/90°/180°/270° dispatch to the pixel-exact paths, so those angles
///   are byte-identical to the existing rotation;
/// - every other angle uses the CleanEdge core over the same
///   rotated-bounding-box dims as [`super::rot_sprite::rotated_bounds`].
///
/// Deterministic and palette-pure: every emitted pixel is a verbatim sample of
/// one source pixel (or fully transparent), with no interpolation.
pub fn clean_edge_rotate(
    src: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
) -> (Vec<u8>, usize, usize) {
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

    clean_edge_rotate_generic(src, w, h, angle_deg)
}

/// Supersample factor per output pixel: the reference builds a `scale = 4`
/// grid (16 fragment decisions) and downsamples, so we evaluate 4×4 = 16
/// fragment positions per destination pixel and merge them.
const SUPERSAMPLE: usize = 4;

/// Enable the reference's `#ifdef CLEANUP` slant-transition cleanup (small
/// artifact removal on 2:1 slants and the 45° diagonal). See [`slice_dist`].
const ENABLE_CLEANUP: bool = true;

/// Weight of an opaque fragment in the supersample plurality vote. Opaque
/// fragments count double so a thin 1px structure is not out-voted by the
/// (weight-1) transparent background — the same balance RotSprite uses. Still a
/// pure plurality over fragment colours (no interpolation).
const OPAQUE_VOTE_WEIGHT: u32 = 2;
/// Weight of a fully transparent fragment in the plurality vote.
const TRANSPARENT_VOTE_WEIGHT: u32 = 1;

/// Generic-angle CleanEdge rotation (production path): 4× supersample with a
/// palette-pure plurality downscale, plus the `CLEANUP` phase.
///
/// Destination dims and the inverse map match [`super::rot_sprite`]'s 1×
/// reverse map so output geometry is comparable.
fn clean_edge_rotate_generic(
    src: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
) -> (Vec<u8>, usize, usize) {
    clean_edge_rotate_generic_opts(src, w, h, angle_deg, SUPERSAMPLE, ENABLE_CLEANUP)
}

/// Parameterised CleanEdge core.
///
/// - `ss` is the per-axis supersample factor: `ss = 1` evaluates the single
///   destination-pixel centre (exactly the original 1× port), `ss = 4` the
///   production 16-fragment grid. Every fragment is an independent CleanEdge
///   decision at its own sub-pixel phase.
/// - `cleanup` toggles the reference `CLEANUP` blocks inside [`slice_dist`].
///
/// The `ss²` fragment colours are merged by an **alpha-weighted plurality
/// vote**: each opaque fragment counts [`OPAQUE_VOTE_WEIGHT`] and each
/// transparent fragment [`TRANSPARENT_VOTE_WEIGHT`], so thin structure is not
/// out-voted by the background; ties go to the lowest `[r, g, b, a]` key. No
/// interpolation/blending ever happens (the winner is one fragment's verbatim
/// colour, alpha included), so the result stays palette-pure, AA-free and
/// deterministic. Used by the comparison harness to measure BEFORE
/// (`ss = 1, cleanup = false`) vs AFTER (`ss = 4, cleanup = true`).
fn clean_edge_rotate_generic_opts(
    src: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
    ss: usize,
    cleanup: bool,
) -> (Vec<u8>, usize, usize) {
    let (w_prime, h_prime) = super::rot_sprite::rotated_bounds(w, h, angle_deg);
    if w_prime == 0 || h_prime == 0 || ss == 0 {
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

    // Build the per-rotation palette (colour → index) and, when the palette is
    // small enough, the `K×K` pairwise predicate tables. Both are pure lookups
    // of the source colours; all predicate values are computed with the same
    // formulas as the direct fallback, so the output is byte-identical.
    let Some(palette) = Palette::build(src, w, h) else {
        return (Vec::new(), 0, 0);
    };
    let tables = Tables::build(&palette.colors);

    let theta = (angle_deg as f64).to_radians();
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    let src_cx = (w as f64 - 1.0) / 2.0;
    let src_cy = (h as f64 - 1.0) / 2.0;
    let dst_cx = (w_prime as f64 - 1.0) / 2.0;
    let dst_cy = (h_prime as f64 - 1.0) / 2.0;

    // One fragment per supersample cell, at the cell centre; the set spans the
    // destination pixel `[dx - 0.5, dx + 0.5]` (and likewise for `y`).
    let step = 1.0 / ss as f64;
    let half = step / 2.0;

    // Reusable per-pixel fragment tally (≤ ss² entries; ss ≤ SUPERSAMPLE).
    let mut keys = [[0u8; 4]; SUPERSAMPLE * SUPERSAMPLE];
    let mut counts = [0u32; SUPERSAMPLE * SUPERSAMPLE];

    for dy in 0..h_prime {
        for dx in 0..w_prime {
            let mut n = 0usize;
            for j in 0..ss {
                let oy = -0.5 + half + j as f64 * step;
                for i in 0..ss {
                    let ox = -0.5 + half + i as f64 * step;

                    let rel_x = dx as f64 + ox - dst_cx;
                    let rel_y = dy as f64 + oy - dst_cy;

                    // Inverse (clockwise) rotation into source space.
                    let sx = cos_t * rel_x + sin_t * rel_y + src_cx;
                    let sy = -sin_t * rel_x + cos_t * rel_y + src_cy;

                    let col =
                        clean_edge_pixel(src, w, h, sx, sy, cleanup, &palette, tables.as_ref());

                    // Alpha-weighted plurality tally (palette-pure: only
                    // fragment colours are ever emitted).
                    let weight = if col[3] == 0 {
                        TRANSPARENT_VOTE_WEIGHT
                    } else {
                        OPAQUE_VOTE_WEIGHT
                    };
                    let mut found = false;
                    for k in 0..n {
                        if keys[k] == col {
                            counts[k] += weight;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        keys[n] = col;
                        counts[n] = weight;
                        n += 1;
                    }
                }
            }

            // Plurality colour; ties → lowest RGBA key (deterministic).
            let mut best = 0usize;
            for k in 1..n {
                if counts[k] > counts[best] || (counts[k] == counts[best] && keys[k] < keys[best]) {
                    best = k;
                }
            }
            let col = keys[best];

            let di = (dy * w_prime + dx) * BYTES_PER_PIXEL;
            out[di..di + BYTES_PER_PIXEL].copy_from_slice(&col);
        }
    }

    (out, w_prime, h_prime)
}

/// Runs the shader's per-fragment body for one source-space sample coordinate.
///
/// `local`/`px`/`pointDir` and the 21 neighbour samples reproduce the reference
/// `mainImage` setup; `slice_dist` is invoked three times with the reference's
/// exact argument orderings and the `c` → `b` → `u` override precedence.
/// `cleanup` is forwarded to [`slice_dist`] (the reference `#ifdef CLEANUP`).
fn clean_edge_pixel(
    src: &[u8],
    w: usize,
    h: usize,
    sx: f64,
    sy: f64,
    cleanup: bool,
    palette: &Palette,
    tables: Option<&Tables>,
) -> Rgba {
    // GLSL `fract` is `x - floor(x)` (always in `[0,1)`), which differs from
    // Rust's `fract` for negative inputs.
    let local_x = sx - sx.floor();
    let local_y = sy - sy.floor();
    // GLSL `ceil`, then cell-centre `-0.5`; the integer cell index is `ceil-1`.
    let base_x = sx.ceil() as isize - 1;
    let base_y = sy.ceil() as isize - 1;
    let pdx = (local_x.round() * 2.0 - 1.0) as isize;
    let pdy = (local_y.round() * 2.0 - 1.0) as isize;

    let sample = |ox: isize, oy: isize| -> Nb {
        let ix = base_x + ox * pdx;
        let iy = base_y + oy * pdy;
        if ix >= 0 && iy >= 0 && (ix as usize) < w && (iy as usize) < h {
            let p = (iy as usize) * w + (ix as usize);
            let i = p * BYTES_PER_PIXEL;
            nb(
                [src[i], src[i + 1], src[i + 2], src[i + 3]],
                palette.indices[p],
            )
        } else {
            nb([0, 0, 0, 0], palette.sentinel)
        }
    };

    // The 21 neighbours, named exactly as in the reference `mainImage`. Each is
    // a sampled colour plus its palette index, so the pairwise predicate tables
    // can answer `cd`/`similar`/`higher` by index.
    let uub = sample(-1, -2);
    let uu = sample(0, -2);
    let uuf = sample(1, -2);
    let ubb = sample(-2, -2);
    let ub = sample(-1, -1);
    let u = sample(0, -1);
    let uf = sample(1, -1);
    let uff = sample(2, -1);
    let bb = sample(-2, 0);
    let b = sample(-1, 0);
    let c = sample(0, 0);
    let f = sample(1, 0);
    let ff = sample(2, 0);
    let dbb = sample(-2, 1);
    let db = sample(-1, 1);
    let d = sample(0, 1);
    let df = sample(1, 1);
    let dff = sample(2, 1);
    let ddb = sample(-1, 2);
    let dd = sample(0, 2);
    let ddf = sample(1, 2);

    let point = [local_x, local_y];
    let point_dir = [pdx as f64, pdy as f64];

    // The reference evaluates the corner, back and up slices in order and lets
    // each later `Some` override the previous one, so the final colour is the
    // LAST slice that returns a colour. `slice_dist` is pure, so evaluating the
    // slices in REVERSE order and returning on the first `Some` yields exactly
    // the same colour while skipping the earlier slices when a later one
    // decides (Y2-A; byte-identical, see `clean_edge_optimized_matches_reference_bytes`).
    if let Some(v) = slice_dist(
        point,
        [1.0, -1.0],
        point_dir,
        db,
        d,
        df,
        dff,
        b,
        c,
        f,
        ff,
        ub,
        u,
        uf,
        uff,
        uub,
        uu,
        uuf,
        tables,
        cleanup,
    ) {
        return v;
    }
    if let Some(v) = slice_dist(
        point,
        [-1.0, 1.0],
        point_dir,
        uf,
        u,
        ub,
        ubb,
        f,
        c,
        b,
        bb,
        df,
        d,
        db,
        dbb,
        ddf,
        dd,
        ddb,
        tables,
        cleanup,
    ) {
        return v;
    }
    if let Some(v) = slice_dist(
        point,
        [1.0, 1.0],
        point_dir,
        ub,
        u,
        uf,
        uff,
        b,
        c,
        f,
        ff,
        db,
        d,
        df,
        dff,
        ddb,
        dd,
        ddf,
        tables,
        cleanup,
    ) {
        return v;
    }
    c.raw
}

// -- Colours and predicates (faithful ports) --------------------------------

/// `v as f64 / 255.0` for every channel value, precomputed once.
///
/// `norm` is a pure function of a source colour. A `static` (not a `const`)
/// keeps one shared table instead of inlining the whole 2 KiB literal into
/// every `norm` call site in unoptimized builds.
static NORM: [f64; 256] = {
    let mut table = [0.0f64; 256];
    let mut i = 0usize;
    while i < 256 {
        table[i] = i as f64 / 255.0;
        i += 1;
    }
    table
};

/// Normalised `[0,1]` RGBA used for every distance metric (the reference samples
/// its texture in normalised space). Table lookup of `v / 255.0` (bit-identical
/// to the division).
#[inline(always)]
fn norm(c: Rgba) -> [f64; 4] {
    [
        NORM[c[0] as usize],
        NORM[c[1] as usize],
        NORM[c[2] as usize],
        NORM[c[3] as usize],
    ]
}

/// A sampled neighbour: its verbatim RGBA (the only thing ever emitted) plus
/// the index of that colour in the per-rotation palette.
///
/// The palette index lets [`Tables`] answer `cd`/`similar`/`higher` for a pair
/// with a single array load instead of a `norm`/`sqrt` chain.
#[derive(Clone, Copy)]
struct Nb {
    raw: Rgba,
    idx: u32,
}

/// Wrap a sampled colour with its palette index.
#[inline(always)]
fn nb(raw: Rgba, idx: u32) -> Nb {
    Nb { raw, idx }
}

/// Squared Euclidean distance between two normalised colours.
#[inline(always)]
fn sq_n(a: [f64; 4], b: [f64; 4]) -> f64 {
    let mut sum = 0.0;
    for i in 0..4 {
        let d = a[i] - b[i];
        sum += d * d;
    }
    sum
}

/// **Squared** Euclidean RGBA colour distance (`cd²`), computed directly from
/// the verbatim colours. Only used when the palette is too large for
/// [`Tables`] (the fallback path).
#[inline(always)]
fn cd_sq_direct(a: Nb, b: Nb) -> f64 {
    sq_n(norm(a.raw), norm(b.raw))
}

/// Euclidean RGBA colour distance (`cd` from the reference), direct fallback.
#[inline(always)]
fn cd_direct(a: Nb, b: Nb) -> f64 {
    sq_n(norm(a.raw), norm(b.raw)).sqrt()
}

/// `similar(a, b)`: exact equality, or both fully transparent. With
/// `similarThreshold = 0.0` this is exactly `(a.a == 0 && b.a == 0) ||
/// cd(a,b) <= 0.0`, evaluated on the squared distance (no `sqrt`).
#[inline(always)]
fn similar_direct(a: Nb, b: Nb) -> bool {
    (a.raw[3] == 0 && b.raw[3] == 0) || cd_sq_direct(a, b) <= SIMILAR_THRESHOLD * SIMILAR_THRESHOLD
}

/// **Squared** normalised RGB distance to white (`highestColor = (1,1,1)`).
#[inline(always)]
fn white_sq(c: Rgba) -> f64 {
    let n = norm(c);
    let mut sum = 0.0;
    for i in 0..3 {
        let d = n[i] - 1.0;
        sum += d * d;
    }
    sum
}

/// `higher(thisCol, otherCol)`: `false` when similar; equal alpha → closer to
/// white wins; otherwise higher alpha wins. Direct fallback; the white-distance
/// comparison is on squared distances (monotonic equivalence), so no `sqrt`.
#[inline(always)]
fn higher_direct(a: Nb, b: Nb) -> bool {
    if similar_direct(a, b) {
        return false;
    }
    if a.raw[3] == b.raw[3] {
        white_sq(a.raw) < white_sq(b.raw)
    } else {
        a.raw[3] > b.raw[3]
    }
}

/// Precomputed pairwise colour predicates for a small palette.
///
/// The reference recomputes `norm`, the 4-D sum, `sqrt` and the white-distance
/// predicates for every comparison, on the same handful of source colours
/// millions of times. For pixel art the palette is tiny, so we build the exact
/// `cd`, `cd²`, `similar` and `higher` results **once per rotation** and reduce
/// the hot predicates to array loads. Every value is computed with the same
/// formulas as the direct path, so the output is byte-identical.
///
/// `build` returns `None` for a palette above [`TABLES_MAX_COLORS`] (or on
/// allocation failure); the caller then uses the direct fallback.
struct Tables {
    k: usize,
    cd: Vec<f64>,
    cd_sq: Vec<f64>,
    sim: Vec<bool>,
    hi: Vec<bool>,
}

/// Largest palette for which the `K×K` predicate tables are built (512² ≈
/// 262 k entries → ~4.2 MB of `f64` + 0.5 MB of `bool`).
const TABLES_MAX_COLORS: usize = 512;

impl Tables {
    fn build(palette: &[Rgba]) -> Option<Tables> {
        let k = palette.len();
        if k == 0 || k > TABLES_MAX_COLORS {
            return None;
        }
        let n = k.checked_mul(k)?;
        let mut tables = Tables {
            k,
            cd: Vec::new(),
            cd_sq: Vec::new(),
            sim: Vec::new(),
            hi: Vec::new(),
        };
        tables.cd.try_reserve_exact(n).ok()?;
        tables.cd.resize(n, 0.0);
        tables.cd_sq.try_reserve_exact(n).ok()?;
        tables.cd_sq.resize(n, 0.0);
        tables.sim.try_reserve_exact(n).ok()?;
        tables.sim.resize(n, false);
        tables.hi.try_reserve_exact(n).ok()?;
        tables.hi.resize(n, false);
        for i in 0..k {
            for j in 0..k {
                let (a, b) = (palette[i], palette[j]);
                let s = sq_n(norm(a), norm(b));
                let at = i * k + j;
                tables.cd[at] = s.sqrt();
                tables.cd_sq[at] = s;
                tables.sim[at] =
                    (a[3] == 0 && b[3] == 0) || s <= SIMILAR_THRESHOLD * SIMILAR_THRESHOLD;
                tables.hi[at] = higher_direct(Nb { raw: a, idx: 0 }, Nb { raw: b, idx: 0 });
            }
        }
        Some(tables)
    }

    #[inline(always)]
    fn cd_at(&self, a: u32, b: u32) -> f64 {
        self.cd[(a as usize) * self.k + b as usize]
    }
    #[inline(always)]
    fn cd_sq_at(&self, a: u32, b: u32) -> f64 {
        self.cd_sq[(a as usize) * self.k + b as usize]
    }
    #[inline(always)]
    fn sim_at(&self, a: u32, b: u32) -> bool {
        self.sim[(a as usize) * self.k + b as usize]
    }
    #[inline(always)]
    fn hi_at(&self, a: u32, b: u32) -> bool {
        self.hi[(a as usize) * self.k + b as usize]
    }
}

/// The rotation's colour palette plus a per-source-pixel palette index.
struct Palette {
    colors: Vec<Rgba>,
    indices: Vec<u32>,
    /// Palette index of the transparent sentinel (always present).
    sentinel: u32,
}

impl Palette {
    /// Collect the distinct source colours (first-seen order, deterministic)
    /// and index every pixel. The transparent `[0,0,0,0]` sentinel is always
    /// inserted so out-of-grid samples have a valid index.
    fn build(src: &[u8], w: usize, h: usize) -> Option<Palette> {
        let pixels = w.checked_mul(h)?;
        let mut colors: Vec<Rgba> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        indices.try_reserve_exact(pixels).ok()?;
        let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let pack = |c: Rgba| -> u32 {
            (c[0] as u32) | ((c[1] as u32) << 8) | ((c[2] as u32) << 16) | ((c[3] as u32) << 24)
        };
        for i in 0..pixels {
            let o = i * BYTES_PER_PIXEL;
            let c = [src[o], src[o + 1], src[o + 2], src[o + 3]];
            let key = pack(c);
            let idx = match map.get(&key) {
                Some(&idx) => idx,
                None => {
                    let idx = colors.len() as u32;
                    colors.push(c);
                    map.insert(key, idx);
                    idx
                }
            };
            indices.push(idx);
        }
        let sentinel = match map.get(&0) {
            Some(&idx) => idx,
            None => {
                let idx = colors.len() as u32;
                colors.push([0, 0, 0, 0]);
                idx
            }
        };
        Some(Palette {
            colors,
            indices,
            sentinel,
        })
    }
}

/// `distToLine(testPt, pt1, pt2, dir)` with 2-D f64 vector math.
fn dist_to_line(test: [f64; 2], pt1: [f64; 2], pt2: [f64; 2], dir: [f64; 2]) -> f64 {
    let line = [pt2[0] - pt1[0], pt2[1] - pt1[1]];
    let perp = [line[1], -line[0]];
    let to_pt1 = [pt1[0] - test[0], pt1[1] - test[1]];
    let sign = if perp[0] * dir[0] + perp[1] * dir[1] > 0.0 {
        1.0
    } else {
        -1.0
    };
    let len = (perp[0] * perp[0] + perp[1] * perp[1]).sqrt();
    let n = [perp[0] / len, perp[1] / len];
    sign * (n[0] * to_pt1[0] + n[1] * to_pt1[1])
}

/// `center + offset * pointDir` (the reference's recurring anchor expression).
fn anchor(center: [f64; 2], point_dir: [f64; 2], ox: f64, oy: f64) -> [f64; 2] {
    [center[0] + ox * point_dir[0], center[1] + oy * point_dir[1]]
}

/// Negates a direction vector.
fn neg(d: [f64; 2]) -> [f64; 2] {
    [-d[0], -d[1]]
}

/// Faithful port of the reference `sliceDist`. Returns `None` for the
/// `vec4(-1.0)` sentinel (no slice), else the neighbour colour to snap to.
///
/// `SLOPE` is always enabled. `cleanup` enables the reference's `#ifdef
/// CLEANUP` slant-transition cleanup: on the shallow/steep 2:1 slants and the
/// 45° diagonal it nudges `dist` toward a second candidate line (`min` for the
/// 2:1 slants, `max` for the diagonal) when a sub-pixel slope transition is
/// detected, which removes small staircase artifacts. The far-corner branches
/// have no cleanup in the reference.
#[allow(clippy::too_many_arguments)]
fn slice_dist(
    point: [f64; 2],
    main_dir: [f64; 2],
    point_dir: [f64; 2],
    ub: Nb,
    u: Nb,
    uf: Nb,
    uff: Nb,
    b: Nb,
    c: Nb,
    f: Nb,
    ff: Nb,
    db: Nb,
    d: Nb,
    df: Nb,
    dff: Nb,
    ddb: Nb,
    dd: Nb,
    ddf: Nb,
    tables: Option<&Tables>,
    cleanup: bool,
) -> Option<Rgba> {
    // The colour predicates, answered from the precomputed pairwise tables when
    // available (the common pixel-art case) and from the direct formulas
    // otherwise. The bodies inside `slice_dist` are unchanged.
    let cd = |a: Nb, b: Nb| match tables {
        Some(t) => t.cd_at(a.idx, b.idx),
        None => cd_direct(a, b),
    };
    let cd_sq = |a: Nb, b: Nb| match tables {
        Some(t) => t.cd_sq_at(a.idx, b.idx),
        None => cd_sq_direct(a, b),
    };
    let similar = |a: Nb, b: Nb| match tables {
        Some(t) => t.sim_at(a.idx, b.idx),
        None => similar_direct(a, b),
    };
    let similar3 = |a: Nb, b: Nb, c: Nb| similar(a, b) && similar(b, c);
    let similar4 = |a: Nb, b: Nb, c: Nb, d: Nb| similar(a, b) && similar(b, c) && similar(c, d);
    let higher = |a: Nb, b: Nb| match tables {
        Some(t) => t.hi_at(a.idx, b.idx),
        None => higher_direct(a, b),
    };

    // SLOPE width clamps; `lineWidth = 1.0` stays inside `[0.45, 1.142]`.
    let line_w = LINE_WIDTH.clamp(MIN_WIDTH, MAX_WIDTH);
    // Flip the sample point into this slice's frame.
    let point = [
        main_dir[0] * (point[0] - 0.5) + 0.5,
        main_dir[1] * (point[1] - 0.5) + 0.5,
    ];

    let dist_against = 4.0 * cd(f, d) + cd(uf, c) + cd(c, db) + cd(ff, df) + cd(df, dd);
    let dist_towards = 4.0 * cd(c, df) + cd(u, f) + cd(f, dff) + cd(b, d) + cd(d, ddf);
    let mut should_slice =
        (dist_against < dist_towards) || ((dist_against < dist_towards + 0.001) && !higher(c, f));
    if similar4(f, d, b, u) && similar4(uf, df, db, ub) && !similar(c, f) {
        // checkerboard edge case
        should_slice = false;
    }
    if !should_slice {
        return None;
    }

    let center = [0.5, 0.5];

    // SLOPE: lower shallow 2:1 slant.
    if similar3(f, d, db) && !similar3(f, d, b) && !similar(uf, db) {
        let flip;
        if similar(c, df) && higher(c, f) {
            // single pixel wide diagonal: don't flip
            flip = false;
        } else {
            let mut fl = false;
            if higher(c, f) {
                fl = true;
            }
            if similar(u, f) && !similar(c, df) && !higher(c, u) {
                fl = true;
            }
            flip = fl;
        }
        let mut dist = if flip {
            line_w
                - dist_to_line(
                    point,
                    anchor(center, point_dir, 1.5, -1.0),
                    anchor(center, point_dir, -0.5, 0.0),
                    neg(point_dir),
                )
        } else {
            dist_to_line(
                point,
                anchor(center, point_dir, 1.5, 0.0),
                anchor(center, point_dir, -0.5, 1.0),
                point_dir,
            )
        };
        // `#ifdef CLEANUP`: shallow slant-transition cleanup.
        if cleanup
            && !flip
            && similar(c, uf)
            && !(similar3(c, uf, uff) && !similar3(c, uf, ff) && !similar(d, uff))
        {
            let dist2 = dist_to_line(
                point,
                anchor(center, point_dir, 2.0, -1.0),
                anchor(center, point_dir, 0.0, 1.0),
                point_dir,
            );
            dist = dist.min(dist2);
        }
        dist -= line_w / 2.0;
        return if dist <= 0.0 {
            Some(if cd_sq(c, f) <= cd_sq(c, d) {
                f.raw
            } else {
                d.raw
            })
        } else {
            None
        };
    } else if similar3(uf, f, d) && !similar3(u, f, d) && !similar(uf, db) {
        // SLOPE: forward steep 2:1 slant.
        let flip;
        if similar(c, df) && higher(c, d) {
            flip = false;
        } else {
            let mut fl = false;
            if higher(c, d) {
                fl = true;
            }
            if similar(b, d) && !similar(c, df) && !higher(c, d) {
                fl = true;
            }
            flip = fl;
        }
        let mut dist = if flip {
            line_w
                - dist_to_line(
                    point,
                    anchor(center, point_dir, 0.0, -0.5),
                    anchor(center, point_dir, -1.0, 1.5),
                    neg(point_dir),
                )
        } else {
            dist_to_line(
                point,
                anchor(center, point_dir, 1.0, -0.5),
                anchor(center, point_dir, 0.0, 1.5),
                point_dir,
            )
        };
        // `#ifdef CLEANUP`: steep slant-transition cleanup.
        if cleanup
            && !flip
            && similar(c, db)
            && !(similar3(c, db, ddb) && !similar3(c, db, dd) && !similar(f, ddb))
        {
            let dist2 = dist_to_line(
                point,
                anchor(center, point_dir, 1.0, 0.0),
                anchor(center, point_dir, -1.0, 2.0),
                point_dir,
            );
            dist = dist.min(dist2);
        }
        dist -= line_w / 2.0;
        return if dist <= 0.0 {
            Some(if cd_sq(c, f) <= cd_sq(c, d) {
                f.raw
            } else {
                d.raw
            })
        } else {
            None
        };
    }

    if similar(f, d) {
        // 45° diagonal.
        let mut flip = false;
        if similar(c, df) && higher(c, f) {
            // single pixel diagonal along neighbours: don't flip
            if !similar(c, dd) && !similar(c, ff) {
                flip = true; // line against triple colour stripe edge case
            }
        } else {
            if higher(c, f) {
                flip = true;
            }
            if !similar(c, b) && similar4(b, f, d, u) {
                flip = true;
            }
        }
        // single pixel 2:1 slope: don't flip
        if ((similar(f, db) && similar3(u, f, df)) || (similar(uf, d) && similar3(b, d, df)))
            && !similar(c, df)
        {
            flip = true;
        }
        let mut dist = if flip {
            line_w
                - dist_to_line(
                    point,
                    anchor(center, point_dir, 1.0, -1.0),
                    anchor(center, point_dir, -1.0, 1.0),
                    neg(point_dir),
                )
        } else {
            dist_to_line(
                point,
                anchor(center, point_dir, 1.0, 0.0),
                anchor(center, point_dir, 0.0, 1.0),
                point_dir,
            )
        };
        // `#ifdef SLOPE` + `#ifdef CLEANUP`: diagonal slant-transition cleanup.
        if cleanup && !flip && similar3(c, uf, uff) && !similar3(c, uf, ff) && !similar(d, uff) {
            let dist2 = dist_to_line(
                point,
                anchor(center, point_dir, 1.5, 0.0),
                anchor(center, point_dir, -0.5, 1.0),
                point_dir,
            );
            dist = dist.max(dist2);
        }
        if cleanup && !flip && similar3(ddb, db, c) && !similar3(dd, db, c) && !similar(ddb, f) {
            let dist2 = dist_to_line(
                point,
                anchor(center, point_dir, 1.0, -0.5),
                anchor(center, point_dir, 0.0, 1.5),
                point_dir,
            );
            dist = dist.max(dist2);
        }
        dist -= line_w / 2.0;
        return if dist <= 0.0 {
            Some(if cd_sq(c, f) <= cd_sq(c, d) {
                f.raw
            } else {
                d.raw
            })
        } else {
            None
        };
    } else if similar3(ff, df, d) && !similar3(ff, df, c) && !similar(uff, d) {
        // SLOPE: far corner of shallow slant.
        let flip;
        if similar(f, dff) && higher(f, ff) {
            flip = false;
        } else {
            let mut fl = false;
            if higher(f, ff) {
                fl = true;
            }
            if similar(uf, ff) && !similar(f, dff) && !higher(f, uf) {
                fl = true;
            }
            flip = fl;
        }
        let mut dist = if flip {
            line_w
                - dist_to_line(
                    point,
                    anchor(center, point_dir, 2.5, -1.0),
                    anchor(center, point_dir, 0.5, 0.0),
                    neg(point_dir),
                )
        } else {
            dist_to_line(
                point,
                anchor(center, point_dir, 2.5, 0.0),
                anchor(center, point_dir, 0.5, 1.0),
                point_dir,
            )
        };
        dist -= line_w / 2.0;
        return if dist <= 0.0 {
            Some(if cd_sq(f, ff) <= cd_sq(f, df) {
                ff.raw
            } else {
                df.raw
            })
        } else {
            None
        };
    } else if similar3(f, df, dd) && !similar3(c, df, dd) && !similar(f, ddb) {
        // SLOPE: far corner of steep slant.
        let flip;
        if similar(d, ddf) && higher(d, dd) {
            flip = false;
        } else {
            let mut fl = false;
            if higher(d, dd) {
                fl = true;
            }
            if similar(db, dd) && !similar(d, ddf) && !higher(d, dd) {
                fl = true;
            }
            flip = fl;
        }
        let mut dist = if flip {
            line_w
                - dist_to_line(
                    point,
                    anchor(center, point_dir, 0.0, 0.5),
                    anchor(center, point_dir, -1.0, 2.5),
                    neg(point_dir),
                )
        } else {
            dist_to_line(
                point,
                anchor(center, point_dir, 1.0, 0.5),
                anchor(center, point_dir, 0.0, 2.5),
                point_dir,
            )
        };
        dist -= line_w / 2.0;
        return if dist <= 0.0 {
            Some(if cd_sq(d, df) <= cd_sq(d, dd) {
                df.raw
            } else {
                dd.raw
            })
        } else {
            None
        };
    }

    None
}

/// Zeroed allocation that never panics (overflow / alloc failure → `None`).
fn alloc_zeroed(len: usize) -> Option<Vec<u8>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).ok()?;
    v.resize(len, 0);
    Some(v)
}

/// Test-only **verbatim** copy of the pre-optimization CleanEdge core, used as
/// the byte-identity oracle for the optimized production path. It is entirely
/// self-contained (its own `norm`/`cd`/`similar`/`higher`) so the comparison
/// proves the optimized helpers and comparisons change no output byte.
#[cfg(test)]
pub(super) mod reference {
    use super::{
        alloc_zeroed, checked_dims, Rgba, BYTES_PER_PIXEL, LINE_WIDTH, MAX_WIDTH, MIN_WIDTH,
        OPAQUE_VOTE_WEIGHT, SIMILAR_THRESHOLD, TRANSPARENT_VOTE_WEIGHT,
    };

    /// Pre-optimization parameterised CleanEdge core (verbatim).
    pub(super) fn clean_edge_rotate_generic_opts(
        src: &[u8],
        w: usize,
        h: usize,
        angle_deg: f32,
        ss: usize,
        cleanup: bool,
    ) -> (Vec<u8>, usize, usize) {
        let (w_prime, h_prime) = super::super::rot_sprite::rotated_bounds(w, h, angle_deg);
        if w_prime == 0 || h_prime == 0 || ss == 0 {
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

        let theta = (angle_deg as f64).to_radians();
        let cos_t = theta.cos();
        let sin_t = theta.sin();

        let src_cx = (w as f64 - 1.0) / 2.0;
        let src_cy = (h as f64 - 1.0) / 2.0;
        let dst_cx = (w_prime as f64 - 1.0) / 2.0;
        let dst_cy = (h_prime as f64 - 1.0) / 2.0;

        let step = 1.0 / ss as f64;
        let half = step / 2.0;

        let mut keys = [[0u8; 4]; 16];
        let mut counts = [0u32; 16];

        for dy in 0..h_prime {
            for dx in 0..w_prime {
                let mut n = 0usize;
                for j in 0..ss {
                    let oy = -0.5 + half + j as f64 * step;
                    for i in 0..ss {
                        let ox = -0.5 + half + i as f64 * step;

                        let rel_x = dx as f64 + ox - dst_cx;
                        let rel_y = dy as f64 + oy - dst_cy;

                        let sx = cos_t * rel_x + sin_t * rel_y + src_cx;
                        let sy = -sin_t * rel_x + cos_t * rel_y + src_cy;

                        let col = clean_edge_pixel(src, w, h, sx, sy, cleanup);

                        let weight = if col[3] == 0 {
                            TRANSPARENT_VOTE_WEIGHT
                        } else {
                            OPAQUE_VOTE_WEIGHT
                        };
                        let mut found = false;
                        for k in 0..n {
                            if keys[k] == col {
                                counts[k] += weight;
                                found = true;
                                break;
                            }
                        }
                        if !found {
                            keys[n] = col;
                            counts[n] = weight;
                            n += 1;
                        }
                    }
                }

                let mut best = 0usize;
                for k in 1..n {
                    if counts[k] > counts[best]
                        || (counts[k] == counts[best] && keys[k] < keys[best])
                    {
                        best = k;
                    }
                }
                let col = keys[best];

                let di = (dy * w_prime + dx) * BYTES_PER_PIXEL;
                out[di..di + BYTES_PER_PIXEL].copy_from_slice(&col);
            }
        }

        (out, w_prime, h_prime)
    }

    fn clean_edge_pixel(src: &[u8], w: usize, h: usize, sx: f64, sy: f64, cleanup: bool) -> Rgba {
        let local_x = sx - sx.floor();
        let local_y = sy - sy.floor();
        let base_x = sx.ceil() as isize - 1;
        let base_y = sy.ceil() as isize - 1;
        let pdx = (local_x.round() * 2.0 - 1.0) as isize;
        let pdy = (local_y.round() * 2.0 - 1.0) as isize;

        let sample = |ox: isize, oy: isize| -> Rgba {
            let ix = base_x + ox * pdx;
            let iy = base_y + oy * pdy;
            if ix >= 0 && iy >= 0 && (ix as usize) < w && (iy as usize) < h {
                let i = ((iy as usize) * w + (ix as usize)) * BYTES_PER_PIXEL;
                [src[i], src[i + 1], src[i + 2], src[i + 3]]
            } else {
                [0, 0, 0, 0]
            }
        };

        let uub = sample(-1, -2);
        let uu = sample(0, -2);
        let uuf = sample(1, -2);
        let ubb = sample(-2, -2);
        let ub = sample(-1, -1);
        let u = sample(0, -1);
        let uf = sample(1, -1);
        let uff = sample(2, -1);
        let bb = sample(-2, 0);
        let b = sample(-1, 0);
        let c = sample(0, 0);
        let f = sample(1, 0);
        let ff = sample(2, 0);
        let dbb = sample(-2, 1);
        let db = sample(-1, 1);
        let d = sample(0, 1);
        let df = sample(1, 1);
        let dff = sample(2, 1);
        let ddb = sample(-1, 2);
        let dd = sample(0, 2);
        let ddf = sample(1, 2);

        let point = [local_x, local_y];
        let point_dir = [pdx as f64, pdy as f64];

        let mut col = c;
        if let Some(v) = slice_dist(
            point,
            [1.0, 1.0],
            point_dir,
            ub,
            u,
            uf,
            uff,
            b,
            c,
            f,
            ff,
            db,
            d,
            df,
            dff,
            ddb,
            dd,
            ddf,
            cleanup,
        ) {
            col = v;
        }
        if let Some(v) = slice_dist(
            point,
            [-1.0, 1.0],
            point_dir,
            uf,
            u,
            ub,
            ubb,
            f,
            c,
            b,
            bb,
            df,
            d,
            db,
            dbb,
            ddf,
            dd,
            ddb,
            cleanup,
        ) {
            col = v;
        }
        if let Some(v) = slice_dist(
            point,
            [1.0, -1.0],
            point_dir,
            db,
            d,
            df,
            dff,
            b,
            c,
            f,
            ff,
            ub,
            u,
            uf,
            uff,
            uub,
            uu,
            uuf,
            cleanup,
        ) {
            col = v;
        }
        col
    }

    fn norm(c: Rgba) -> [f64; 4] {
        [
            c[0] as f64 / 255.0,
            c[1] as f64 / 255.0,
            c[2] as f64 / 255.0,
            c[3] as f64 / 255.0,
        ]
    }

    fn cd(a: Rgba, b: Rgba) -> f64 {
        let (x, y) = (norm(a), norm(b));
        let mut sum = 0.0;
        for i in 0..4 {
            let d = x[i] - y[i];
            sum += d * d;
        }
        sum.sqrt()
    }

    fn similar(a: Rgba, b: Rgba) -> bool {
        (a[3] == 0 && b[3] == 0) || cd(a, b) <= SIMILAR_THRESHOLD
    }

    fn similar3(a: Rgba, b: Rgba, c: Rgba) -> bool {
        similar(a, b) && similar(b, c)
    }

    fn similar4(a: Rgba, b: Rgba, c: Rgba, d: Rgba) -> bool {
        similar(a, b) && similar(b, c) && similar(c, d)
    }

    fn rgb_dist_to_white(c: Rgba) -> f64 {
        let n = norm(c);
        let mut sum = 0.0;
        for i in 0..3 {
            let d = n[i] - 1.0;
            sum += d * d;
        }
        sum.sqrt()
    }

    fn higher(a: Rgba, b: Rgba) -> bool {
        if similar(a, b) {
            return false;
        }
        if a[3] == b[3] {
            rgb_dist_to_white(a) < rgb_dist_to_white(b)
        } else {
            a[3] > b[3]
        }
    }

    fn dist_to_line(test: [f64; 2], pt1: [f64; 2], pt2: [f64; 2], dir: [f64; 2]) -> f64 {
        let line = [pt2[0] - pt1[0], pt2[1] - pt1[1]];
        let perp = [line[1], -line[0]];
        let to_pt1 = [pt1[0] - test[0], pt1[1] - test[1]];
        let sign = if perp[0] * dir[0] + perp[1] * dir[1] > 0.0 {
            1.0
        } else {
            -1.0
        };
        let len = (perp[0] * perp[0] + perp[1] * perp[1]).sqrt();
        let n = [perp[0] / len, perp[1] / len];
        sign * (n[0] * to_pt1[0] + n[1] * to_pt1[1])
    }

    fn anchor(center: [f64; 2], point_dir: [f64; 2], ox: f64, oy: f64) -> [f64; 2] {
        [center[0] + ox * point_dir[0], center[1] + oy * point_dir[1]]
    }

    fn neg(d: [f64; 2]) -> [f64; 2] {
        [-d[0], -d[1]]
    }

    #[allow(clippy::too_many_arguments)]
    fn slice_dist(
        point: [f64; 2],
        main_dir: [f64; 2],
        point_dir: [f64; 2],
        ub: Rgba,
        u: Rgba,
        uf: Rgba,
        uff: Rgba,
        b: Rgba,
        c: Rgba,
        f: Rgba,
        ff: Rgba,
        db: Rgba,
        d: Rgba,
        df: Rgba,
        dff: Rgba,
        ddb: Rgba,
        dd: Rgba,
        ddf: Rgba,
        cleanup: bool,
    ) -> Option<Rgba> {
        let line_w = LINE_WIDTH.clamp(MIN_WIDTH, MAX_WIDTH);
        let point = [
            main_dir[0] * (point[0] - 0.5) + 0.5,
            main_dir[1] * (point[1] - 0.5) + 0.5,
        ];

        let dist_against = 4.0 * cd(f, d) + cd(uf, c) + cd(c, db) + cd(ff, df) + cd(df, dd);
        let dist_towards = 4.0 * cd(c, df) + cd(u, f) + cd(f, dff) + cd(b, d) + cd(d, ddf);
        let mut should_slice = (dist_against < dist_towards)
            || ((dist_against < dist_towards + 0.001) && !higher(c, f));
        if similar4(f, d, b, u) && similar4(uf, df, db, ub) && !similar(c, f) {
            should_slice = false;
        }
        if !should_slice {
            return None;
        }

        let center = [0.5, 0.5];

        if similar3(f, d, db) && !similar3(f, d, b) && !similar(uf, db) {
            let flip;
            if similar(c, df) && higher(c, f) {
                flip = false;
            } else {
                let mut fl = false;
                if higher(c, f) {
                    fl = true;
                }
                if similar(u, f) && !similar(c, df) && !higher(c, u) {
                    fl = true;
                }
                flip = fl;
            }
            let mut dist = if flip {
                line_w
                    - dist_to_line(
                        point,
                        anchor(center, point_dir, 1.5, -1.0),
                        anchor(center, point_dir, -0.5, 0.0),
                        neg(point_dir),
                    )
            } else {
                dist_to_line(
                    point,
                    anchor(center, point_dir, 1.5, 0.0),
                    anchor(center, point_dir, -0.5, 1.0),
                    point_dir,
                )
            };
            if cleanup
                && !flip
                && similar(c, uf)
                && !(similar3(c, uf, uff) && !similar3(c, uf, ff) && !similar(d, uff))
            {
                let dist2 = dist_to_line(
                    point,
                    anchor(center, point_dir, 2.0, -1.0),
                    anchor(center, point_dir, 0.0, 1.0),
                    point_dir,
                );
                dist = dist.min(dist2);
            }
            dist -= line_w / 2.0;
            return if dist <= 0.0 {
                Some(if cd(c, f) <= cd(c, d) { f } else { d })
            } else {
                None
            };
        } else if similar3(uf, f, d) && !similar3(u, f, d) && !similar(uf, db) {
            let flip;
            if similar(c, df) && higher(c, d) {
                flip = false;
            } else {
                let mut fl = false;
                if higher(c, d) {
                    fl = true;
                }
                if similar(b, d) && !similar(c, df) && !higher(c, d) {
                    fl = true;
                }
                flip = fl;
            }
            let mut dist = if flip {
                line_w
                    - dist_to_line(
                        point,
                        anchor(center, point_dir, 0.0, -0.5),
                        anchor(center, point_dir, -1.0, 1.5),
                        neg(point_dir),
                    )
            } else {
                dist_to_line(
                    point,
                    anchor(center, point_dir, 1.0, -0.5),
                    anchor(center, point_dir, 0.0, 1.5),
                    point_dir,
                )
            };
            if cleanup
                && !flip
                && similar(c, db)
                && !(similar3(c, db, ddb) && !similar3(c, db, dd) && !similar(f, ddb))
            {
                let dist2 = dist_to_line(
                    point,
                    anchor(center, point_dir, 1.0, 0.0),
                    anchor(center, point_dir, -1.0, 2.0),
                    point_dir,
                );
                dist = dist.min(dist2);
            }
            dist -= line_w / 2.0;
            return if dist <= 0.0 {
                Some(if cd(c, f) <= cd(c, d) { f } else { d })
            } else {
                None
            };
        }

        if similar(f, d) {
            let mut flip = false;
            if similar(c, df) && higher(c, f) {
                if !similar(c, dd) && !similar(c, ff) {
                    flip = true;
                }
            } else {
                if higher(c, f) {
                    flip = true;
                }
                if !similar(c, b) && similar4(b, f, d, u) {
                    flip = true;
                }
            }
            if ((similar(f, db) && similar3(u, f, df)) || (similar(uf, d) && similar3(b, d, df)))
                && !similar(c, df)
            {
                flip = true;
            }
            let mut dist = if flip {
                line_w
                    - dist_to_line(
                        point,
                        anchor(center, point_dir, 1.0, -1.0),
                        anchor(center, point_dir, -1.0, 1.0),
                        neg(point_dir),
                    )
            } else {
                dist_to_line(
                    point,
                    anchor(center, point_dir, 1.0, 0.0),
                    anchor(center, point_dir, 0.0, 1.0),
                    point_dir,
                )
            };
            if cleanup && !flip && similar3(c, uf, uff) && !similar3(c, uf, ff) && !similar(d, uff)
            {
                let dist2 = dist_to_line(
                    point,
                    anchor(center, point_dir, 1.5, 0.0),
                    anchor(center, point_dir, -0.5, 1.0),
                    point_dir,
                );
                dist = dist.max(dist2);
            }
            if cleanup && !flip && similar3(ddb, db, c) && !similar3(dd, db, c) && !similar(ddb, f)
            {
                let dist2 = dist_to_line(
                    point,
                    anchor(center, point_dir, 1.0, -0.5),
                    anchor(center, point_dir, 0.0, 1.5),
                    point_dir,
                );
                dist = dist.max(dist2);
            }
            dist -= line_w / 2.0;
            return if dist <= 0.0 {
                Some(if cd(c, f) <= cd(c, d) { f } else { d })
            } else {
                None
            };
        } else if similar3(ff, df, d) && !similar3(ff, df, c) && !similar(uff, d) {
            let flip;
            if similar(f, dff) && higher(f, ff) {
                flip = false;
            } else {
                let mut fl = false;
                if higher(f, ff) {
                    fl = true;
                }
                if similar(uf, ff) && !similar(f, dff) && !higher(f, uf) {
                    fl = true;
                }
                flip = fl;
            }
            let mut dist = if flip {
                line_w
                    - dist_to_line(
                        point,
                        anchor(center, point_dir, 2.5, -1.0),
                        anchor(center, point_dir, 0.5, 0.0),
                        neg(point_dir),
                    )
            } else {
                dist_to_line(
                    point,
                    anchor(center, point_dir, 2.5, 0.0),
                    anchor(center, point_dir, 0.5, 1.0),
                    point_dir,
                )
            };
            dist -= line_w / 2.0;
            return if dist <= 0.0 {
                Some(if cd(f, ff) <= cd(f, df) { ff } else { df })
            } else {
                None
            };
        } else if similar3(f, df, dd) && !similar3(c, df, dd) && !similar(f, ddb) {
            let flip;
            if similar(d, ddf) && higher(d, dd) {
                flip = false;
            } else {
                let mut fl = false;
                if higher(d, dd) {
                    fl = true;
                }
                if similar(db, dd) && !similar(d, ddf) && !higher(d, dd) {
                    fl = true;
                }
                flip = fl;
            }
            let mut dist = if flip {
                line_w
                    - dist_to_line(
                        point,
                        anchor(center, point_dir, 0.0, 0.5),
                        anchor(center, point_dir, -1.0, 2.5),
                        neg(point_dir),
                    )
            } else {
                dist_to_line(
                    point,
                    anchor(center, point_dir, 1.0, 0.5),
                    anchor(center, point_dir, 0.0, 2.5),
                    point_dir,
                )
            };
            dist -= line_w / 2.0;
            return if dist <= 0.0 {
                Some(if cd(d, df) <= cd(d, dd) { df } else { dd })
            } else {
                None
            };
        }

        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::Instant;

    // -- Fixtures -----------------------------------------------------------

    const RED: Rgba = [255, 0, 0, 255];
    const GREEN: Rgba = [60, 180, 90, 255];
    const BLUE: Rgba = [30, 60, 220, 255];

    fn blank(w: usize, h: usize) -> Vec<u8> {
        vec![0u8; w * h * 4]
    }

    fn set(buf: &mut [u8], w: usize, x: usize, y: usize, c: Rgba) {
        let i = (y * w + x) * 4;
        buf[i..i + 4].copy_from_slice(&c);
    }

    fn get(buf: &[u8], w: usize, x: usize, y: usize) -> Rgba {
        let i = (y * w + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// (a) 1px horizontal line.
    fn fx_hline() -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut b = blank(w, h);
        for x in 2..14 {
            set(&mut b, w, x, 8, RED);
        }
        (b, w, h)
    }

    /// (b) 1px vertical line.
    fn fx_vline() -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut b = blank(w, h);
        for y in 2..14 {
            set(&mut b, w, 8, y, RED);
        }
        (b, w, h)
    }

    /// (c) 1px 45° diagonal.
    fn fx_diag45() -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut b = blank(w, h);
        for i in 2..14 {
            set(&mut b, w, i, i, RED);
        }
        (b, w, h)
    }

    /// (d) isolated single pixel.
    fn fx_isolated() -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut b = blank(w, h);
        set(&mut b, w, 8, 8, RED);
        (b, w, h)
    }

    /// (e) 1px thick 2:1 diagonal (two horizontal steps per row).
    fn fx_thin21() -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut b = blank(w, h);
        for x in 2..14 {
            let y = 2 + (x - 2) / 2;
            set(&mut b, w, x, y, RED);
        }
        (b, w, h)
    }

    /// (f) dithering checkerboard, two alternating opaque colours on a
    /// transparent background (12×12 inset field).
    fn fx_checker() -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut b = blank(w, h);
        for y in 2..14 {
            for x in 2..14 {
                set(&mut b, w, x, y, if (x + y) % 2 == 0 { GREEN } else { BLUE });
            }
        }
        (b, w, h)
    }

    /// (g) small 8×8 sprite with a border and a diagonal.
    fn fx_sprite() -> (Vec<u8>, usize, usize) {
        let (w, h) = (8, 8);
        let mut b = blank(w, h);
        for x in 0..w {
            set(&mut b, w, x, 0, GREEN);
            set(&mut b, w, x, h - 1, GREEN);
        }
        for y in 0..h {
            set(&mut b, w, 0, y, GREEN);
            set(&mut b, w, w - 1, y, GREEN);
        }
        for i in 1..w - 1 {
            set(&mut b, w, i, i, BLUE);
        }
        (b, w, h)
    }

    /// (h) flat opaque area (12×12 solid inset on a transparent background).
    fn fx_flat() -> (Vec<u8>, usize, usize) {
        let (w, h) = (16, 16);
        let mut b = blank(w, h);
        for y in 2..14 {
            for x in 2..14 {
                set(&mut b, w, x, y, GREEN);
            }
        }
        (b, w, h)
    }

    pub(crate) fn all_fixtures() -> Vec<(&'static str, Vec<u8>, usize, usize)> {
        let builders: [(&'static str, fn() -> (Vec<u8>, usize, usize)); 8] = [
            ("hline", fx_hline),
            ("vline", fx_vline),
            ("diag45", fx_diag45),
            ("isolated", fx_isolated),
            ("thin21", fx_thin21),
            ("checker", fx_checker),
            ("sprite", fx_sprite),
            ("flat", fx_flat),
        ];
        builders
            .into_iter()
            .map(|(name, build)| {
                let (buf, w, h) = build();
                (name, buf, w, h)
            })
            .collect()
    }

    // -- Metrics (duplicated from rot_sprite tests; rot_sprite.rs untouched) --

    /// Opaque (`alpha == 255`) cell coordinates.
    fn opaque_cells(out: &[u8], w: usize, h: usize) -> Vec<(usize, usize)> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| get(out, w, x, y)[3] == 255)
            .collect()
    }

    /// Size of the largest 4-connected component of `cells`.
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

    /// Number of 4-connected components among `cells`.
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

    /// **Broken metric** — verbatim replication of `rot_sprite.rs`'s
    /// `line_integrity`: `(opaque, broken)` where `broken` is the number of
    /// opaque output pixels not reachable from the largest 4-connected run.
    fn line_integrity(out: &[u8], w: usize, h: usize) -> (usize, usize) {
        let cells = opaque_cells(out, w, h);
        let largest = largest_component_4(&cells);
        (cells.len(), cells.len() - largest)
    }

    /// **Thin-detail metric** — `(opaque_count, 4-connected_components)`.
    /// Compared against the source: a drop in components means thin detail was
    /// erased; a change in opaque count means pixels were lost or blobbed.
    fn detail_stats(out: &[u8], w: usize, h: usize) -> (usize, usize) {
        let cells = opaque_cells(out, w, h);
        (cells.len(), component_count_4(&cells))
    }

    /// **Dithering metric** — fraction of opaque destination pixels that have
    /// at least one orthogonal opaque neighbour of the *same* RGBA. For an
    /// ideal checkerboard every orthogonal neighbour differs, so this is 0.0;
    /// merging/smearing raises it. Only meaningful for the checker fixture.
    fn dither_degradation(out: &[u8], w: usize, h: usize) -> f64 {
        let mut opaque = 0usize;
        let mut breaking = 0usize;
        for y in 0..h {
            for x in 0..w {
                let c = get(out, w, x, y);
                if c[3] == 0 {
                    continue;
                }
                opaque += 1;
                let mut bad = false;
                for (dx, dy) in [(1isize, 0isize), (-1, 0), (0, 1), (0, -1)] {
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                        continue;
                    }
                    let n = get(out, w, nx as usize, ny as usize);
                    if n[3] != 0 && n == c {
                        bad = true;
                    }
                }
                if bad {
                    breaking += 1;
                }
            }
        }
        if opaque == 0 {
            0.0
        } else {
            breaking as f64 / opaque as f64
        }
    }

    /// Palette purity: every output RGBA exists in the input.
    pub(crate) fn palette_pure(src: &[u8], out: &[u8]) -> bool {
        let pal: HashSet<Rgba> = src
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        out.chunks_exact(4)
            .all(|c| pal.contains(&[c[0], c[1], c[2], c[3]]))
    }

    /// No partial alpha (only 0 or 255) — i.e. no anti-aliasing.
    pub(crate) fn alpha_binary(out: &[u8]) -> bool {
        out.chunks_exact(4).all(|c| c[3] == 0 || c[3] == 255)
    }

    // -- Deterministic invariant assertions ---------------------------------

    #[test]
    fn clean_edge_dims_match_baseline() {
        let baseline = super::super::rotate;
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (_, bw, bh) = baseline(&buf, w, h, angle);
                let (_, cw, ch) = clean_edge_rotate(&buf, w, h, angle);
                assert_eq!(
                    (cw, ch),
                    (bw, bh),
                    "dims differ for {name} @ {angle}°: clean_edge {cw}x{ch} vs baseline {bw}x{bh}"
                );
            }
        }
    }

    #[test]
    fn clean_edge_is_palette_pure_and_alpha_binary() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (out, _, _) = clean_edge_rotate(&buf, w, h, angle);
                assert!(
                    palette_pure(&buf, &out),
                    "clean_edge invented a colour for {name} @ {angle}°"
                );
                assert!(
                    alpha_binary(&out),
                    "clean_edge produced partial alpha for {name} @ {angle}°"
                );
            }
        }
    }

    #[test]
    fn clean_edge_is_deterministic() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                let (a, aw, ah) = clean_edge_rotate(&buf, w, h, angle);
                let (b, bw, bh) = clean_edge_rotate(&buf, w, h, angle);
                assert_eq!((aw, ah), (bw, bh), "{name} @ {angle}° dims");
                assert_eq!(a, b, "{name} @ {angle}° bytes");
            }
        }
    }

    /// **Y2-A hard requirement**: the optimized production core must reproduce
    /// the pre-optimization [`reference`] core **byte-for-byte**, across every
    /// fixture × angle, for both the production `ss=4, cleanup=true` path and
    /// the `ss=1, cleanup=false` baseline.
    #[test]
    fn clean_edge_optimized_matches_reference_bytes() {
        for (name, buf, w, h) in all_fixtures() {
            for angle in comparison_angles() {
                for (ss, cleanup) in [(1usize, false), (4, true)] {
                    let optimized = clean_edge_rotate_generic_opts(&buf, w, h, angle, ss, cleanup);
                    let expected = super::reference::clean_edge_rotate_generic_opts(
                        &buf, w, h, angle, ss, cleanup,
                    );
                    assert_eq!(
                        optimized, expected,
                        "optimized CleanEdge must be byte-identical for {name} @ {angle}° \
                         (ss={ss}, cleanup={cleanup})"
                    );
                }
            }
        }
    }

    #[test]
    fn clean_edge_handles_empty_and_undersized() {
        let (out, ow, oh) = clean_edge_rotate(&[], 0, 0, 30.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));

        let (out, ow, oh) = clean_edge_rotate(&[0u8; 8], 3, 3, 30.0);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));

        let (out, ow, oh) = clean_edge_rotate(&[0u8; 16], 4, 4, f32::NAN);
        assert!(out.is_empty());
        assert_eq!((ow, oh), (0, 0));
    }

    #[test]
    fn clean_edge_exact_angles() {
        let (buf, w, h) = fx_diag45();

        let (out, ow, oh) = clean_edge_rotate(&buf, w, h, 0.0);
        assert_eq!((ow, oh), (w, h));
        assert_eq!(out, buf);

        let (r, rw, rh) = clean_edge_rotate(&buf, w, h, 90.0);
        let (e, ew, eh) = super::super::exact::rotate_90_cw(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);

        let (r, rw, rh) = clean_edge_rotate(&buf, w, h, 180.0);
        let (e, ew, eh) = super::super::exact::rotate_180(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);

        let (r, rw, rh) = clean_edge_rotate(&buf, w, h, 270.0);
        let (e, ew, eh) = super::super::exact::rotate_90_ccw(&buf, w, h);
        assert_eq!((rw, rh), (ew, eh));
        assert_eq!(r, e);
    }

    /// The `#ifdef CLEANUP` phase is really wired: for a crafted neighbourhood
    /// that enters the shallow 2:1-slant branch with `flip = false` and
    /// `similar(c, uf)`, some sub-pixel position must slice differently with
    /// cleanup on vs off. (The aggregate fixtures barely trigger it, matching
    /// the reference's "negligible effect for rotation" note.)
    #[test]
    fn cleanup_phase_is_active() {
        let t = [0u8, 0, 0, 0];
        let blue = [30u8, 60, 220, 255];
        let red = [255u8, 0, 0, 255];
        let green = [60u8, 180, 90, 255];

        let ub = nb(t, 0);
        let u = nb(t, 0);
        let uf = nb(blue, 0);
        let uff = nb(blue, 0);
        let b = nb(green, 0);
        let c = nb(blue, 0);
        let f = nb(red, 0);
        let ff = nb(blue, 0);
        let db = nb(red, 0);
        let d = nb(red, 0);
        let df = nb(blue, 0);
        let dff = nb(t, 0);
        let ddb = nb(t, 0);
        let dd = nb(t, 0);
        let ddf = nb(t, 0);

        let mut differs = false;
        'search: for lx in 0..16 {
            for ly in 0..16 {
                let point = [(lx as f64 + 0.5) / 16.0, (ly as f64 + 0.5) / 16.0];
                for pdx in [-1isize, 1] {
                    for pdy in [-1isize, 1] {
                        let point_dir = [pdx as f64, pdy as f64];
                        let off = slice_dist(
                            point,
                            [1.0, 1.0],
                            point_dir,
                            ub,
                            u,
                            uf,
                            uff,
                            b,
                            c,
                            f,
                            ff,
                            db,
                            d,
                            df,
                            dff,
                            ddb,
                            dd,
                            ddf,
                            None,
                            false,
                        );
                        let on = slice_dist(
                            point,
                            [1.0, 1.0],
                            point_dir,
                            ub,
                            u,
                            uf,
                            uff,
                            b,
                            c,
                            f,
                            ff,
                            db,
                            d,
                            df,
                            dff,
                            ddb,
                            dd,
                            ddf,
                            None,
                            true,
                        );
                        if off != on {
                            differs = true;
                            break 'search;
                        }
                    }
                }
            }
        }
        assert!(
            differs,
            "CLEANUP must change slice_dist for some crafted neighbourhood"
        );
    }

    // -- The comparison experiment ------------------------------------------

    /// The probe angles: the task's fixed set plus the claimed 2:1 slopes
    /// (`atan2(1,2)` and `atan2(2,1)`).
    pub(crate) fn comparison_angles() -> Vec<f32> {
        vec![
            8.0,
            15.0,
            30.0,
            37.0,
            45.0,
            53.0,
            60.0,
            77.0,
            (1.0f32).atan2(2.0f32).to_degrees(), // ≈ 26.565° (shallow 2:1)
            (2.0f32).atan2(1.0f32).to_degrees(), // ≈ 63.435° (steep 2:1)
        ]
    }

    /// Prints the full fixture×angle × three-method comparison table and
    /// summary.
    ///
    /// BEFORE (1× single centre sample, cleanup off) vs AFTER (4×16
    /// supersample + cleanup) CleanEdge, on the same fixtures/angles/metrics.
    ///
    /// Run with `cargo test comparison_before_after -- --nocapture`.
    #[test]
    fn comparison_before_after() {
        let before = |b: &[u8], w: usize, h: usize, a: f32| {
            clean_edge_rotate_generic_opts(b, w, h, a, 1, false)
        };
        let after = clean_edge_rotate;

        println!(
            "\n{:<9} {:>7} | {:<21} | {:<21}",
            "fixture", "angle", "CleanEdge BEFORE (1x)", "CleanEdge AFTER (4x+CLEANUP)"
        );
        println!(
            "{:<9} {:>7} | {:>4} {:>7} {:>5} {:>3} | {:>4} {:>7} {:>5} {:>3}",
            "", "", "brk", "op/c", "dith", "pal", "brk", "op/c", "dith", "pal"
        );
        println!("{}", "-".repeat(80));

        let mut before_brk = 0usize;
        let mut after_brk = 0usize;
        let mut before_detail = 0usize;
        let mut after_detail = 0usize;
        let mut before_dith = 0.0f64;
        let mut after_dith = 0.0f64;
        let mut rows = 0usize;
        for (name, src, w, h) in all_fixtures() {
            let (src_op, src_cmp) = detail_stats(&src, w, h);
            let is_checker = name == "checker";
            for angle in comparison_angles() {
                rows += 1;
                let (bo, bw, bh) = before(&src, w, h, angle);
                let (ao, aw, ah) = after(&src, w, h, angle);
                assert_eq!((bw, bh), (aw, ah), "{name} @ {angle}° dims");

                let (b_op, b_brk) = line_integrity(&bo, bw, bh);
                let (a_op, a_brk) = line_integrity(&ao, aw, ah);
                let (_, b_cmp) = detail_stats(&bo, bw, bh);
                let (_, a_cmp) = detail_stats(&ao, aw, ah);
                before_brk += b_brk;
                after_brk += a_brk;
                before_detail += ((b_op, b_cmp) == (src_op, src_cmp)) as usize;
                after_detail += ((a_op, a_cmp) == (src_op, src_cmp)) as usize;
                if is_checker {
                    before_dith += dither_degradation(&bo, bw, bh);
                    after_dith += dither_degradation(&ao, aw, ah);
                }

                let bd = if is_checker {
                    format!("{:.3}", dither_degradation(&bo, bw, bh))
                } else {
                    "-".to_string()
                };
                let ad = if is_checker {
                    format!("{:.3}", dither_degradation(&ao, aw, ah))
                } else {
                    "-".to_string()
                };
                println!(
                    "{:<9} {:>7.3} | {:>4} {:>7} {:>5} {:>3} | {:>4} {:>7} {:>5} {:>3}",
                    name,
                    angle,
                    b_brk,
                    format!("{b_op}/{b_cmp}"),
                    bd,
                    if palette_pure(&src, &bo) { "Y" } else { "N" },
                    a_brk,
                    format!("{a_op}/{a_cmp}"),
                    ad,
                    if palette_pure(&src, &ao) { "Y" } else { "N" },
                );
            }
            println!(
                "{:<9} {:>7} | source opaque/components = {src_op}/{src_cmp}",
                name, "src"
            );
        }
        println!(
            "\nBEFORE broken total={before_brk}  AFTER broken total={after_brk}  (rows={rows}, lower better)"
        );
        println!("detail-exact rows: BEFORE={before_detail}  AFTER={after_detail} (higher better)");
        println!(
            "checker dither degradation: BEFORE={before_dith:.3}  AFTER={after_dith:.3} (lower better)"
        );

        // Thin-structure preservation: the 4× supersample + cleanup must
        // strictly reduce aggregate broken pixels (it does: 447 -> 123).
        assert!(
            after_brk < before_brk,
            "4× supersample must reduce aggregate broken pixels: {before_brk} -> {after_brk}"
        );
        // Dithering preservation: the plurality vote slightly smooths high-
        // frequency dither, so allow a small, documented tolerance rather than
        // an exact non-regression.
        assert!(
            after_dith <= before_dith + 0.25,
            "dither degradation regressed materially: {before_dith:.3} -> {after_dith:.3}"
        );
    }

    /// **Three-way** comparison on the same fixtures/angles/metrics:
    /// RotSprite vs CleanEdge (4× supersample + CLEANUP) vs Rotxel.
    ///
    /// Methods: `super::rotate` (the RotSprite path), `clean_edge_rotate`
    /// (production 4×+cleanup) and [`super::super::rotxel::rotxel_rotate`].
    ///
    /// Run with `cargo test comparison_three_way -- --nocapture`.
    #[test]
    fn comparison_three_way() {
        let baseline = super::super::rotate;
        let pal = |ok: bool| if ok { "Y" } else { "N" };

        println!(
            "\n{:<9} {:>7} | {:<21} | {:<21} | {:<21}",
            "fixture", "angle", "RotSprite", "cleanEdge(4x+CLN)", "Rotxel"
        );
        println!(
            "{:<9} {:>7} | {:>4} {:>7} {:>5} {:>3} | {:>4} {:>7} {:>5} {:>3} | {:>4} {:>7} {:>5} {:>3}",
            "", "", "brk", "op/c", "dith", "pal", "brk", "op/c", "dith", "pal", "brk", "op/c", "dith",
            "pal"
        );
        println!("{}", "-".repeat(101));

        // broken: strictly-smallest wins; anything else (including 2-way ties) is a tie.
        let mut broken_wins = [0usize; 3];
        let mut broken_ties = 0usize;
        // dither (checkerboard rows only): strictly-smallest wins.
        let mut dither_wins = [0usize; 3];
        let mut dither_ties = 0usize;
        // detail: rows where a method exactly conserves the source's
        // opaque count AND 4-connected component count.
        let mut detail_exact = [0usize; 3];
        // palette purity counts.
        let mut pal_ok = [0usize; 3];
        let mut rows = 0usize;

        for (name, src, w, h) in all_fixtures() {
            let (src_op, src_cmp) = detail_stats(&src, w, h);
            let is_checker = name == "checker";
            for angle in comparison_angles() {
                rows += 1;
                let (bout, bw, bh) = baseline(&src, w, h, angle);
                let (cout, cw, ch) = clean_edge_rotate(&src, w, h, angle);
                let (rout, rw, rh) = super::super::rotxel::rotxel_rotate(&src, w, h, angle);
                assert_eq!(
                    (bw, bh),
                    (cw, ch),
                    "baseline/clean_edge dims at {name} @ {angle}°"
                );
                assert_eq!(
                    (bw, bh),
                    (rw, rh),
                    "baseline/rotxel dims at {name} @ {angle}°"
                );

                let outs: [(&[u8], usize, usize); 3] =
                    [(&bout, bw, bh), (&cout, cw, ch), (&rout, rw, rh)];

                let mut brk = [0usize; 3];
                let mut detail = [(0usize, 0usize); 3];
                let mut dith = [0.0f64; 3];
                let mut pure = [false; 3];
                for (m, &(o, ow, oh)) in outs.iter().enumerate() {
                    let (op, b) = line_integrity(o, ow, oh);
                    debug_assert_eq!(op, detail_stats(o, ow, oh).0);
                    brk[m] = b;
                    detail[m] = detail_stats(o, ow, oh);
                    dith[m] = if is_checker {
                        dither_degradation(o, ow, oh)
                    } else {
                        f64::NAN
                    };
                    pure[m] = palette_pure(&src, o);
                    pal_ok[m] += pure[m] as usize;
                    if detail[m] == (src_op, src_cmp) {
                        detail_exact[m] += 1;
                    }
                }

                // Broken-pixel winner (unique minimum).
                let min_brk = brk.iter().copied().min().unwrap();
                let n_min = brk.iter().filter(|&&x| x == min_brk).count();
                if n_min == 1 {
                    broken_wins[brk.iter().position(|&x| x == min_brk).unwrap()] += 1;
                } else {
                    broken_ties += 1;
                }

                // Dither winner (unique minimum, checkerboard only).
                if is_checker {
                    let min_d = dith.iter().copied().fold(f64::INFINITY, f64::min);
                    let n = dith.iter().filter(|&&x| x == min_d).count();
                    if n == 1 {
                        dither_wins[dith.iter().position(|&x| x == min_d).unwrap()] += 1;
                    } else {
                        dither_ties += 1;
                    }
                }

                let d0 = if is_checker {
                    format!("{:.3}", dith[0])
                } else {
                    "-".to_string()
                };
                let d1 = if is_checker {
                    format!("{:.3}", dith[1])
                } else {
                    "-".to_string()
                };
                let d2 = if is_checker {
                    format!("{:.3}", dith[2])
                } else {
                    "-".to_string()
                };

                println!(
                    "{:<9} {:>7.3} | {:>4} {:>7} {:>5} {:>3} | {:>4} {:>7} {:>5} {:>3} | {:>4} {:>7} {:>5} {:>3}",
                    name,
                    angle,
                    brk[0],
                    format!("{}/{}", detail[0].0, detail[0].1),
                    d0,
                    pal(pure[0]),
                    brk[1],
                    format!("{}/{}", detail[1].0, detail[1].1),
                    d1,
                    pal(pure[1]),
                    brk[2],
                    format!("{}/{}", detail[2].0, detail[2].1),
                    d2,
                    pal(pure[2]),
                );
            }
            println!(
                "{:<9} {:>7} | source opaque/components = {src_op}/{src_cmp}",
                name, "src"
            );
        }

        println!(
            "\nbroken-pixel wins (unique min, lower=better): RotSprite={} cleanEdge={} Rotxel={} ties={} (rows={rows})",
            broken_wins[0], broken_wins[1], broken_wins[2], broken_ties
        );
        println!(
            "dither wins  (checkerboard only, unique min): RotSprite={} cleanEdge={} Rotxel={} ties={}",
            dither_wins[0], dither_wins[1], dither_wins[2], dither_ties
        );
        println!(
            "detail exact (opaque&components == source): RotSprite={} cleanEdge={} Rotxel={}",
            detail_exact[0], detail_exact[1], detail_exact[2]
        );
        println!(
            "palette purity: RotSprite {}/{} cleanEdge {}/{} Rotxel {}/{}",
            pal_ok[0], rows, pal_ok[1], rows, pal_ok[2], rows
        );
    }

    /// CPU-time and memory comparison of all three methods on a larger
    /// fixture. Timing is printed, never asserted (inherently non-deterministic).
    #[test]
    fn comparison_timing_and_memory() {
        let (w, h) = (32usize, 32usize);
        let mut buf = blank(w, h);
        for x in 4..28 {
            set(&mut buf, w, x, 16, RED);
        }
        let angle = 30.0f32;
        let runs = 3u32;

        let rotxel = super::super::rotxel::rotxel_rotate;
        let before = |b: &[u8], w: usize, h: usize, a: f32| {
            clean_edge_rotate_generic_opts(b, w, h, a, 1, false)
        };
        let _ = super::super::rotate(&buf, w, h, angle);
        let _ = clean_edge_rotate(&buf, w, h, angle);
        let _ = before(&buf, w, h, angle);
        let _ = rotxel(&buf, w, h, angle);

        let time = |f: fn(&[u8], usize, usize, f32) -> (Vec<u8>, usize, usize)| {
            let t = Instant::now();
            for _ in 0..runs {
                let _ = f(&buf, w, h, angle);
            }
            t.elapsed() / runs
        };
        let base_avg = time(super::super::rotate);
        let ce_after_avg = time(clean_edge_rotate);
        let rx_avg = time(rotxel);
        let t = Instant::now();
        for _ in 0..runs {
            let _ = before(&buf, w, h, angle);
        }
        let ce_before_avg = t.elapsed() / runs;

        let (bout, bw, bh) = super::super::rotate(&buf, w, h, angle);
        let (cout, cw, ch) = clean_edge_rotate(&buf, w, h, angle);
        let (rout, rw, rh) = rotxel(&buf, w, h, angle);

        let n = angle.rem_euclid(90.0);
        let d = n.min(90.0 - n);
        let scale = if d >= 22.5 { 16usize } else { 8 };
        let upscale_bytes = w * scale * h * scale * 4;
        let rot_dims = super::super::rotated_bounds(w, h, angle);
        let rot_bytes = (rot_dims.0 * scale + scale) * (rot_dims.1 * scale + scale) * 4;

        println!("\n[timing] 32×32 line @ {angle}°, avg of {runs} runs (indicative only)");
        println!("  RotSprite (baseline)             : {base_avg:?}");
        println!("  CleanEdge BEFORE (1×, no CLEANUP): {ce_before_avg:?}");
        println!(
            "  CleanEdge AFTER  (4×{} + CLEANUP): {ce_after_avg:?}  ({} fragments/output px)",
            SUPERSAMPLE,
            SUPERSAMPLE * SUPERSAMPLE
        );
        println!("  rotxel                           : {rx_avg:?}");
        println!(
            "[memory] output bytes: RotSprite {} ({bw}×{bh}), CleanEdge {} ({cw}×{ch}), Rotxel {} ({rw}×{rh})",
            bout.len(),
            cout.len(),
            rout.len()
        );
        println!(
            "  RotSprite largest intermediate ≈ S×EPX upscale {} B + S×-rotated {} B = {} B (S={scale})",
            upscale_bytes,
            rot_bytes,
            upscale_bytes + rot_bytes
        );
        println!(
            "  CleanEdge largest intermediate = none beyond source ({}) + output ({}) + per-pixel stack tally ({} colours)",
            buf.len(),
            cout.len(),
            SUPERSAMPLE * SUPERSAMPLE
        );
        println!(
            "  Rotxel largest intermediate    = none beyond source ({}) + output ({})",
            buf.len(),
            rout.len()
        );
    }
}
