//! Sprite transforms — nearest-neighbour free-angle rotation (a bare NN
//! reverse-map, NOT RotSprite anti-jaggy repair), exact 90° paths, axis flips,
//! and nearest-neighbour scaling.
//!
//! All functions operate on raw RGBA8 buffers (`[r, g, b, a]` per pixel,
//! row-major, `w * h * 4` bytes) and are pure: no I/O, no randomness, no
//! wall-clock, no UI dependencies. The same input always produces the same
//! output bytes.
//!
//! # Rotation model (D34 / D33 / D55)
//!
//! Free 1°-precise rotation is the primary path (Aseprite-style, arbitrary
//! angle). The 22.5° snap applied to Shift-held gestures is a **UI-layer**
//! concern (amendment 2026-09-12 to D33/D55) — this module is
//! angle-agnostic and accepts any `f32` angle. Exact 90°/180°/270°
//! rotations and H/V flips are provided as secondary pixel-exact paths;
//! [`rotate`] and [`rotate_with`] dispatch exact multiples of 90° to them so
//! the results are bit-identical for every [`TransformAlgorithm`].
//!
//! # Guarantees
//!
//! - **Palette purity**: output colors are a subset of input colors — no
//!   interpolation, no blending, no new colors. Alpha is only ever copied
//!   verbatim, so an input with alpha ∈ {0, 255} yields output alpha ∈
//!   {0, 255} (no anti-aliasing / partial coverage).
//! - **Determinism**: same input + same angle → identical bytes every call.
//! - **No panics**: empty or undersized buffers return empty results, never
//!   panic.

pub mod clean_edge;
mod curve;
mod exact;
mod object;
mod rot_sprite;
pub mod rotxel;
mod scale;

pub use curve::{
    CurveTransform, CONTROL_POINTS, CURVE_HIT_RADIUS, GIZMO_COUNT, GIZMO_HIT_RADIUS, SEGMENT_COUNT,
};
pub use exact::{flip_h, flip_v, rotate_180, rotate_90_ccw, rotate_90_cw};
pub use object::{
    LayerBuffer, LayerCommit, RenderedObject, SessionState, TransformObject, TransformSession,
};
pub use rot_sprite::{rotate, rotated_bounds};
pub use scale::scale_nn;

/// Selectable free-angle rotation algorithm.
///
/// This is the core-side contract consumed by the transform tool properties
/// (`TransformObject::algorithm` / `set_algorithm`). All variants share the
/// same exact 0/90/180/270° paths and the same output geometry; only the
/// generic-angle core differs. `Default` is [`TransformAlgorithm::RotSprite`],
/// so existing behaviour is preserved unless a caller opts in.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum TransformAlgorithm {
    /// Structure-preserving Scale2x/EPX RotSprite (S1–U1 core). Default.
    #[default]
    RotSprite,
    /// CleanEdge (torcado) edge-snapping rotation.
    CleanEdge,
    /// Rotxel (Azagaya / Pixelorama) subcell-EPX rotation.
    Rotxel,
}

/// Rotates an RGBA8 buffer by `angle_deg` degrees **clockwise** about its
/// centre (screen coords, y-down) using the selected [`TransformAlgorithm`].
///
/// Shared guards and exact-angle dispatch run FIRST, identically for every
/// algorithm:
///
/// - empty/undersized buffer (`checked_dims`) or a non-finite angle
///   (`NaN`/`±inf`) → `(Vec::new(), 0, 0)`;
/// - normalized `0/90/180/270°` → the pixel-exact copy / transpose paths, so
///   **all three algorithms are byte-identical** at those angles.
///
/// Only generic angles reach the algorithm cores. In particular
/// `rotate_with(buf, w, h, angle, TransformAlgorithm::RotSprite)` is
/// byte-identical to [`rotate`] for every angle (default preserved).
pub fn rotate_with(
    buf: &[u8],
    w: usize,
    h: usize,
    angle_deg: f32,
    alg: TransformAlgorithm,
) -> (Vec<u8>, usize, usize) {
    let Some((w, h)) = checked_dims(buf, w, h) else {
        return (Vec::new(), 0, 0);
    };
    if !angle_deg.is_finite() {
        return (Vec::new(), 0, 0);
    }

    let normalized = angle_deg.rem_euclid(360.0);
    if normalized == 0.0 {
        return (buf[..w * h * BYTES_PER_PIXEL].to_vec(), w, h);
    }
    if normalized == 90.0 {
        return exact::rotate_90_cw(buf, w, h);
    }
    if normalized == 180.0 {
        return exact::rotate_180(buf, w, h);
    }
    if normalized == 270.0 {
        return exact::rotate_90_ccw(buf, w, h);
    }

    match alg {
        TransformAlgorithm::RotSprite => rot_sprite::rotate(buf, w, h, angle_deg),
        TransformAlgorithm::CleanEdge => clean_edge::clean_edge_rotate(buf, w, h, angle_deg),
        TransformAlgorithm::Rotxel => rotxel::rotxel_rotate(buf, w, h, angle_deg),
    }
}

/// Bytes per RGBA8 pixel.
const BYTES_PER_PIXEL: usize = 4;

/// Validates the RGBA8 buffer contract: `w`/`h` non-zero and `buf` holding
/// at least `w * h * 4` bytes. Returns `None` for empty or undersized
/// buffers; callers return an empty result instead of panicking.
fn checked_dims(buf: &[u8], w: usize, h: usize) -> Option<(usize, usize)> {
    if w == 0 || h == 0 {
        return None;
    }
    let needed = w.checked_mul(h)?.checked_mul(BYTES_PER_PIXEL)?;
    if buf.len() < needed {
        return None;
    }
    Some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ALGS: [TransformAlgorithm; 3] = [
        TransformAlgorithm::RotSprite,
        TransformAlgorithm::CleanEdge,
        TransformAlgorithm::Rotxel,
    ];

    #[test]
    fn transform_algorithm_default_is_rotsprite() {
        assert_eq!(TransformAlgorithm::default(), TransformAlgorithm::RotSprite);
    }

    /// `rotate_with(..., RotSprite)` must be byte-identical to [`rotate`] for
    /// every angle/fixture (the default path is preserved).
    #[test]
    fn rotate_with_rotsprite_equals_rotate() {
        for (name, buf, w, h) in clean_edge::tests::all_fixtures() {
            for angle in [0.0f32, 8.0, 30.0, 45.0, 90.0, 123.0, 180.0, 270.0, -40.0] {
                let got = rotate_with(&buf, w, h, angle, TransformAlgorithm::RotSprite);
                let want = rotate(&buf, w, h, angle);
                assert_eq!(got, want, "RotSprite mismatch for {name} @ {angle}°");
            }
        }
    }

    /// Exact 0/90/180/270° results (and their ±360k equivalents) are
    /// byte-identical across all three algorithms.
    #[test]
    fn rotate_with_exact_angles_identical_across_algorithms() {
        for (name, buf, w, h) in clean_edge::tests::all_fixtures() {
            for angle in [0.0f32, 360.0, 90.0, 450.0, 180.0, -180.0, 270.0, -90.0] {
                let (r, rw, rh) = rotate_with(&buf, w, h, angle, TransformAlgorithm::RotSprite);
                for alg in [TransformAlgorithm::CleanEdge, TransformAlgorithm::Rotxel] {
                    let (o, ow, oh) = rotate_with(&buf, w, h, angle, alg);
                    assert_eq!((rw, rh), (ow, oh), "{name} @ {angle}° dims ({alg:?})");
                    assert_eq!(r, o, "{name} @ {angle}° bytes ({alg:?})");
                }
            }
        }
    }

    /// All algorithms share the guards: non-finite angle / empty / undersized
    /// buffer → `(empty, 0, 0)`, never a panic.
    #[test]
    fn rotate_with_guards_are_shared() {
        let (buf, w, h) = clean_edge::tests::all_fixtures()
            .into_iter()
            .find(|(n, ..)| *n == "diag45")
            .map(|(_, b, w, h)| (b, w, h))
            .unwrap();
        for alg in ALL_ALGS {
            for angle in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                assert_eq!(
                    rotate_with(&buf, w, h, angle, alg),
                    (Vec::new(), 0, 0),
                    "non-finite angle {angle} with {alg:?}"
                );
            }
            assert_eq!(rotate_with(&[], 0, 0, 30.0, alg), (Vec::new(), 0, 0));
            assert_eq!(rotate_with(&[0u8; 8], 3, 3, 30.0, alg), (Vec::new(), 0, 0));
        }
    }

    /// A generic angle actually selects different cores: at least one pair of
    /// algorithms differs on a structured fixture.
    #[test]
    fn rotate_with_generic_angles_can_differ() {
        let (buf, w, h) = clean_edge::tests::all_fixtures()
            .into_iter()
            .find(|(n, ..)| *n == "sprite")
            .map(|(_, b, w, h)| (b, w, h))
            .unwrap();
        let outs: Vec<Vec<u8>> = ALL_ALGS
            .iter()
            .map(|&alg| rotate_with(&buf, w, h, 30.0, alg).0)
            .collect();
        assert!(
            outs.iter().any(|o| *o != outs[0]) || outs[1] != outs[2],
            "algorithm selection had no effect at 30°"
        );
    }
}
