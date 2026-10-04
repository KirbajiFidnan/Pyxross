//! Transform object — the in-canvas, layer-less floating selection
//! (FEATURES.md §3, DECISIONS.md D32/D35/D36).
//!
//! A [`TransformObject`] owns the lifted pixels as a **mini-stack**: one
//! [`LayerBuffer`] per source layer (D32). The single-layer MVP is the
//! default ([`TransformObject::lift`], D36); multi-layer readiness is
//! provided by [`TransformObject::from_layers`] (test-only). The object is
//! visually layer-less — the UI composites it above all layers.
//!
//! # Fixed transform pipeline
//!
//! Every layer is transformed in this exact order (documented contract for
//! U18/U19/U20):
//!
//! 1. **Flip** — `flip_h` then `flip_v` applied to the source buffer
//!    (in-place, dims unchanged).
//! 2. **Scale from the source origin** — [`super::scale_nn`] resamples the
//!    flipped buffer to `ceil(w · sx) × ceil(h · sy)` (per-axis factors
//!    [`TransformObject::scale_xy`]). Because scaling happens BEFORE the
//!    rotation, `sx`/`sy` are true LOCAL dimension factors: the scaled rect's
//!    own width/height, never a canvas-axis stretch.
//! 3. **Rotate around the pivot** — [`super::rotate_with`] rotates the scaled
//!    buffer about its own visual area centre (it dispatches exact multiples
//!    of 90° to the pixel-exact paths, so 90°/180°/270° are bit-identical to
//!    U16's exact functions); the result is then translated so the canvas-space
//!    [`TransformObject::pivot`] stays fixed. Angle convention matches
//!    [`super::rotate_with`]: positive = clockwise (screen coords, y-down). For
//!    exact 90° multiples the placement uses integer coefficient tables
//!    (no trig epsilon), so `dst` is exactly on the integer pixel grid.
//! 4. **Translate** — the final top-left canvas position is the `dst`
//!    reported by [`TransformObject::commit`].
//!
//! The resulting canvas mapping is `canvas(u) = pivot + R(θ)·S·(pos + u −
//! pivot)` — SCALE-THEN-ROTATE (`A = R·S`), so a non-uniform scale on a
//! rotated object stays a rectangle (no shear). At `theta == 0` and for
//! uniform scale this is bit-identical to the former rotate-then-scale order.
//!
//! # Session model (D35)
//!
//! One enter → commit/cancel session is **one undo step**: the per-layer
//! deltas returned by [`TransformSession::commit`] form a single UI-side
//! composite delta. [`TransformSession::cancel`] returns the byte-exact
//! original pixels so the UI can restore them without any inverse math.
//!
//! # Guarantees
//!
//! Pure and deterministic: no I/O, no randomness, no threads, no
//! `HashMap`, no panics. Same input → same output bytes. Palette-pure:
//! every output pixel is a verbatim copy of one source pixel (NN sampling
//! throughout), so alpha stays ∈ {0, 255} and no new colors are created.

use super::{
    checked_dims, flip_h, flip_v, rotate_with, rotated_bounds, scale_nn, TransformAlgorithm,
    BYTES_PER_PIXEL,
};

/// Cache key for [`TransformObject::placement`]: the exact bit patterns of
/// every field that affects the placement math (angle, per-axis scale, pos,
/// pivot). Flips do **not** affect placement or dims, so they are excluded.
/// Direct public-field mutation changes the bits and invalidates the cache
/// automatically.
#[derive(Clone, Copy, PartialEq)]
struct PlacementKey {
    angle: u32,
    sx: u32,
    sy: u32,
    px: u32,
    py: u32,
    vx: u32,
    vy: u32,
}

/// Single-slot memo for [`TransformObject::placement`] so repeated calls in
/// an identical state skip even the trig. `computations` counts actual math
/// evaluations (test-only observability).
struct PlacementCache {
    key: Option<PlacementKey>,
    dst: (f32, f32),
    computations: u64,
}

impl PlacementCache {
    fn new() -> Self {
        PlacementCache {
            key: None,
            dst: (0.0, 0.0),
            computations: 0,
        }
    }
}

/// One lifted layer of the mini-stack (D32): an RGBA8 buffer with alpha
/// ∈ {0, 255} (no anti-aliasing), `w * h * 4` bytes, row-major.
pub struct LayerBuffer {
    /// Source layer this buffer was lifted from (used to route the commit
    /// back to the correct layer).
    pub layer_id: usize,
    /// Buffer width in pixels.
    pub w: usize,
    /// Buffer height in pixels.
    pub h: usize,
    /// RGBA8 pixel data, `w * h * 4` bytes.
    pub buf: Vec<u8>,
}

/// The floating transform object: owns the lifted per-layer buffers plus
/// the live transform state (position, pivot, angle, scale, flips).
///
/// `pos` is the canvas position of the source buffer's top-left corner
/// (the identity placement). `pivot` is the canvas-space point that
/// rotation and scaling are anchored to (default: the source's visual AREA
/// centre, `pos + w/2`).
pub struct TransformObject {
    /// Mini-stack: one buffer per lifted source layer (D32).
    layers: Vec<LayerBuffer>,
    /// Canvas position of the source top-left (identity placement).
    pub pos: (f32, f32),
    /// Canvas-space rotation/scale anchor.
    pub pivot: (f32, f32),
    /// Rotation angle in degrees, positive = clockwise (U16 convention).
    pub angle_deg: f32,
    /// Uniform scale factor (always > 0.01). Only meaningful when `sx == sy`
    /// (e.g. after [`scale_by`](Self::scale_by)); use [`scale_xy`](Self::scale_xy)
    /// for the true per-axis factors. Kept for backward compatibility.
    pub scale: f32,
    /// Horizontal scale factor (always > 0.01).
    pub sx: f32,
    /// Vertical scale factor (always > 0.01).
    pub sy: f32,
    /// Horizontal mirror applied to the source before rotation.
    pub flip_h: bool,
    /// Vertical mirror applied to the source before rotation.
    pub flip_v: bool,
    /// Free-angle rotation algorithm used by the transform pipeline
    /// (`RotSprite` by default). Settable via [`set_algorithm`](Self::set_algorithm).
    alg: TransformAlgorithm,
    /// The `pos` captured at lift time; `restore()` reports this so pivot
    /// or pos changes never affect cancelled pixels (D35).
    orig_pos: (f32, f32),
    /// Single-slot memo for [`placement`](Self::placement): repeated calls
    /// in an identical transform state skip the placement math entirely.
    placement_cache: std::cell::RefCell<PlacementCache>,
}

impl TransformObject {
    /// Lifts a single layer into a new transform object (single-layer MVP,
    /// D36). The pivot defaults to the **visual AREA centre** of `src` in
    /// canvas space (`pos + (w/2, h/2)`; the area is `[pos, pos + w]`, e.g.
    /// 16×16 → `pos + (8, 8)`, 15×15 → `pos + (7.5, 7.5)`), scale is 1.0,
    /// angle is 0, no flips.
    pub fn lift(src: LayerBuffer, pos: (f32, f32)) -> Self {
        let pivot = (pos.0 + src.w as f32 / 2.0, pos.1 + src.h as f32 / 2.0);
        TransformObject {
            layers: vec![src],
            pos,
            pivot,
            angle_deg: 0.0,
            scale: 1.0,
            sx: 1.0,
            sy: 1.0,
            flip_h: false,
            flip_v: false,
            alg: TransformAlgorithm::default(),
            orig_pos: pos,
            placement_cache: std::cell::RefCell::new(PlacementCache::new()),
        }
    }

    /// Lifts multiple layers (multi-layer readiness, D32). The pivot
    /// defaults to the **visual AREA centre** of the first layer
    /// (`pos + (w/2, h/2)`); every layer shares the same `pos`. Test-only:
    /// the single-layer MVP is the shipped path.
    #[cfg(test)]
    pub fn from_layers(layers: Vec<LayerBuffer>, pos: (f32, f32)) -> Self {
        let pivot = match layers.first() {
            Some(l) => (pos.0 + l.w as f32 / 2.0, pos.1 + l.h as f32 / 2.0),
            None => pos,
        };
        TransformObject {
            layers,
            pos,
            pivot,
            angle_deg: 0.0,
            scale: 1.0,
            sx: 1.0,
            sy: 1.0,
            flip_h: false,
            flip_v: false,
            alg: TransformAlgorithm::default(),
            orig_pos: pos,
            placement_cache: std::cell::RefCell::new(PlacementCache::new()),
        }
    }

    /// Moves the rotation/scale anchor to a canvas-space point. The
    /// transform is recomputed around the new pivot on the next
    /// [`commit`](Self::commit)/[`render`](Self::render).
    pub fn set_pivot(&mut self, canvas_pos: (f32, f32)) {
        self.pivot = canvas_pos;
    }

    /// The canvas-space rotation/scale anchor (read access to the public
    /// [`pivot`](Self::pivot) field).
    pub fn pivot(&self) -> (f32, f32) {
        self.pivot
    }

    /// Display size of the transformed result after scale, as
    /// `rotated_bounds(ceil(w·sx), ceil(h·sy), θ)` — the local scaled dims
    /// rotated. Pure math — no pixel work.
    pub fn bounds_size(&self) -> (u32, u32) {
        let (sw, sh) = self.result_dims();
        (sw as u32, sh as u32)
    }

    /// Axis-aligned bounding box of the transformed result in canvas
    /// space: `(min_x, min_y, max_x, max_y)` with `max = min + size`.
    pub fn canvas_bbox(&self) -> (f32, f32, f32, f32) {
        let (sw, sh) = self.result_dims();
        let dst = self.placement();
        (dst.0, dst.1, dst.0 + sw as f32, dst.1 + sh as f32)
    }

    /// The four corners of the object's LOGICAL rect under the fixed pipeline
    /// (flip → scale → rotate about pivot), in canvas space, in CLOCKWISE
    /// order `[TL, TR, BR, BL]`.
    ///
    /// The logical rect is the first layer's source rect at `pos`,
    /// `[pos.0, pos.0 + w] × [pos.1, pos.1 + h]` (the `(w, h)` edge convention
    /// `transform_layer` uses; `flip` does not change dims so it is not
    /// included). Source corner `u` maps to `pivot + R(angle)·S·(pos + u −
    /// pivot)` — the SCALE-THEN-ROTATE mapping `placement_math`/`transform_layer`
    /// implement (`A = R·S`), with the exact-90°-multiple integer coefficient
    /// tables so a 90° step stays axis-aligned with no trig epsilon. A
    /// non-uniform scale on a rotated object therefore stays a rectangle: the
    /// quad never shears.
    ///
    /// Angle-0 convention: at `angle_deg == 0` with `sx == sy == 1` these are
    /// exactly the [`canvas_bbox`](Self::canvas_bbox) rect corners. For a
    /// scaled axis whose exact product is non-integer, `canvas_bbox` `ceil`s
    /// the result dims while these corners use the un-rounded product, so the
    /// quad can trail the AABB by `< 1 px` on that axis (documented tolerance).
    pub fn canvas_corners(&self) -> [(f32, f32); 4] {
        let (w, h) = self
            .layers
            .first()
            .map(|l| (l.w as f32, l.h as f32))
            .unwrap_or((0.0, 0.0));
        let pivot = self.pivot;
        let base = [
            (self.pos.0, self.pos.1),
            (self.pos.0 + w, self.pos.1),
            (self.pos.0 + w, self.pos.1 + h),
            (self.pos.0, self.pos.1 + h),
        ];
        base.map(|p| {
            // SCALE about the pivot (in source-local axes), then ROTATE about
            // the pivot: `pivot + R·S·(p − pivot)`.
            let s = (self.sx * (p.0 - pivot.0), self.sy * (p.1 - pivot.1));
            let r = rotate_vector(s, self.angle_deg);
            (pivot.0 + r.0, pivot.1 + r.1)
        })
    }

    /// Rotates the object by `angle_rad` radians **relative** to its
    /// current angle, **around the pivot** (positive = clockwise, matching
    /// the rotation convention). The caller is responsible for snapping
    /// (e.g. the UI's 22.5° Shift snap); this method stores the raw
    /// accumulated angle (kept internally in degrees).
    pub fn rotate_by(&mut self, angle_rad: f32) {
        self.angle_deg += angle_rad.to_degrees();
    }

    /// Sets the absolute rotation angle in degrees (positive = clockwise).
    pub fn set_angle(&mut self, deg: f32) {
        self.angle_deg = deg;
    }

    /// Scales the object uniformly by `factor` (multiplicative). The scale
    /// is clamped so it never drops below 0.01. Equivalent to
    /// [`resize(factor, factor)`](Self::resize): `scale`, `sx` and `sy` all
    /// stay in sync, so uniform gestures keep the legacy `scale` field
    /// meaningful.
    pub fn scale_by(&mut self, factor: f32) {
        self.scale = (self.scale * factor).max(0.01);
        self.sx = (self.sx * factor).max(0.01);
        self.sy = (self.sy * factor).max(0.01);
    }

    /// Resizes the object non-uniformly, nearest-neighbour, by `sx` along
    /// the x axis and `sy` along the y axis (absolute factors, not
    /// multiplicative). Both axes are clamped to `>= 0.01` (same clamp as
    /// [`scale_by`](Self::scale_by)). When `sx == sy` the legacy `scale`
    /// field is set to match; for non-uniform factors `scale` keeps its
    /// previous value (it is only meaningful when `sx == sy` — use
    /// [`scale_xy`](Self::scale_xy) for the true per-axis factors).
    pub fn resize(&mut self, sx: f32, sy: f32) {
        let sx = sx.max(0.01);
        let sy = sy.max(0.01);
        self.sx = sx;
        self.sy = sy;
        if sx == sy {
            self.scale = sx;
        }
    }

    /// The current per-axis scale factors `(sx, sy)`.
    pub fn scale_xy(&self) -> (f32, f32) {
        (self.sx, self.sy)
    }

    /// Public wrapper over the current rendered result size of the first
    /// lifted layer, in integer pixels (post-rotation AND post-scale).
    pub fn rotated_dims(&self) -> (usize, usize) {
        self.result_dims()
    }

    /// The first layer's SOURCE dims `(w, h)`, in pixels (before flip/scale/
    /// rotate). The local-frame resize table is stated in these coordinates.
    pub fn source_dims(&self) -> (usize, usize) {
        self.layers.first().map(|l| (l.w, l.h)).unwrap_or((0, 0))
    }

    /// The LOCAL scaled dims `(ceil(w·sx), ceil(h·sy))` — the size the local
    /// resize handles control, BEFORE rotation. This is the INVARIANT display
    /// size of the scaled buffer (`transform_layer`'s `sw × sh`), independent
    /// of the angle.
    pub fn local_scaled_dims(&self) -> (usize, usize) {
        match self.layers.first() {
            Some(layer) => (
                checked_scaled_dimension(layer.w, self.sx).unwrap_or(0),
                checked_scaled_dimension(layer.h, self.sy).unwrap_or(0),
            ),
            None => (0, 0),
        }
    }

    /// Resize to an EXACT **LOCAL** pixel target, NN, no flip. `anchor` is a
    /// canvas-space point that stays fixed (valid for any rotation). Both
    /// targets mean LOCAL dims (`ceil(w·sx) × ceil(h·sy)`) and are clamped to
    /// `>= 1`. Returns the applied `(sx, sy)`.
    ///
    /// The factor is derived as `target / source_dim` and nudged by a few
    /// ULPs so the `ceil` dims pipeline hits the target EXACTLY (preview and
    /// commit agree, no 1px drift). The pivot is left untouched; `pos` is
    /// adjusted by the correction `Δ = (S_new⁻¹ − S_old⁻¹)·Rᵀ·(anchor − pivot)`
    /// so `anchor` maps to itself under the new SCALE-THEN-ROTATE mapping.
    pub fn resize_to_pixels(
        &mut self,
        target_w: u32,
        target_h: u32,
        anchor: (f32, f32),
    ) -> (f32, f32) {
        let (w, h) = self.layers.first().map(|l| (l.w, l.h)).unwrap_or((1, 1));
        let sx_old = if self.sx.is_finite() && self.sx > 0.0 {
            self.sx
        } else {
            0.01
        };
        let sy_old = if self.sy.is_finite() && self.sy > 0.0 {
            self.sy
        } else {
            0.01
        };
        let sx_new = exact_scale(target_w, w);
        let sy_new = exact_scale(target_h, h);
        // canvas(u) = pivot + R·S·(pos + u − pivot); the pos Jacobian is R·S,
        // so the correction is (S_new⁻¹ − S_old⁻¹)·Rᵀ·(anchor − pivot).
        let q = (anchor.0 - self.pivot.0, anchor.1 - self.pivot.1);
        let rq = rotate_inverse(q, self.angle_deg);
        let d = (
            (1.0 / sx_new - 1.0 / sx_old) * rq.0,
            (1.0 / sy_new - 1.0 / sy_old) * rq.1,
        );
        self.pos = (self.pos.0 + d.0, self.pos.1 + d.1);
        self.sx = sx_new;
        self.sy = sy_new;
        if (sx_new - sy_new).abs() <= f32::EPSILON {
            self.scale = sx_new;
        }
        (self.sx, self.sy)
    }

    /// Resize to an EXACT **LOCAL** pixel target and place the LOCAL scaled
    /// rect's MIN relative to the pivot absolutely at `local_min`.
    ///
    /// `local_min` is expressed in the object's SCALED-LOCAL frame (the frame
    /// BEFORE the rotation, measured from the pivot in canvas units): the
    /// source rect's min corner is placed at `pivot + local_min` in that
    /// un-rotated frame, so after the rotation it maps to `pivot + R·local_min`.
    /// This is what a start-referenced local resize needs so a FLIP can move
    /// the object to the opposite side of the anchor without any accumulated
    /// state. The exact-target contract is preserved
    /// (`local_scaled_dims() == (target_local_w, target_local_h)`). Returns
    /// the applied `(sx, sy)`.
    pub fn resize_local_to_pixels_at_min(
        &mut self,
        target_local_w: u32,
        target_local_h: u32,
        local_min: (f32, f32),
    ) -> (f32, f32) {
        let (w, h) = self.layers.first().map(|l| (l.w, l.h)).unwrap_or((1, 1));
        let sx_new = exact_scale(target_local_w, w);
        let sy_new = exact_scale(target_local_h, h);
        self.sx = sx_new;
        self.sy = sy_new;
        if (sx_new - sy_new).abs() <= f32::EPSILON {
            self.scale = sx_new;
        }
        // `local_min` is `S·(pos − pivot)`; invert to recover `pos`.
        self.pos = (
            self.pivot.0 + local_min.0 / sx_new,
            self.pivot.1 + local_min.1 / sy_new,
        );
        (sx_new, sy_new)
    }

    /// Explicit min-size clamp: result is `>= 1x1` and both factors stay
    /// strictly positive (a pointer that crossed the anchor can never
    /// flip/vanish the object). Returns the applied `(sx, sy)`.
    pub fn clamp_min_size(&mut self) -> (f32, f32) {
        let (rw, rh) = self
            .layers
            .first()
            .map(|l| rotated_bounds(l.w, l.h, self.angle_deg))
            .unwrap_or((1, 1));
        let min_sx = 1.0 / rw.max(1) as f32;
        let min_sy = 1.0 / rh.max(1) as f32;
        self.sx = if self.sx.is_finite() && self.sx > 0.0 {
            self.sx.max(min_sx)
        } else {
            min_sx
        };
        self.sy = if self.sy.is_finite() && self.sy > 0.0 {
            self.sy.max(min_sy)
        } else {
            min_sy
        };
        if (self.sx - self.sy).abs() <= f32::EPSILON {
            self.scale = self.sx;
        }
        (self.sx, self.sy)
    }

    /// Sets the horizontal/vertical mirror flags applied to the source
    /// before rotation.
    pub fn set_flips(&mut self, h: bool, v: bool) {
        self.flip_h = h;
        self.flip_v = v;
    }

    /// The free-angle rotation algorithm currently selected for this object
    /// (default [`TransformAlgorithm::RotSprite`]).
    pub fn algorithm(&self) -> TransformAlgorithm {
        self.alg
    }

    /// Selects the free-angle rotation algorithm used by
    /// [`render`](Self::render)/[`commit`](Self::commit). Exact 0/90/180/270°
    /// results stay byte-identical for every algorithm; only generic angles
    /// differ. Placement, dims and palette guarantees are unchanged.
    pub fn set_algorithm(&mut self, alg: TransformAlgorithm) {
        self.alg = alg;
    }

    /// Renders the transformed layers.
    ///
    /// - `None` — **exact path** (commit path): full-resolution NN resample
    ///   ([`transform_layer`]; single-pass for generic angle + non-uniform
    ///   scale, otherwise the two-pass flip → scale → rotate), bit-identical
    ///   to [`commit`](Self::commit) (same buffers, dims, layer order).
    /// - `Some(max_dim)` — **preview path** (FEATURES.md §3.3): if the exact
    ///   result fits within `max_dim × max_dim` it is returned unchanged
    ///   (preview == commit for small selections); otherwise the transform —
    ///   including the rotation — runs at FULL resolution first and only the
    ///   RESULT is NN-downscaled to fit `max_dim`. Downscaling the source
    ///   before the transform would destroy 1px detail (and, for RotSprite,
    ///   the very structure the 8× upscale reconstructs) before the rotation
    ///   could preserve it, so the preview is derived from the exact commit
    ///   result. Preview buffers are display-only: the UI scales them to
    ///   [`canvas_bbox`](Self::canvas_bbox).
    pub fn render(&self, max_dim: Option<usize>) -> Vec<RenderedObject> {
        self.render_with_algorithm(max_dim, self.alg)
    }

    /// [`render`](Self::render) with an explicit **override** algorithm for the
    /// free-angle rotation, without mutating the object's stored selection.
    ///
    /// This is the two-tier preview hook (Y2-B): while a transform is being
    /// dragged the UI passes the FAST algorithm ([`TransformAlgorithm::Rotxel`])
    /// here, and once the drag ends it falls back to
    /// [`render`](Self::render)/the stored (user-selected) algorithm. Commit is
    /// unaffected — [`commit`](Self::commit) always uses the stored algorithm,
    /// so the preview==commit quality guarantee is restored at commit time.
    ///
    /// The override flows through the SAME [`transform_layer`] pipeline
    /// (placement, dims, flips and palette purity unchanged) and only changes
    /// which core [`rotate_with`] dispatches to at generic angles; exact
    /// 0/90/180/270° results stay byte-identical for every algorithm.
    pub fn render_with_algorithm(
        &self,
        max_dim: Option<usize>,
        alg: TransformAlgorithm,
    ) -> Vec<RenderedObject> {
        match max_dim {
            None => self
                .commit_with_algorithm(alg)
                .into_iter()
                .map(|c| RenderedObject {
                    layer_id: c.layer_id,
                    w: c.w,
                    h: c.h,
                    buf: c.buf,
                })
                .collect(),
            Some(max_dim) => {
                // Compute the exact result dims without pixel work; if
                // everything fits, the exact result IS the preview.
                let mut fits = true;
                for layer in &self.layers {
                    let (sw, sh) = self.result_dims_for(layer.w, layer.h);
                    if sw > max_dim || sh > max_dim {
                        fits = false;
                        break;
                    }
                }
                if fits {
                    return self
                        .commit_with_algorithm(alg)
                        .into_iter()
                        .map(|c| RenderedObject {
                            layer_id: c.layer_id,
                            w: c.w,
                            h: c.h,
                            buf: c.buf,
                        })
                        .collect();
                }
                // Oversize preview: run the SAME full-resolution transform the
                // commit uses, then NN-downscale only the RESULT to fit. The
                // RotSprite/fallback memory guard therefore sees the exact
                // same (w, h) as the commit, so preview and commit agree.
                let mut out = Vec::with_capacity(self.layers.len());
                for layer in &self.layers {
                    let (buf, rw, rh, _dst) = transform_layer(
                        &layer.buf,
                        layer.w,
                        layer.h,
                        self.pos,
                        self.pivot,
                        self.angle_deg,
                        (self.sx, self.sy),
                        self.flip_h,
                        self.flip_v,
                        alg,
                    );
                    if buf.is_empty() {
                        return Vec::new();
                    }
                    let (buf, rw, rh) = if rw > max_dim || rh > max_dim {
                        let m = rw.max(rh) as f32;
                        let Some(tw) = checked_scaled_dimension(rw, max_dim as f32 / m) else {
                            return Vec::new();
                        };
                        let Some(th) = checked_scaled_dimension(rh, max_dim as f32 / m) else {
                            return Vec::new();
                        };
                        let tw = tw.min(max_dim);
                        let th = th.min(max_dim);
                        (scale_nn(&buf, rw, rh, tw, th), tw, th)
                    } else {
                        (buf, rw, rh)
                    };
                    out.push(RenderedObject {
                        layer_id: layer.layer_id,
                        w: rw,
                        h: rh,
                        buf,
                    });
                }
                out
            }
        }
    }

    /// Final transformed pixels per layer — the exact resample, never the
    /// preview (FEATURES.md §3.3: the committed result is never the preview).
    /// Generic angles go through the RotSprite core (via [`rotate_with`]) for the
    /// two-pass path; the generic-angle + non-uniform-scale case uses the Q1
    /// single-pass nearest reverse-map ([`scale_rotate_nn`]).
    ///
    /// Each returned [`LayerCommit`] carries the transformed buffer, its
    /// canvas top-left `dst`, and the source `layer_id` so the UI can write
    /// each buffer back to its own layer (D32). The per-layer deltas form a
    /// **single UI-side composite undo step** (D35): one enter → commit
    /// session is one undo entry, regardless of how many layers it touches.
    pub fn commit(&self) -> Vec<LayerCommit> {
        self.commit_with_algorithm(self.alg)
    }

    /// [`commit`](Self::commit) with an explicit rotation algorithm. Private:
    /// the public commit path always uses the stored (user-selected)
    /// algorithm, so the two-tier preview can never change committed pixels.
    fn commit_with_algorithm(&self, alg: TransformAlgorithm) -> Vec<LayerCommit> {
        self.layers
            .iter()
            .map(|layer| {
                let (buf, w, h, dst) = transform_layer(
                    &layer.buf,
                    layer.w,
                    layer.h,
                    self.pos,
                    self.pivot,
                    self.angle_deg,
                    (self.sx, self.sy),
                    self.flip_h,
                    self.flip_v,
                    alg,
                );
                LayerCommit {
                    layer_id: layer.layer_id,
                    dst,
                    w,
                    h,
                    buf,
                }
            })
            .collect()
    }

    /// ORIGINAL pixels per layer, byte-exact — the cancel path (D35).
    ///
    /// Returns the buffers exactly as they were passed to
    /// [`lift`](Self::lift)/[`from_layers`](Self::from_layers) (dims,
    /// palette, channel order unchanged) with `dst` = the original `pos`
    /// captured at lift time. Pivot, pos, angle, scale and flip changes
    /// made during the session do **not** affect the restored pixels.
    pub fn restore(&self) -> Vec<LayerCommit> {
        self.layers
            .iter()
            .map(|layer| LayerCommit {
                layer_id: layer.layer_id,
                dst: self.orig_pos,
                w: layer.w,
                h: layer.h,
                buf: layer.buf.clone(),
            })
            .collect()
    }

    /// Result dims `rotated_bounds(ceil(w·sx), ceil(h·sy), θ)` for the first
    /// layer — used by [`bounds_size`](Self::bounds_size).
    fn result_dims(&self) -> (usize, usize) {
        match self.layers.first() {
            Some(layer) => self.result_dims_for(layer.w, layer.h),
            None => (0, 0),
        }
    }

    /// Result dims for a layer of `w × h`, mirroring the dims that
    /// [`transform_layer`] produces under SCALE-THEN-ROTATE: scale the source
    /// to `ceil(w·sx) × ceil(h·sy)` first, then rotate that rect
    /// ([`rotated_bounds`]). Pure math — no pixel work.
    fn result_dims_for(&self, w: usize, h: usize) -> (usize, usize) {
        let Some(sw) = checked_scaled_dimension(w, self.sx) else {
            return (0, 0);
        };
        let Some(sh) = checked_scaled_dimension(h, self.sy) else {
            return (0, 0);
        };
        if sw == 0 || sh == 0 {
            return (0, 0);
        }
        rotated_bounds(sw, sh, self.angle_deg)
    }

    /// Canvas top-left placement of the transformed result (the `dst` that
    /// [`commit`](Self::commit) reports for every layer). Memoised: repeating
    /// the call in an identical transform state reuses the cached result.
    fn placement(&self) -> (f32, f32) {
        let Some(layer) = self.layers.first() else {
            return self.pos;
        };
        let key = PlacementKey {
            angle: self.angle_deg.to_bits(),
            sx: self.sx.to_bits(),
            sy: self.sy.to_bits(),
            px: self.pos.0.to_bits(),
            py: self.pos.1.to_bits(),
            vx: self.pivot.0.to_bits(),
            vy: self.pivot.1.to_bits(),
        };
        let mut cache = self.placement_cache.borrow_mut();
        if cache.key != Some(key) {
            cache.dst = placement_math(
                layer.w,
                layer.h,
                self.pos,
                self.pivot,
                self.angle_deg,
                self.sx,
                self.sy,
            );
            cache.key = Some(key);
            cache.computations += 1;
        }
        cache.dst
    }

    /// Test-only observability: how many times the placement math actually
    /// ran (cache misses). Used to prove the memo short-circuits repeats.
    #[cfg(test)]
    pub(crate) fn placement_computations(&self) -> u64 {
        self.placement_cache.borrow().computations
    }
}

/// Rotate a canvas-space vector about the origin by `angle_deg` (CW, y-down:
/// `R·v = (c·v.x − s·v.y, s·v.x + c·v.y)`). Mirrors [`placement_math`]'s
/// rotation exactly, including its exact-90°-multiple integer coefficient
/// tables, so [`TransformObject::canvas_corners`] stays consistent with the
/// placement math even for the axis-aligned quarter turns.
fn rotate_vector(v: (f32, f32), angle_deg: f32) -> (f32, f32) {
    match angle_deg.rem_euclid(360.0) {
        0.0 => v,
        90.0 => (-v.1, v.0),
        180.0 => (-v.0, -v.1),
        270.0 => (v.1, -v.0),
        _ => {
            let theta = (angle_deg as f64).to_radians();
            let (c, s) = (theta.cos() as f32, theta.sin() as f32);
            (c * v.0 - s * v.1, s * v.0 + c * v.1)
        }
    }
}

/// Rotate a canvas-space vector about the origin by `-angle_deg` (i.e. apply
/// `Rᵀ`). Mirrors [`rotate_vector`] with the sign flipped, including the
/// exact-90°-multiple integer coefficient tables.
fn rotate_inverse(v: (f32, f32), angle_deg: f32) -> (f32, f32) {
    match angle_deg.rem_euclid(360.0) {
        0.0 => v,
        90.0 => (v.1, -v.0),
        180.0 => (-v.0, -v.1),
        270.0 => (-v.1, v.0),
        _ => {
            let theta = (angle_deg as f64).to_radians();
            let (c, s) = (theta.cos() as f32, theta.sin() as f32);
            (c * v.0 + s * v.1, -s * v.0 + c * v.1)
        }
    }
}

/// Pure placement math (NO pixel work): canvas top-left of the transformed
/// result. Mirrors [`transform_layer`]'s placement lines exactly, including
/// the exact-90°-multiple integer coefficient tables (so `dst` stays on the
/// integer pixel grid) and the f64 trig path for all other angles.
///
/// SCALE-THEN-ROTATE: the flipped source is scaled to `ceil(w·sx) ×
/// ceil(h·sy)`, then rotated about the scaled buffer's visual AREA centre
/// (`sw/2`, `sh/2`); `dst` places the rotated buffer so the pivot stays fixed.
/// The default pivot (`lift`) is the source's area centre, so a pure rotation
/// (`sx == sy == 1`) keeps the pivot on the output's area centre and each
/// pixel area maps symmetrically about it.
fn placement_math(
    w: usize,
    h: usize,
    pos: (f32, f32),
    pivot: (f32, f32),
    angle_deg: f32,
    sx: f32,
    sy: f32,
) -> (f32, f32) {
    let Some(sw) = checked_scaled_dimension(w, sx) else {
        return pos;
    };
    let Some(sh) = checked_scaled_dimension(h, sy) else {
        return pos;
    };
    if sw == 0 || sh == 0 {
        return pos;
    }
    let (rw, rh) = rotated_bounds(sw, sh, angle_deg);
    // Pivot in scaled-local coords (the scaled buffer sits at `pos`, scaled
    // about its origin), relative to the scaled buffer's AREA centre. The
    // visual area is `[pos, pos + sw]`, so its centre is `sw / 2` (NOT the
    // pixel-coordinate centre `(sw-1)/2`). Using the area centre keeps the
    // pivot on the visual centre: 16×16 → 8, 15×15 → 7.5.
    let pf = (pivot.0 - pos.0, pivot.1 - pos.1);
    let cf = (sw as f32 / 2.0, sh as f32 / 2.0);
    let cr = (rw as f32 / 2.0, rh as f32 / 2.0);
    let rel = (pf.0 * sx - cf.0, pf.1 * sy - cf.1);
    let r = rotate_vector(rel, angle_deg);
    let pr = (cr.0 + r.0, cr.1 + r.1);
    (pivot.0 - pr.0, pivot.1 - pr.1)
}

/// Derives a per-axis factor such that scaling `base` by it `ceil`s to
/// exactly `target` under **both** the f32 product used by the transform
/// pipeline and the f64 product used by [`checked_scaled_dimension`]. The
/// ideal factor `target / base` is nudged down by up to a few ULPs when
/// rounding would otherwise overshoot, so preview and commit agree without
/// a 1px drift.
fn exact_scale(target: u32, base: usize) -> f32 {
    let base_f = base.max(1) as f32;
    let t = target.max(1);
    let mut s = t as f32 / base_f;
    if !s.is_finite() || s <= 0.0 {
        return 1.0 / base_f;
    }
    let ok = |s: f32| {
        (base_f * s).ceil() as u32 == t && ((base as f64) * (s as f64)).ceil() as u64 == t as u64
    };
    let mut guard = 0;
    while !ok(s) && guard < 8 {
        let next = f32::from_bits(s.to_bits() - 1);
        if !next.is_finite() || next <= 0.0 {
            return 1.0 / base_f;
        }
        s = next;
        guard += 1;
    }
    s
}

/// One rendered layer of the transform object (preview or exact).
pub struct RenderedObject {
    /// Source layer this buffer belongs to.
    pub layer_id: usize,
    /// Buffer width in pixels.
    pub w: usize,
    /// Buffer height in pixels.
    pub h: usize,
    /// RGBA8 pixel data, `w * h * 4` bytes.
    pub buf: Vec<u8>,
}

/// One layer's write-back delta: the transformed (or restored) pixels plus
/// the canvas top-left position to stamp them at.
#[derive(Debug, PartialEq)]
pub struct LayerCommit {
    /// Source layer to write back into.
    pub layer_id: usize,
    /// Canvas top-left position of the buffer.
    pub dst: (f32, f32),
    /// Buffer width in pixels.
    pub w: usize,
    /// Buffer height in pixels.
    pub h: usize,
    /// RGBA8 pixel data, `w * h * 4` bytes.
    pub buf: Vec<u8>,
}

/// Lifecycle state of a [`TransformSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No transform session in progress.
    Idle,
    /// A transform session is in progress (object is mutable).
    Active,
}

/// One enter → commit/cancel transform session (D35). A session owns the
/// [`TransformObject`]; while [`SessionState::Active`] the object can be
/// manipulated, and committing or cancelling consumes the session and
/// returns the write-back deltas.
pub struct TransformSession {
    object: TransformObject,
    state: SessionState,
}

impl TransformSession {
    /// Starts a session: the object becomes [`SessionState::Active`] and
    /// can be manipulated until committed or cancelled.
    pub fn begin(object: TransformObject) -> Self {
        TransformSession {
            object,
            state: SessionState::Active,
        }
    }

    /// Current session state.
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// Immutable access to the underlying transform object.
    pub fn object(&self) -> &TransformObject {
        &self.object
    }

    /// Mutable access to the underlying transform object.
    ///
    /// Returns `None` unless the session is [`SessionState::Active`] —
    /// after commit/cancel the object is frozen and only readable.
    pub fn object_mut(&mut self) -> Option<&mut TransformObject> {
        if self.state == SessionState::Active {
            Some(&mut self.object)
        } else {
            None
        }
    }

    /// Commits the session: [`SessionState::Active`] → [`SessionState::Idle`].
    ///
    /// Returns the final transformed pixels per layer (the exact NN resample,
    /// not RotSprite) and the new state. The per-layer deltas form a **single
    /// UI-side composite undo step** (D35): one session = one undo entry.
    pub fn commit(self) -> (Vec<LayerCommit>, SessionState) {
        if self.state == SessionState::Active {
            (self.object.commit(), SessionState::Idle)
        } else {
            (Vec::new(), SessionState::Idle)
        }
    }

    /// Cancels the session: [`SessionState::Active`] → [`SessionState::Idle`].
    ///
    /// Returns the byte-exact original pixels per layer (cancel path, D35)
    /// and the new state, so the UI can restore the lifted pixels without
    /// any inverse transform math.
    pub fn cancel(self) -> (Vec<LayerCommit>, SessionState) {
        if self.state == SessionState::Active {
            (self.object.restore(), SessionState::Idle)
        } else {
            (Vec::new(), SessionState::Idle)
        }
    }
}

/// Applies the fixed transform pipeline (flip → scale → rotate about pivot →
/// translate) to one `w × h` RGBA8 buffer and returns `(buf, w, h, dst)`
/// where `dst` is the canvas top-left placement.
///
/// SCALE-THEN-ROTATE (`A = R·S`): the source is flipped, scaled to the LOCAL
/// dims `ceil(w·sx) × ceil(h·sy)`, and only then rotated about the scaled
/// buffer's centre; `dst` places the result so the canvas-space pivot stays
/// fixed. Because the scale is applied in the object's own local axes, a
/// non-uniform scale on a rotated object stays a rectangle (no shear).
///
/// **Resampling dispatch (Q1).** NN scale ([`scale_nn`], floor/left-edge) and
/// NN rotate ([`rotate_with`], centre/round) are two independent quantizers, so
/// chaining them is NOT the NN of the composed affine `A = R·S`. For the only
/// case where that double-resample loses structure — a GENERIC angle with a
/// NON-UNIFORM scale — the single-pass [`scale_rotate_nn`] reverse-maps the
/// composed affine and samples the SOURCE once. Every other case keeps the
/// existing two-pass path byte-for-byte:
/// - `θ ∈ {0, 90, 180, 270}` (any scale): the second pass is an exact
///   pixel-copy / transpose, so the composition is already single-resample;
/// - uniform scale `sx == sy` (any θ): a scalar commutes with `R`, so both
///   passes sample on the same grid.
///
/// The pipeline is documented on the module level; this is the single
/// implementation shared by [`TransformObject::commit`] and the preview
/// path of [`TransformObject::render`].
fn transform_layer(
    buf: &[u8],
    w: usize,
    h: usize,
    pos: (f32, f32),
    pivot: (f32, f32),
    angle_deg: f32,
    scale_xy: (f32, f32),
    mirror_h: bool,
    mirror_v: bool,
    alg: TransformAlgorithm,
) -> (Vec<u8>, usize, usize, (f32, f32)) {
    let (sx, sy) = scale_xy;
    let n = angle_deg.rem_euclid(360.0);
    let axis_aligned = n == 0.0 || n == 90.0 || n == 180.0 || n == 270.0;
    if !axis_aligned && sx != sy {
        // Generic angle + non-uniform scale: ONE NN quantizer over `A = R·S`.
        // This path is algorithm-independent (a bare single-pass affine NN, not
        // a feature-repair core); the selected algorithm applies to the
        // two-pass rotate path below.
        return scale_rotate_nn(buf, w, h, pos, pivot, angle_deg, sx, sy, mirror_h, mirror_v);
    }

    // -- Two-pass path (byte-identical to the pre-Q1 pipeline) --------------
    // 1. Flip on the source (flip_h then flip_v; dims unchanged).
    let (fbuf, fw, fh) = flip_source(buf, w, h, mirror_h, mirror_v);

    // 2. Scale the flipped buffer about its origin to the LOCAL dims
    //    `ceil(fw·sx) × ceil(fh·sy)`.
    let Some(sw) = checked_scaled_dimension(fw, sx) else {
        return (Vec::new(), 0, 0, pos);
    };
    let Some(sh) = checked_scaled_dimension(fh, sy) else {
        return (Vec::new(), 0, 0, pos);
    };
    let sbuf = scale_nn(&fbuf, fw, fh, sw, sh);
    if sbuf.is_empty() {
        return (Vec::new(), 0, 0, pos);
    }

    // 3. Rotate the scaled buffer about its centre (`rotate_with` dispatches
    //    exact 90° multiples to the pixel-exact paths for every algorithm),
    //    then translate so the canvas-space pivot stays fixed.
    let (rbuf, rw, rh) = rotate_with(&sbuf, sw, sh, angle_deg, alg);

    // Placement mirrors `placement_math` exactly (scale dims → rotate → place
    // so the pivot is fixed).
    let dst = placement_math(w, h, pos, pivot, angle_deg, sx, sy);
    (rbuf, rw, rh, dst)
}

/// Apply the mirror flags to a source buffer (`flip_h` then `flip_v`; dims
/// unchanged). Returns an owned copy, exactly as the two-pass pipeline's
/// first stage.
fn flip_source(
    buf: &[u8],
    w: usize,
    h: usize,
    mirror_h: bool,
    mirror_v: bool,
) -> (Vec<u8>, usize, usize) {
    if mirror_h {
        let (b, w2, h2) = flip_h(buf, w, h);
        if mirror_v {
            flip_v(&b, w2, h2)
        } else {
            (b, w2, h2)
        }
    } else if mirror_v {
        flip_v(buf, w, h)
    } else {
        (buf.to_vec(), w, h)
    }
}

/// Single-pass nearest-neighbour resample of the composed affine
/// `canvas(u) = pivot + R(θ)·S·(pos + u − pivot)`, sampling the SOURCE once
/// (no intermediate scaled buffer).
///
/// Output dims are `rotated_bounds(⌈w·sx⌉, ⌈h·sy⌉, θ)` and `dst` comes from
/// [`placement_math`] — the SAME sizes/placement as the two-pass path.
///
/// Reverse map: the rotated buffer's pixel `b` maps back through
/// `q = cf + Rᵀ·(b − cr)` (the scaled-local coordinate, `cf`/`cr` the scaled/
/// rotated buffer centres) and then `u = S⁻¹·q`. This is algebraically the
/// pivot form `u = (pivot − pos) + S⁻¹·Rᵀ·(canvas − pivot)`, but computing it
/// from the buffer-local offset reproduces [`rotate_with`]'s f64 sampling
/// (`cf + Rᵀ·(b − cr)`, round-half-away-from-zero) EXACTLY when `S = I`.
/// Flips are folded by mirroring the sampled source coordinate after the NN
/// round, exactly as flipping the source first would.
///
/// Out-of-bounds samples are transparent (all-zero RGBA); every in-bounds
/// output pixel is a verbatim source copy, so the result is palette-pure and
/// deterministic. `θ == 0` is a pure scale and delegates to [`scale_nn`] so the
/// degenerate case matches the canonical path byte-for-byte.
///
/// NOTE: this is a bare NN reverse-map, NOT RotSprite anti-jaggy repair — it
/// removes the double-resample error but does not synthesise pixels.
fn scale_rotate_nn(
    src: &[u8],
    w: usize,
    h: usize,
    pos: (f32, f32),
    pivot: (f32, f32),
    angle_deg: f32,
    sx: f32,
    sy: f32,
    flip_h: bool,
    flip_v: bool,
) -> (Vec<u8>, usize, usize, (f32, f32)) {
    let Some((w, h)) = checked_dims(src, w, h) else {
        return (Vec::new(), 0, 0, pos);
    };
    let Some(sw) = checked_scaled_dimension(w, sx) else {
        return (Vec::new(), 0, 0, pos);
    };
    let Some(sh) = checked_scaled_dimension(h, sy) else {
        return (Vec::new(), 0, 0, pos);
    };
    if sw == 0 || sh == 0 {
        return (Vec::new(), 0, 0, pos);
    }
    let (rw, rh) = rotated_bounds(sw, sh, angle_deg);
    if rw == 0 || rh == 0 {
        return (Vec::new(), 0, 0, pos);
    }
    let dst = placement_math(w, h, pos, pivot, angle_deg, sx, sy);

    // θ == 0 is a pure scale: delegate so the degenerate case matches the
    // two-pass result byte-for-byte.
    if angle_deg.rem_euclid(360.0) == 0.0 {
        let (fbuf, fw, fh) = flip_source(src, w, h, flip_h, flip_v);
        let out = scale_nn(&fbuf, fw, fh, sw, sh);
        if out.is_empty() {
            return (Vec::new(), 0, 0, pos);
        }
        return (out, sw, sh, dst);
    }

    let Some(out_len) = rw
        .checked_mul(rh)
        .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
    else {
        return (Vec::new(), 0, 0, pos);
    };
    let mut out = Vec::new();
    if out.try_reserve_exact(out_len).is_err() {
        return (Vec::new(), 0, 0, pos);
    }
    out.resize(out_len, 0);

    // Hoist the f64 trig and the reverse-map constants out of the loop.
    let theta = (angle_deg as f64).to_radians();
    let (cos_t, sin_t) = (theta.cos(), theta.sin());
    let cf_x = (sw as f64 - 1.0) / 2.0;
    let cf_y = (sh as f64 - 1.0) / 2.0;
    let cr_x = (rw as f64 - 1.0) / 2.0;
    let cr_y = (rh as f64 - 1.0) / 2.0;
    let inv_sx = 1.0 / sx as f64;
    let inv_sy = 1.0 / sy as f64;
    let wi = w as isize;
    let hi = h as isize;

    for by in 0..rh {
        let rel_y = by as f64 - cr_y;
        for bx in 0..rw {
            let rel_x = bx as f64 - cr_x;
            // Rᵀ·rel: undo the clockwise rotation.
            let rx = cos_t * rel_x + sin_t * rel_y;
            let ry = -sin_t * rel_x + cos_t * rel_y;
            // q = cf + Rᵀ·rel is the scaled-local coordinate; S⁻¹ → source.
            let ux = (cf_x + rx) * inv_sx;
            let uy = (cf_y + ry) * inv_sy;
            let mut nx = ux.round() as isize;
            let mut ny = uy.round() as isize;
            if flip_h {
                nx = wi - 1 - nx;
            }
            if flip_v {
                ny = hi - 1 - ny;
            }
            if 0 <= nx && nx < wi && 0 <= ny && ny < hi {
                let s = (ny as usize * w + nx as usize) * BYTES_PER_PIXEL;
                let d = (by * rw + bx) * BYTES_PER_PIXEL;
                out[d..d + BYTES_PER_PIXEL].copy_from_slice(&src[s..s + BYTES_PER_PIXEL]);
            }
        }
    }
    (out, rw, rh, dst)
}

fn checked_scaled_dimension(dimension: usize, factor: f32) -> Option<usize> {
    let scaled = dimension as f64 * factor as f64;
    if !scaled.is_finite() || scaled <= 0.0 || scaled.ceil() >= usize::MAX as f64 {
        return None;
    }
    Some(scaled.ceil() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::transform::{rotate_180, rotate_90_cw};
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

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

    fn layer(buf: Vec<u8>, w: usize, h: usize, layer_id: usize) -> LayerBuffer {
        LayerBuffer {
            layer_id,
            w,
            h,
            buf,
        }
    }

    fn px(buf: &[u8], w: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * w + x) * 4;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    /// Approximate f32 comparison (placement math carries trig epsilon).
    fn assert_close(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-4, "expected {b}, got {a}");
    }

    /// 4×4 fixture (opaque), for anchor tests.
    fn fixture_4x4() -> (Vec<u8>, usize, usize) {
        (vec![255u8; 4 * 4 * 4], 4, 4)
    }

    /// 4×4 fixture whose palette already CONTAINS transparent, so the
    /// transparent out-of-bounds fill of a rotation stays within the input
    /// palette (the palette-purity property then applies strictly).
    fn fixture_transparent_4x4() -> (Vec<u8>, usize, usize) {
        let palette: [[u8; 4]; 4] = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [0, 0, 0, 0],
        ];
        let mut buf = Vec::with_capacity(16 * 4);
        for i in 0..16 {
            buf.extend_from_slice(&palette[i % palette.len()]);
        }
        (buf, 4, 4)
    }

    /// `canvas(u) = pivot + R(θ)·S·(pos + u − pivot)` — the SCALE-THEN-ROTATE
    /// mapping the resize/anchor contract is stated against.
    fn canvas_point(obj: &TransformObject, u: (f32, f32)) -> (f32, f32) {
        let theta = obj.angle_deg.to_radians();
        let (c, s) = (theta.cos(), theta.sin());
        let v = (obj.pos.0 + u.0 - obj.pivot.0, obj.pos.1 + u.1 - obj.pivot.1);
        let scaled = (obj.sx * v.0, obj.sy * v.1);
        let r = (c * scaled.0 - s * scaled.1, s * scaled.0 + c * scaled.1);
        (obj.pivot.0 + r.0, obj.pivot.1 + r.1)
    }

    /// Inverse rotation (by `−angle_deg`) of a vector.
    fn rot_inv(p: (f32, f32), angle_deg: f32) -> (f32, f32) {
        let theta = angle_deg.to_radians();
        let (c, s) = (theta.cos(), theta.sin());
        (c * p.0 + s * p.1, -s * p.0 + c * p.1)
    }

    #[test]
    fn lift_defaults() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        assert_eq!(obj.pos, (10.0, 20.0));
        assert_eq!(obj.pivot, (11.5, 21.5)); // AREA centre of 3×3 at (10,20): 10+1.5
        assert_eq!(obj.angle_deg, 0.0);
        assert_eq!(obj.scale, 1.0);
        assert!(!obj.flip_h);
        assert!(!obj.flip_v);
    }

    /// The default pivot is the visual AREA centre `pos + w/2`, NOT the
    /// pixel-coordinate centre `pos + (w-1)/2`: 16×16 → 8, 15×15 → 7.5.
    #[test]
    fn lift_pivot_is_the_visual_area_centre() {
        let obj = TransformObject::lift(layer(vec![0u8; 16 * 16 * 4], 16, 16, 0), (0.0, 0.0));
        assert_eq!(obj.pivot(), (8.0, 8.0), "16×16 area centre is (8,8)");
        let obj = TransformObject::lift(layer(vec![0u8; 15 * 15 * 4], 15, 15, 0), (0.0, 0.0));
        assert_eq!(
            obj.pivot(),
            (7.5, 7.5),
            "15×15 area centre is (7.5,7.5), not (w-1)/2 = 7"
        );
        // The pivot tracks a non-zero origin and a non-square area.
        let (buf, w, h) = fixture_4x2();
        let obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        assert_eq!(obj.pivot(), (12.0, 21.0));
    }

    /// The stored pivot is exactly the AREA centre `min + size/2` of
    /// [`TransformObject::canvas_bbox`] (the visual gizmo centre) at identity
    /// and for every exact quarter turn, for even and odd dims alike.
    #[test]
    fn pivot_is_the_rendered_bbox_area_centre() {
        for (w, h) in [(16usize, 16usize), (15, 15), (4, 2), (3, 3), (5, 4)] {
            for angle in [0.0f32, 90.0, 180.0, 270.0] {
                let mut obj =
                    TransformObject::lift(layer(vec![0u8; w * h * 4], w, h, 0), (1.0, 2.0));
                obj.set_angle(angle);
                let (x0, y0, x1, y1) = obj.canvas_bbox();
                assert_close(obj.pivot().0, (x0 + x1) * 0.5);
                assert_close(obj.pivot().1, (y0 + y1) * 0.5);
            }
        }
    }

    /// 180° maps each pixel's AREA `[pos+i, pos+i+1]` onto the mirrored pixel
    /// area `w-1-i` about the area centre, so the whole source area
    /// `[pos, pos+w]` maps onto itself.
    #[test]
    fn rotate_180_maps_pixel_areas_symmetrically_about_the_area_centre() {
        let (w, h) = (5usize, 3usize);
        let mut obj = TransformObject::lift(layer(vec![0u8; w * h * 4], w, h, 0), (2.0, 7.0));
        obj.set_angle(180.0);
        assert_eq!(obj.pivot(), (4.5, 8.5));
        // The output occupies exactly the source area again (180° is an
        // involution of the area about its centre).
        let (x0, y0, x1, y1) = obj.canvas_bbox();
        assert_close(x0, 2.0);
        assert_close(x1, 2.0 + w as f32);
        assert_close(y0, 7.0);
        assert_close(y1, 7.0 + h as f32);
        // Pixel i's area maps onto pixel (w-1-i)'s area.
        for i in 0..w {
            let lead = canvas_point(&obj, (i as f32, 0.0));
            let trail = canvas_point(&obj, (i as f32 + 1.0, 0.0));
            let mirrored = (w - 1 - i) as f32;
            let lo = trail.0.min(lead.0);
            let hi = trail.0.max(lead.0);
            assert_close(lo, 2.0 + mirrored);
            assert_close(hi, 2.0 + mirrored + 1.0);
        }
    }

    /// Resize sizing about the top-left AREA corner keeps that anchor fixed and
    /// the exact local target under the area-centre convention.
    #[test]
    fn resize_anchor_is_fixed_under_the_area_centre_convention() {
        let (w, h) = (6usize, 4usize);
        let mut obj = TransformObject::lift(layer(vec![0u8; w * h * 4], w, h, 0), (1.0, 2.0));
        assert_eq!(obj.pivot(), (4.0, 4.0));
        let anchor = (1.0f32, 2.0f32);
        obj.resize_to_pixels(12, 8, anchor);
        assert_eq!(obj.local_scaled_dims(), (12, 8));
        // The source top-left area corner (u = 0,0) still maps to the anchor.
        let p = canvas_point(&obj, (0.0, 0.0));
        assert!(
            (p.0 - anchor.0).abs() < 1e-3 && (p.1 - anchor.1).abs() < 1e-3,
            "area corner anchor drifted: {p:?}"
        );
    }

    #[test]
    fn rotate_by_accumulates() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.rotate_by((10f32).to_radians());
        obj.rotate_by((20f32).to_radians());
        assert_eq!(obj.angle_deg, 30.0);
        obj.rotate_by((-45f32).to_radians());
        assert_eq!(obj.angle_deg, -15.0);
    }

    #[test]
    fn scale_by_clamps_minimum() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.scale_by(0.0001);
        assert_eq!(obj.scale, 0.01);
        obj.scale_by(2.0);
        assert_eq!(obj.scale, 0.02);
    }

    #[test]
    fn set_flips_sets_flags() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_flips(true, false);
        assert!(obj.flip_h);
        assert!(!obj.flip_v);
        obj.set_flips(false, true);
        assert!(!obj.flip_h);
        assert!(obj.flip_v);
    }

    #[test]
    fn set_angle_and_set_pivot() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_angle(45.0);
        assert_eq!(obj.angle_deg, 45.0);
        obj.set_pivot((5.0, 6.0));
        assert_eq!(obj.pivot, (5.0, 6.0));
    }

    #[test]
    fn bounds_size_identity_and_scale() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        assert_eq!(obj.bounds_size(), (3, 3));
        obj.scale_by(2.0);
        assert_eq!(obj.bounds_size(), (6, 6));
        obj.set_angle(90.0);
        // 3×3 rotated 90° is still 3×3; scaled 2× → 6×6.
        assert_eq!(obj.bounds_size(), (6, 6));
    }

    #[test]
    fn bounds_size_ceil_for_fractional_scale() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.scale_by(1.5);
        // ceil(3 * 1.5) = ceil(4.5) = 5.
        assert_eq!(obj.bounds_size(), (5, 5));
    }

    #[test]
    fn bounds_size_shrinks_below_one() {
        let (buf, w, h) = fixture_4x2();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.scale_by(0.5);
        // ceil(4 * 0.5) = 2, ceil(2 * 0.5) = 1.
        assert_eq!(obj.bounds_size(), (2, 1));
    }

    #[test]
    fn canvas_bbox_identity() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        assert_eq!(obj.canvas_bbox(), (10.0, 20.0, 13.0, 23.0));
    }

    #[test]
    fn canvas_bbox_after_rotation() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_angle(90.0);
        // 3×3 rotated 90° about its area centre (1.5,1.5): dst = (0,0), dims 3×3.
        assert_eq!(obj.canvas_bbox(), (0.0, 0.0, 3.0, 3.0));
    }

    #[test]
    fn canvas_bbox_non_center_pivot() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_pivot((0.0, 0.0));
        obj.set_angle(90.0);
        // Pivot at the top-left CORNER of the area: the area centre pivots a
        // 3×3 to (-3,0)..(0,3).
        let (min_x, min_y, max_x, max_y) = obj.canvas_bbox();
        assert_close(min_x, -3.0);
        assert_close(min_y, 0.0);
        assert_close(max_x, 0.0);
        assert_close(max_y, 3.0);
    }

    #[test]
    fn canvas_corners_identity_matches_bbox() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        // Angle 0, scale 1: the quad corners ARE the bbox corners exactly.
        assert_eq!(
            obj.canvas_corners(),
            [(10.0, 20.0), (13.0, 20.0), (13.0, 23.0), (10.0, 23.0)]
        );
        let (min_x, min_y, max_x, max_y) = obj.canvas_bbox();
        assert_eq!((min_x, min_y, max_x, max_y), (10.0, 20.0, 13.0, 23.0));
    }

    #[test]
    fn canvas_corners_after_90_rotation_are_the_axis_aligned_swap() {
        let (buf, w, h) = fixture_4x2();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        obj.set_angle(90.0);
        // 4×2 at (10,20), AREA centre pivot (12,21): the quarter turn swaps the
        // extents to [11,13]×[19,23] with no trig epsilon (odd/even areas are
        // symmetric about the area centre).
        assert_eq!(
            obj.canvas_corners(),
            [(13.0, 19.0), (13.0, 23.0), (11.0, 23.0), (11.0, 19.0)]
        );
        // The quad is axis-aligned: each corner is a corner of its own AABB.
        let corners = obj.canvas_corners();
        let min_x = corners.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
        let max_x = corners
            .iter()
            .map(|p| p.0)
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = corners.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
        let max_y = corners
            .iter()
            .map(|p| p.1)
            .fold(f32::NEG_INFINITY, f32::max);
        for (x, y) in corners {
            assert!(
                (x == min_x || x == max_x) && (y == min_y || y == max_y),
                "corner ({x},{y}) is not an AABB corner"
            );
        }
    }

    #[test]
    fn canvas_corners_match_the_canvas_point_mapping_when_rotated_and_scaled() {
        let (buf, w, h) = fixture_4x2();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        obj.set_angle(45.0);
        obj.resize(1.5, 2.0);
        let corners = obj.canvas_corners();
        // The contract `canvas(u) = pivot + R·S·(pos + u − pivot)` applies to
        // the four source-rect corners.
        let base = [
            (0.0, 0.0),
            (w as f32, 0.0),
            (w as f32, h as f32),
            (0.0, h as f32),
        ];
        for (i, u) in base.iter().enumerate() {
            let expect = canvas_point(&obj, *u);
            assert!(
                (corners[i].0 - expect.0).abs() < 1e-3 && (corners[i].1 - expect.1).abs() < 1e-3,
                "corner {i}: {:?} != {expect:?}",
                corners[i]
            );
        }
        // The quad really is rotated (its leading edge is neither axis-aligned).
        let edge = (corners[1].0 - corners[0].0, corners[1].1 - corners[0].1);
        assert!(
            edge.0.abs() > 1e-3 && edge.1.abs() > 1e-3,
            "expected a rotated quad, got edge {edge:?}"
        );
    }

    #[test]
    fn local_scaled_dims_matches_scale_and_is_angle_independent() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        assert_eq!(obj.local_scaled_dims(), (3, 3));
        obj.resize(2.0, 0.5);
        assert_eq!(obj.local_scaled_dims(), (6, 2));
        obj.set_angle(30.0);
        // Local dims are the pre-rotation size, so the angle cannot move them.
        assert_eq!(obj.local_scaled_dims(), (6, 2));
        // The rendered result is the local dims rotated.
        assert_eq!(obj.result_dims_for(3, 3), rotated_bounds(6, 2, 30.0));
    }

    #[test]
    fn canvas_corners_rectangular_after_rotated_nonuniform_scale() {
        // SCALE-THEN-ROTATE: a non-uniform scale on a rotated object stays a
        // rectangle — consecutive edges perpendicular, opposite edges parallel.
        let (buf, w, h) = fixture_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        obj.set_angle(30.0);
        obj.resize(2.0, 1.0);
        let c = obj.canvas_corners();
        let e0 = (c[1].0 - c[0].0, c[1].1 - c[0].1); // TL→TR
        let e1 = (c[2].0 - c[1].0, c[2].1 - c[1].1); // TR→BR
        let e2 = (c[3].0 - c[2].0, c[3].1 - c[2].1); // BR→BL
        let dot = e0.0 * e1.0 + e0.1 * e1.1;
        let norm = e0.0.hypot(e0.1).max(1e-6) * e1.0.hypot(e1.1).max(1e-6);
        assert!(
            dot.abs() / norm < 1e-4,
            "rotated non-uniform scale must stay rectangular (dot={dot})"
        );
        assert!(
            (e0.0 + e2.0).abs() < 1e-3 && (e0.1 + e2.1).abs() < 1e-3,
            "opposite edges must be parallel: e0={e0:?} e2={e2:?}"
        );
    }

    #[test]
    fn transform_layer_scale_then_rotate_is_palette_pure_and_deterministic() {
        // A fixture that ALREADY contains transparent background, so NN
        // sampling cannot introduce a new color.
        let palette: [[u8; 4]; 4] = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [0, 0, 0, 0],
        ];
        let mut buf = Vec::with_capacity(16 * 4);
        for i in 0..16 {
            buf.extend_from_slice(&palette[i % palette.len()]);
        }
        let mut obj = TransformObject::lift(layer(buf.clone(), 4, 4, 0), (0.0, 0.0));
        obj.set_angle(30.0);
        obj.resize(1.5, 2.0);
        let a = obj.commit();
        let b = obj.commit();
        assert_eq!(a[0].buf, b[0].buf, "same state must render identical bytes");
        // Every output pixel is a verbatim copy of an input pixel.
        let input: std::collections::BTreeSet<[u8; 4]> = buf
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        for px in a[0].buf.chunks_exact(4) {
            assert!(
                input.contains(&[px[0], px[1], px[2], px[3]]),
                "non-palette pixel {px:?}"
            );
        }
    }

    // -- Q1: single-pass affine NN resample ----------------------------------

    /// 1. At `S = I` the single-pass helper is a valid nearest rotation:
    /// dims match `rotated_bounds`, it is deterministic, palette-pure and
    /// alpha ∈ {0, 255}. It is NOT required to be byte-identical to the
    /// RotSprite `rotate` (which reconstructs 1px structure before sampling):
    /// both are valid pixel-art rotations with different artifact trade-offs.
    #[test]
    fn single_pass_unit_scale_properties() {
        let (src, w, h) = fixture_transparent_4x4();
        let palette: std::collections::BTreeSet<[u8; 4]> = src
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        for angle in [17.0f32, 30.0, 45.0, 123.0, -40.0] {
            let (a, aw, ah, _) = scale_rotate_nn(
                &src,
                w,
                h,
                (0.0, 0.0),
                (0.0, 0.0),
                angle,
                1.0,
                1.0,
                false,
                false,
            );
            let (b, bw, bh, _) = scale_rotate_nn(
                &src,
                w,
                h,
                (0.0, 0.0),
                (0.0, 0.0),
                angle,
                1.0,
                1.0,
                false,
                false,
            );
            assert_eq!(
                (aw, ah),
                rotated_bounds(w, h, angle),
                "dims at angle={angle}"
            );
            assert_eq!((aw, ah), (bw, bh));
            assert_eq!(a, b, "deterministic at angle={angle}");
            for c in a.chunks_exact(4) {
                let p = [c[0], c[1], c[2], c[3]];
                assert!(palette.contains(&p), "non-palette {p:?} at angle={angle}");
                assert!(c[3] == 0 || c[3] == 255, "partial alpha at angle={angle}");
            }
        }
    }

    /// 2. At `θ = 0` the helper is a pure scale and matches `scale_nn`.
    #[test]
    fn single_pass_equals_scale_nn_at_theta_zero() {
        let (src, w, h) = fixture_3x3();
        for (sx, sy) in [(2.0f32, 1.0f32), (1.5, 2.5), (0.5, 2.0)] {
            let sw = ((w as f64) * (sx as f64)).ceil() as usize;
            let sh = ((h as f64) * (sy as f64)).ceil() as usize;
            let (got, gw, gh, _) = scale_rotate_nn(
                &src,
                w,
                h,
                (0.0, 0.0),
                (0.0, 0.0),
                0.0,
                sx,
                sy,
                false,
                false,
            );
            let expect = scale_nn(&src, w, h, sw, sh);
            assert_eq!((gw, gh), (sw, sh));
            assert_eq!(got, expect, "theta=0 scale ({sx}, {sy})");
        }
    }

    /// 3. Generic angle + non-uniform scale (the Q1 single-pass case) is a
    /// valid nearest resample: dims match `rotated_bounds`, deterministic,
    /// palette-pure and alpha ∈ {0, 255}. The exact byte map of the bare NN
    /// composition is intentionally NOT frozen as ground truth (RotSprite
    /// supersedes it for the uniform case); only the invariants are pinned.
    #[test]
    fn single_pass_nonuniform_properties() {
        let (src, w, h) = fixture_transparent_4x4();
        let palette: std::collections::BTreeSet<[u8; 4]> = src
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        for (angle, sx, sy) in [
            (30.0f32, 1.7f32, 1.1f32),
            (45.0, 2.0, 0.6),
            (123.0, 1.3, 2.4),
        ] {
            let pos = (2.0f32, -1.0f32);
            let pivot = (4.0f32, 3.0f32);
            let (a, aw, ah, _) =
                scale_rotate_nn(&src, w, h, pos, pivot, angle, sx, sy, false, false);
            let (b, bw, bh, _) =
                scale_rotate_nn(&src, w, h, pos, pivot, angle, sx, sy, false, false);
            let sw = sw_of(w, sx);
            let sh = sh_of(h, sy);
            assert_eq!(
                (aw, ah),
                rotated_bounds(sw, sh, angle),
                "dims at angle={angle} s=({sx},{sy})"
            );
            assert_eq!((aw, ah), (bw, bh));
            assert_eq!(a, b, "deterministic at angle={angle}");
            for c in a.chunks_exact(4) {
                let p = [c[0], c[1], c[2], c[3]];
                assert!(palette.contains(&p), "non-palette {p:?} at angle={angle}");
                assert!(c[3] == 0 || c[3] == 255, "partial alpha at angle={angle}");
            }
        }
    }

    fn sw_of(w: usize, sx: f32) -> usize {
        ((w as f64) * (sx as f64)).ceil() as usize
    }
    fn sh_of(h: usize, sy: f32) -> usize {
        ((h as f64) * (sy as f64)).ceil() as usize
    }

    /// 4. Non-uniform θ=0 commit is byte-identical to `scale_nn`.
    #[test]
    fn angle_zero_bytes_unchanged() {
        let (src, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(src.clone(), w, h, 0), (2.0, 3.0));
        obj.resize(2.0, 1.5);
        let c = obj.commit();
        let sw = sw_of(w, 2.0);
        let sh = sh_of(h, 1.5);
        assert_eq!((c[0].w, c[0].h), (sw, sh));
        assert_eq!(c[0].buf, scale_nn(&src, w, h, sw, sh));
    }

    /// 5. θ=90 non-uniform commit is `rotate_90_cw(scale_nn(...))`.
    #[test]
    fn exact_90_commit_is_scale_then_exact_rotate() {
        let (src, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(src.clone(), w, h, 0), (1.0, 2.0));
        obj.set_angle(90.0);
        obj.resize(2.0, 3.0);
        let c = obj.commit();
        let sw = sw_of(w, 2.0);
        let sh = sh_of(h, 3.0);
        let scaled = scale_nn(&src, w, h, sw, sh);
        let (expect, ew, eh) = rotate_90_cw(&scaled, sw, sh);
        assert_eq!(c[0].buf, expect);
        assert_eq!((c[0].w, c[0].h), (ew, eh));
    }

    /// 6. Uniform scale at any angle stays on the two-pass path and is
    /// byte-unchanged by the dispatch.
    #[test]
    fn uniform_scale_any_angle_bytes_unchanged() {
        let (src, w, h) = fixture_3x3();
        for angle in [30.0f32, 45.0] {
            for s in [1.5f32, 2.0] {
                let mut obj = TransformObject::lift(layer(src.clone(), w, h, 0), (0.0, 0.0));
                obj.set_angle(angle);
                obj.resize(s, s);
                let c = obj.commit();
                let sw = sw_of(w, s);
                let sh = sh_of(h, s);
                let (expect, ew, eh) = rotate_with(
                    &scale_nn(&src, w, h, sw, sh),
                    sw,
                    sh,
                    angle,
                    TransformAlgorithm::RotSprite,
                );
                assert_eq!(c[0].buf, expect, "uniform s={s} angle={angle}");
                assert_eq!((c[0].w, c[0].h), (ew, eh));
            }
        }
    }

    /// 7. The single-pass result is palette-pure (verbatim copies, alpha
    /// 0/255) and deterministic.
    #[test]
    fn single_pass_palette_pure_and_deterministic() {
        let palette: [[u8; 4]; 4] = [
            [255, 0, 0, 255],
            [0, 255, 0, 255],
            [0, 0, 255, 255],
            [0, 0, 0, 0],
        ];
        let mut src = Vec::new();
        for i in 0..16 {
            src.extend_from_slice(&palette[i % palette.len()]);
        }
        let mut obj = TransformObject::lift(layer(src.clone(), 4, 4, 0), (0.0, 0.0));
        obj.set_angle(30.0);
        obj.resize(1.7, 0.8);
        let a = obj.commit();
        let b = obj.commit();
        assert_eq!(a[0].buf, b[0].buf, "deterministic");
        let input: std::collections::BTreeSet<[u8; 4]> = src
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        for px in a[0].buf.chunks_exact(4) {
            assert!(
                input.contains(&[px[0], px[1], px[2], px[3]]),
                "non-palette pixel {px:?}"
            );
            assert!(px[3] == 0 || px[3] == 255, "alpha not 0/255: {px:?}");
        }
    }

    /// 8. Folding the mirror flags into the single-pass sampler equals running
    /// it on an explicitly pre-flipped source.
    #[test]
    fn single_pass_flip_matches_preflip() {
        let (src, w, h) = fixture_3x3();
        for (fh, fv) in [(true, false), (false, true), (true, true)] {
            let flipped = flip_source(&src, w, h, fh, fv).0;
            let (a, ..) =
                scale_rotate_nn(&src, w, h, (1.0, 1.0), (2.0, 2.0), 30.0, 1.7, 0.9, fh, fv);
            let (b, ..) = scale_rotate_nn(
                &flipped,
                w,
                h,
                (1.0, 1.0),
                (2.0, 2.0),
                30.0,
                1.7,
                0.9,
                false,
                false,
            );
            assert_eq!(a, b, "flip ({fh},{fv})");
        }
    }

    #[test]
    fn commit_identity_returns_original_bytes_and_pos() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (10.0, 20.0));
        let commits = obj.commit();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].layer_id, 0);
        assert_eq!(commits[0].buf, buf);
        assert_eq!(commits[0].w, w);
        assert_eq!(commits[0].h, h);
        assert_eq!(commits[0].dst, (10.0, 20.0));
    }

    #[test]
    fn render_none_equals_commit() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.rotate_by(37f32.to_radians());
        obj.scale_by(1.5);
        obj.set_flips(true, false);
        let rendered = obj.render(None);
        let committed = obj.commit();
        assert_eq!(rendered.len(), committed.len());
        for (r, c) in rendered.iter().zip(committed.iter()) {
            assert_eq!(r.layer_id, c.layer_id);
            assert_eq!(r.w, c.w);
            assert_eq!(r.h, c.h);
            assert_eq!(r.buf, c.buf);
        }
    }

    #[test]
    fn cancel_restores_byte_exact_after_transform_soup() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (10.0, 20.0));
        let mut session = TransformSession::begin(obj);
        let o = session.object_mut().unwrap();
        o.rotate_by(37f32.to_radians());
        o.scale_by(1.5);
        o.set_flips(true, false);
        o.set_pivot((12.0, 22.0));
        let (restored, state) = session.cancel();
        assert_eq!(state, SessionState::Idle);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].layer_id, 0);
        assert_eq!(restored[0].w, w);
        assert_eq!(restored[0].h, h);
        assert_eq!(restored[0].buf, buf);
        assert_eq!(restored[0].dst, (10.0, 20.0));
    }

    #[test]
    fn rotate_90_matches_exact_cw() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        obj.set_angle(90.0);
        let commits = obj.commit();
        let (expected, ew, eh) = rotate_90_cw(&buf, w, h);
        assert_eq!(commits[0].w, ew);
        assert_eq!(commits[0].h, eh);
        assert_eq!(commits[0].buf, expected);
        // Pivot = centre stays fixed: the rotated 3×3 is placed at (0,0).
        assert_eq!(commits[0].dst, (0.0, 0.0));
    }

    #[test]
    fn rotate_around_pivot_90_matches_exact_and_integer_dst() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        obj.set_pivot((0.0, 0.0));
        obj.rotate_by(FRAC_PI_2);
        let commits = obj.commit();
        let (expected, ew, eh) = rotate_90_cw(&buf, w, h);
        assert_eq!((commits[0].w, commits[0].h), (ew, eh));
        assert_eq!(commits[0].buf, expected);
        // Integer snap: the exact-90° placement is bit-exact, no trig
        // epsilon — 3×3 rotated CW about the top-left AREA-corner pivot lands
        // at (-3, 0) EXACTLY.
        assert_eq!(commits[0].dst, (-3.0, 0.0));
    }

    #[test]
    fn rotate_around_pivot_180_exact() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        obj.set_pivot((0.0, 0.0));
        obj.rotate_by(PI);
        let commits = obj.commit();
        let (expected, ew, eh) = rotate_180(&buf, w, h);
        assert_eq!((commits[0].w, commits[0].h), (ew, eh));
        assert_eq!(commits[0].buf, expected);
        // Pivot stays fixed: 3×3 rotated 180° about the top-left AREA corner
        // lands at (-3, -3) exactly (integer snap).
        assert_eq!(commits[0].dst, (-3.0, -3.0));
    }

    #[test]
    fn rotate_around_pivot_45_nn_palette_pure() {
        // 45° around a non-centre pivot: NN sampling, no interpolation.
        let (mut buf, w, h) = fixture_3x3();
        // Make one source pixel transparent so the transparent OOB fill of
        // the rotated bounding box is part of the input palette (NN never
        // creates colors; strict subset requires 0,0,0,0 to already exist).
        buf[0..4].copy_from_slice(&[0, 0, 0, 0]);
        let mut obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        obj.set_pivot((0.0, 0.0));
        obj.rotate_by(FRAC_PI_4);
        let commits = obj.commit();
        let c = &commits[0];
        // 3×3 at 45° → 4×4 bounding box (matches [`rotate_with`]'s bbox).
        assert_eq!((c.w, c.h), (4, 4));
        assert_eq!(obj.bounds_size(), (4, 4));
        // Palette ⊆ input palette and alpha ∈ {0, 255} (NN, no AA/blend).
        let mut palette = std::collections::HashSet::new();
        for p in buf.chunks_exact(4) {
            palette.insert([p[0], p[1], p[2], p[3]]);
        }
        for p in c.buf.chunks_exact(4) {
            let col = [p[0], p[1], p[2], p[3]];
            assert!(palette.contains(&col), "new color {col:?} created at 45°");
            assert!(p[3] == 0 || p[3] == 255, "partial alpha {} (AA)", p[3]);
        }
    }

    #[test]
    fn pivot_getter_returns_field() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        assert_eq!(obj.pivot(), obj.pivot);
    }

    #[test]
    fn rotate_by_takes_radians() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.rotate_by(FRAC_PI_2);
        assert_eq!(obj.angle_deg, 90.0);
        obj.rotate_by(FRAC_PI_2);
        assert_eq!(obj.angle_deg, 180.0);
        // 45° in radians → 45.0 degrees; a full turn in radians → 360°.
        let mut full = TransformObject::lift(layer(vec![0u8; 4], 1, 1, 0), (0.0, 0.0));
        full.rotate_by(std::f32::consts::TAU);
        assert_eq!(full.angle_deg, 360.0);
    }

    #[test]
    fn rotate_45_nn_mapping_no_blend_on_2x1() {
        // 2×1: one opaque pixel, one transparent; at 45° every output pixel
        // is either fully transparent or the exact opaque source color
        // (nearest-neighbour through the full TransformObject pipeline).
        let buf = [255u8, 0, 0, 255, 0, 0, 0, 0];
        let mut obj = TransformObject::lift(layer(buf.to_vec(), 2, 1, 0), (0.0, 0.0));
        obj.rotate_by(FRAC_PI_4);
        let commits = obj.commit();
        let c = &commits[0];
        assert_eq!((c.w, c.h), (2, 2));
        for p in c.buf.chunks_exact(4) {
            assert!(p[3] == 0 || p[3] == 255, "blended alpha {}", p[3]);
            if p[3] == 255 {
                assert_eq!([p[0], p[1], p[2], p[3]], [255, 0, 0, 255]);
            }
        }
    }

    #[test]
    fn rotate_360_pivot_invariance() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        // Non-centre pivot to prove the placement returns to pos too.
        obj.set_pivot((0.0, 0.0));
        // 36 × 10° steps; every commit is recomputed from the SAME source
        // (commit takes &self and never mutates the stored buffers), so no
        // incremental error can accumulate.
        for _ in 0..36 {
            obj.rotate_by((10f32).to_radians());
            let commits = obj.commit();
            assert_eq!(commits.len(), 1);
        }
        // After a full 360° (accumulated exactly from 36 × 10°-in-radians
        // steps) the buffer is byte-identical to the original and the
        // placement returns to the original pos — the exact-90°-multiple
        // branch lands dst on the integer grid, so this is exact.
        let commits = obj.commit();
        assert_eq!(commits[0].buf, buf);
        assert_close(commits[0].dst.0, 0.0);
        assert_close(commits[0].dst.1, 0.0);
    }

    #[test]
    fn scale_gt_one_oob_stays_transparent() {
        // 2×2 with a single opaque pixel; rotate 45° then scale 2×.
        // Pixels outside the rotated footprint must stay transparent
        // (matches U16's out-of-bounds rule).
        let mut buf = vec![0u8; 2 * 2 * 4];
        buf[0..4].copy_from_slice(&[255, 0, 0, 255]); // top-left opaque
        let mut obj = TransformObject::lift(layer(buf, 2, 2, 0), (0.0, 0.0));
        obj.set_angle(45.0);
        obj.scale_by(2.0);
        let commits = obj.commit();
        let c = &commits[0];
        // 2×2 at 45° → 3×3; scaled 2× → 6×6.
        assert_eq!((c.w, c.h), (6, 6));
        // Corners of the scaled result map to the transparent corners of
        // the rotated footprint.
        assert_eq!(px(&c.buf, c.w, 0, 0)[3], 0);
        assert_eq!(px(&c.buf, c.w, c.w - 1, 0)[3], 0);
        assert_eq!(px(&c.buf, c.w, 0, c.h - 1)[3], 0);
        assert_eq!(px(&c.buf, c.w, c.w - 1, c.h - 1)[3], 0);
    }

    #[test]
    fn flips_match_exact_paths() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        obj.set_flips(true, false);
        let commits = obj.commit();
        let (expected, _, _) = flip_h(&buf, w, h);
        assert_eq!(commits[0].buf, expected);
        assert_eq!(commits[0].dst, (0.0, 0.0));
    }

    #[test]
    fn multi_layer_commit_preserves_layer_ids() {
        let (buf0, w0, h0) = fixture_3x3();
        let (buf1, w1, h1) = fixture_4x2();
        let mut obj = TransformObject::from_layers(
            vec![layer(buf0.clone(), w0, h0, 0), layer(buf1, w1, h1, 1)],
            (0.0, 0.0),
        );
        obj.set_angle(90.0);
        let commits = obj.commit();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].layer_id, 0);
        assert_eq!(commits[1].layer_id, 1);
        // Each layer is transformed independently (90° swaps dims).
        assert_eq!((commits[0].w, commits[0].h), (h0, w0));
        assert_eq!((commits[1].w, commits[1].h), (h1, w1));
        // Layer 0's buffer matches the exact 90° path bit-for-bit.
        let (expected, _, _) = rotate_90_cw(&buf0, w0, h0);
        assert_eq!(commits[0].buf, expected);
    }

    #[test]
    fn session_begin_is_active() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        let session = TransformSession::begin(obj);
        assert_eq!(session.state(), SessionState::Active);
        let mut session = session;
        assert!(session.object_mut().is_some());
    }

    #[test]
    fn session_commit_returns_deltas_and_idle() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        let mut session = TransformSession::begin(obj);
        session.object_mut().unwrap().rotate_by(FRAC_PI_2);
        let (deltas, state) = session.commit();
        assert_eq!(state, SessionState::Idle);
        assert_eq!(deltas.len(), 1);
        let (expected, _, _) = rotate_90_cw(&buf, w, h);
        assert_eq!(deltas[0].buf, expected);
    }

    #[test]
    fn session_cancel_restores_originals_and_idle() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (5.0, 6.0));
        let mut session = TransformSession::begin(obj);
        session.object_mut().unwrap().rotate_by(37f32.to_radians());
        session.object_mut().unwrap().scale_by(2.0);
        let (originals, state) = session.cancel();
        assert_eq!(state, SessionState::Idle);
        assert_eq!(originals.len(), 1);
        assert_eq!(originals[0].buf, buf);
        assert_eq!(originals[0].dst, (5.0, 6.0));
    }

    #[test]
    fn render_preview_fits_max_dim() {
        // Large source (200×200) → preview must fit within 64×64.
        let buf = vec![0u8; 200 * 200 * 4];
        let mut obj = TransformObject::lift(layer(buf, 200, 200, 0), (0.0, 0.0));
        obj.rotate_by((30f32).to_radians());
        obj.scale_by(1.5);
        let preview = obj.render(Some(64));
        assert_eq!(preview.len(), 1);
        assert!(preview[0].w <= 64 && preview[0].h <= 64);
        assert!(!preview[0].buf.is_empty());
    }

    #[test]
    fn render_preview_deterministic() {
        let buf = vec![0u8; 100 * 100 * 4];
        let mut obj = TransformObject::lift(layer(buf, 100, 100, 0), (0.0, 0.0));
        obj.rotate_by((25f32).to_radians());
        let a = obj.render(Some(64));
        let b = obj.render(Some(64));
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.w, y.w);
            assert_eq!(x.h, y.h);
            assert_eq!(x.buf, y.buf);
        }
    }

    #[test]
    fn render_preview_small_source_is_exact() {
        // A source that already fits within max_dim returns the exact
        // result (bit-identical to commit).
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.rotate_by(FRAC_PI_2);
        let preview = obj.render(Some(64));
        let committed = obj.commit();
        assert_eq!(preview.len(), committed.len());
        assert_eq!(preview[0].w, committed[0].w);
        assert_eq!(preview[0].h, committed[0].h);
        assert_eq!(preview[0].buf, committed[0].buf);
    }

    #[test]
    fn preview_threshold_boundary_is_bounded_and_exact_when_fit() {
        let buf = vec![7u8; 64 * 64 * 4];
        let obj = TransformObject::lift(layer(buf, 64, 64, 0), (0.0, 0.0));
        let below = obj.render(Some(63));
        let at = obj.render(Some(64));
        let above = obj.render(Some(65));
        assert!(below[0].w <= 63 && below[0].h <= 63);
        assert_eq!(at[0].w, 64);
        assert_eq!(at[0].h, 64);
        assert_eq!(above[0].buf, at[0].buf);
    }

    #[test]
    fn non_square_preview_is_bounded_without_mutating_commit_state() {
        let buf = vec![11u8; 65 * 32 * 4];
        let mut obj = TransformObject::lift(layer(buf, 65, 32, 0), (3.0, 4.0));
        obj.rotate_by((17f32).to_radians());
        let exact_before = obj.commit();
        let preview = obj.render(Some(64));
        let exact_after = obj.commit();
        assert!(preview[0].w <= 64 && preview[0].h <= 64);
        assert_eq!(exact_before, exact_after);
    }

    /// The `<= max_dim` fast path returns the exact commit bytes
    /// (preview == commit for small selections).
    #[test]
    fn preview_fast_path_equals_commit() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.rotate_by((30f32).to_radians());
        let exact = obj.commit();
        assert!(exact[0].w <= 64 && exact[0].h <= 64);
        let preview = obj.render(Some(64));
        assert_eq!(preview[0].w, exact[0].w);
        assert_eq!(preview[0].h, exact[0].h);
        assert_eq!(preview[0].buf, exact[0].buf);
    }

    /// Oversize preview: the transform runs at FULL resolution first and only
    /// the RESULT is downscaled. The preview bytes are therefore exactly the
    /// single NN downscale of the full-resolution commit — the source is
    /// never downscaled before the rotation (the R1 P1 regression).
    #[test]
    fn oversize_preview_is_post_downscale_of_full_res_commit() {
        // 80×80 transparent field with a 1px-thick horizontal line (source
        // detail a pre-rotation source downscale could erase).
        let mut buf = vec![0u8; 80 * 80 * 4];
        for x in 20..60 {
            let i = (40 * 80 + x) * 4;
            buf[i..i + 4].copy_from_slice(&[255, 0, 0, 255]);
        }
        let mut obj = TransformObject::lift(layer(buf, 80, 80, 0), (0.0, 0.0));
        obj.set_angle(30.0);

        let exact = obj.commit();
        let (ew, eh) = (exact[0].w, exact[0].h);
        assert!(
            ew > 64 || eh > 64,
            "fixture must be oversize (got {ew}×{eh})"
        );
        assert!(
            exact[0].buf.chunks_exact(4).any(|p| p[3] == 255),
            "full-resolution commit kept no content"
        );

        let preview = obj.render(Some(64));
        assert_eq!(preview.len(), 1);
        assert!(preview[0].w <= 64 && preview[0].h <= 64);
        // Proof of the single post-downscale: the preview is byte-identical to
        // downscaling the full-resolution commit result.
        let expect = scale_nn(&exact[0].buf, ew, eh, preview[0].w, preview[0].h);
        assert_eq!(preview[0].buf, expect);
    }

    #[test]
    fn impossible_scale_dimension_returns_safe_empty_result() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        // The pipeline reads the per-axis factors; an overflow-sized factor
        // must produce a safe empty result rather than a panic.
        obj.sx = f32::MAX;
        let committed = obj.commit();
        assert_eq!(committed.len(), 1);
        assert!(committed[0].buf.is_empty());
        assert_eq!((committed[0].w, committed[0].h), (0, 0));
    }

    #[test]
    fn resize_non_uniform_dims() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.resize(2.0, 1.0);
        assert_eq!(obj.scale_xy(), (2.0, 1.0));
        let commits = obj.commit();
        assert_eq!((commits[0].w, commits[0].h), (6, 3));
        // Legacy `scale` is untouched for a non-uniform resize (sx != sy).
        assert_eq!(obj.scale, 1.0);
    }

    #[test]
    fn resize_non_uniform_nn_palette_pure() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        obj.resize(2.0, 1.0);
        let commits = obj.commit();
        let c = &commits[0];
        assert_eq!((c.w, c.h), (6, 3));
        // NN scaling copies pixels verbatim: no new colors, alpha stays
        // ∈ {0, 255} (the fixture is fully opaque → every output is too).
        let mut palette = std::collections::HashSet::new();
        for p in buf.chunks_exact(4) {
            palette.insert([p[0], p[1], p[2], p[3]]);
        }
        for p in c.buf.chunks_exact(4) {
            let col = [p[0], p[1], p[2], p[3]];
            assert!(palette.contains(&col), "new color {col:?} created");
            assert!(p[3] == 0 || p[3] == 255, "partial alpha {}", p[3]);
        }
    }

    #[test]
    fn resize_non_uniform_integer_dst() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_pivot((0.0, 0.0));
        obj.resize(2.0, 1.0);
        // Integer pivot/pos and integer factors → dst is EXACTLY integer.
        let commits = obj.commit();
        assert_eq!(commits[0].dst, (0.0, 0.0));
    }

    #[test]
    fn resize_uniform_matches_scale_by() {
        let (buf, w, h) = fixture_3x3();
        let mut a = TransformObject::lift(layer(buf.clone(), w, h, 0), (0.0, 0.0));
        let mut b = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        a.resize(2.0, 2.0);
        b.scale_by(2.0);
        assert_eq!(a.scale, b.scale);
        assert_eq!(a.scale_xy(), b.scale_xy());
        let ca = a.commit();
        let cb = b.commit();
        assert_eq!(ca[0].w, cb[0].w);
        assert_eq!(ca[0].h, cb[0].h);
        assert_eq!(ca[0].buf, cb[0].buf);
        assert_eq!(ca[0].dst, cb[0].dst);
    }

    #[test]
    fn scale_by_syncs_sx_sy() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.scale_by(2.0);
        assert_eq!(obj.scale, 2.0);
        assert_eq!(obj.sx, 2.0);
        assert_eq!(obj.sy, 2.0);
        obj.scale_by(0.5);
        assert_eq!(obj.scale, 1.0);
        assert_eq!(obj.sx, 1.0);
        assert_eq!(obj.sy, 1.0);
        // The 0.01 clamp applies to every axis.
        obj.scale_by(0.0001);
        assert_eq!(obj.scale, 0.01);
        assert_eq!(obj.sx, 0.01);
        assert_eq!(obj.sy, 0.01);
    }

    #[test]
    fn scale_xy_getter() {
        let (buf, w, h) = fixture_3x3();
        let obj = TransformObject::lift(layer(buf, w, h, 0), (10.0, 20.0));
        assert_eq!(obj.scale_xy(), (1.0, 1.0));
    }

    #[test]
    fn resize_after_rotation_preview_fits() {
        // Large source, non-uniform factors (2× / 0.5×) plus 45° rotation:
        // the preview must stay within max_dim on BOTH axes.
        let buf = vec![7u8; 200 * 200 * 4];
        let mut obj = TransformObject::lift(layer(buf, 200, 200, 0), (0.0, 0.0));
        obj.rotate_by(FRAC_PI_4);
        obj.resize(2.0, 0.5);
        let preview = obj.render(Some(64));
        assert_eq!(preview.len(), 1);
        assert!(
            preview[0].w <= 64 && preview[0].h <= 64,
            "preview {}×{} exceeds 64×64",
            preview[0].w,
            preview[0].h
        );
        assert!(!preview[0].buf.is_empty());
        // The exact result is genuinely out of bounds (k < 1 was needed).
        let exact = obj.commit();
        assert!(
            exact[0].w > 64 || exact[0].h > 64,
            "expected an exact result that exceeds max_dim"
        );
    }

    #[test]
    fn rotated_dims_matches_bounds_size() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.scale_by(1.5);
        // ceil(3 * 1.5) = 5.
        assert_eq!(obj.rotated_dims(), (5, 5));
        let b = obj.bounds_size();
        assert_eq!(obj.rotated_dims(), (b.0 as usize, b.1 as usize));
    }

    #[test]
    fn resize_to_pixels_exact_target() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.resize_to_pixels(32, 18, (0.0, 0.0));
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), (32, 18));
        assert_eq!(obj.rotated_dims(), (32, 18));

        let (buf, w, h) = fixture_4x2();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.resize_to_pixels(7, 3, (0.0, 0.0));
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), (7, 3));
        assert_eq!(obj.rotated_dims(), (7, 3));

        let (buf, w, h) = fixture_4x2();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.resize_to_pixels(1, 1, (0.0, 0.0));
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), (1, 1));
        assert_eq!(obj.rotated_dims(), (1, 1));
    }

    #[test]
    fn resize_to_pixels_non_representable_ratios() {
        // 7×5 → 13×11: neither 13/7 nor 11/5 is exactly representable.
        let mut obj = TransformObject::lift(layer(vec![0u8; 7 * 5 * 4], 7, 5, 0), (0.0, 0.0));
        obj.resize_to_pixels(13, 11, (0.0, 0.0));
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), (13, 11));
        assert_eq!(obj.rotated_dims(), (13, 11));

        // 1×1 → 4096×3: large non-representable ratio, no 1px drift.
        let mut obj = TransformObject::lift(layer(vec![0u8; 4], 1, 1, 0), (0.0, 0.0));
        obj.resize_to_pixels(4096, 3, (0.0, 0.0));
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), (4096, 3));
        assert_eq!(obj.rotated_dims(), (4096, 3));
    }

    #[test]
    fn resize_after_rotation_exact() {
        // 5×3 rotated 30°: `resize_to_pixels` now targets LOCAL dims, so a
        // 20×11 local target renders as the rotated bounds of 20×11.
        let mut obj = TransformObject::lift(layer(vec![0u8; 5 * 3 * 4], 5, 3, 0), (0.0, 0.0));
        obj.set_angle(30.0);
        assert_eq!(obj.local_scaled_dims(), (5, 3));
        assert_eq!(obj.rotated_dims(), (6, 5));
        obj.resize_to_pixels(20, 11, (0.0, 0.0));
        assert_eq!(obj.local_scaled_dims(), (20, 11));
        let expect = rotated_bounds(20, 11, 30.0);
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), expect);
        assert_eq!(obj.rotated_dims(), expect);
    }

    #[test]
    fn resize_to_pixels_keeps_anchor_fixed() {
        // Angle 0: 4×4 at (0,0), AREA centre pivot (2,2), anchor (0,0).
        let (buf, w, h) = fixture_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        let anchor = (0.0f32, 0.0f32);
        let s_old = obj.sx;
        let u_star = (
            (anchor.0 - obj.pivot.0) / s_old - obj.pos.0 + obj.pivot.0,
            (anchor.1 - obj.pivot.1) / s_old - obj.pos.1 + obj.pivot.1,
        );
        obj.resize_to_pixels(8, 8, anchor);
        let p = canvas_point(&obj, u_star);
        assert!(
            (p.0 - anchor.0).abs() < 1e-3 && (p.1 - anchor.1).abs() < 1e-3,
            "angle 0 anchor drifted: {p:?}"
        );

        // Angle 30: u* = R⁻¹·((anchor−pivot)/s_old) + pivot − pos.
        let (buf, w, h) = fixture_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_angle(30.0);
        let s_old = obj.sx;
        let q = (
            (anchor.0 - obj.pivot.0) / s_old,
            (anchor.1 - obj.pivot.1) / s_old,
        );
        let ri = rot_inv(q, 30.0);
        let u_star = (
            ri.0 + obj.pivot.0 - obj.pos.0,
            ri.1 + obj.pivot.1 - obj.pos.1,
        );
        obj.resize_to_pixels(8, 8, anchor);
        let p = canvas_point(&obj, u_star);
        assert!(
            (p.0 - anchor.0).abs() < 1e-3 && (p.1 - anchor.1).abs() < 1e-3,
            "angle 30 anchor drifted: {p:?}"
        );
    }

    /// `resize_local_to_pixels_at_min` places the LOCAL scaled rect's min
    /// RELATIVE TO THE PIVOT absolutely (including on the opposite side of the
    /// pivot), preserving the exact target LOCAL dims — the primitive a
    /// start-referenced local flip needs.
    #[test]
    fn resize_local_to_pixels_at_min_pins_the_local_min() {
        // Unrotated: the local min is the canvas min. Pin it to the LEFT of
        // the pivot and verify the exact local dims.
        let (buf, w, h) = fixture_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_pivot((0.0, 0.0));
        obj.set_flips(true, false);
        obj.resize_local_to_pixels_at_min(8, 4, (-8.0, 0.0));
        assert!(obj.flip_h && !obj.flip_v, "flip_h must be applied");
        assert_eq!(obj.local_scaled_dims(), (8, 4));
        let (min_x, min_y, max_x, max_y) = obj.canvas_bbox();
        assert_close(min_x, -8.0);
        assert_close(min_y, 0.0);
        assert_close(max_x, 0.0);
        assert_close(max_y, 4.0);
        // Preview == commit: the committed buffer has the exact local dims.
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), (8, 4));

        // Rotated: the local min is pinned in the UN-rotated frame, so the
        // source min maps to `pivot + R·local_min`.
        let (buf, w, h) = fixture_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_pivot((0.0, 0.0));
        obj.set_angle(30.0);
        obj.resize_local_to_pixels_at_min(9, 7, (3.0, -2.0));
        assert_eq!(obj.local_scaled_dims(), (9, 7));
        let p = canvas_point(&obj, (0.0, 0.0));
        let theta = 30f32.to_radians();
        let (c, s) = (theta.cos(), theta.sin());
        let expect = (
            obj.pivot.0 + c * 3.0 - s * -2.0,
            obj.pivot.1 + s * 3.0 + c * -2.0,
        );
        assert!(
            (p.0 - expect.0).abs() < 1e-3 && (p.1 - expect.1).abs() < 1e-3,
            "source min {p:?} != pivot + R·local_min {expect:?}"
        );
    }

    #[test]
    fn resize_to_pixels_never_flips_or_vanishes() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.resize_to_pixels(0, 0, (5.0, 5.0));
        let c = obj.commit();
        assert_eq!((c[0].w, c[0].h), (1, 1));
        assert!(obj.sx > 0.0 && obj.sy > 0.0);
        assert!(!obj.flip_h && !obj.flip_v);

        // A pointer that crossed the anchor (negative factor) is clamped
        // positive, never flipped or vanished.
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.sx = -5.0;
        obj.clamp_min_size();
        assert!(obj.sx > 0.0 && obj.sy > 0.0);
        let d = obj.rotated_dims();
        assert!(d.0 >= 1 && d.1 >= 1, "dims vanished: {d:?}");
        assert!(!obj.flip_h && !obj.flip_v);
        let c = obj.commit();
        assert!(c[0].w >= 1 && c[0].h >= 1);
    }

    #[test]
    fn placement_memo_cache_short_circuits_and_invalidates() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        let a = obj.canvas_bbox();
        let b = obj.canvas_bbox();
        assert_eq!(a, b, "same state must yield identical placement");
        assert_eq!(
            obj.placement_computations(),
            1,
            "second call must be cached"
        );

        obj.set_angle(30.0);
        let c = obj.canvas_bbox();
        assert_ne!(c, a, "angle change must change the bbox");
        assert_eq!(obj.placement_computations(), 2, "angle change invalidates");

        let d = obj.canvas_bbox();
        assert_eq!(c, d);
        assert_eq!(obj.placement_computations(), 2, "repeat stays cached");
    }

    #[test]
    fn placement_math_matches_transform_layer_dst() {
        // `canvas_bbox` min is now pure math; it must equal the `dst` that
        // `commit` (the old transform_layer reference) reports, for identity,
        // exact 90° multiples, generic angles and non-centre pivots.
        let cases = [
            (0.0f32, (1.5f32, 1.5f32), (0.0f32, 0.0f32)),
            (90.0, (1.5, 1.5), (0.0, 0.0)),
            (180.0, (1.5, 1.5), (10.0, 20.0)),
            (270.0, (2.5, 1.5), (5.0, -2.0)),
            (45.0, (0.0, 0.0), (3.0, 4.0)),
            (37.0, (1.0, 2.0), (-1.0, 3.0)),
        ];
        for (angle, pivot, pos) in cases {
            let (buf, w, h) = fixture_3x3();
            let mut obj = TransformObject::lift(layer(buf, w, h, 0), pos);
            obj.set_pivot(pivot);
            obj.set_angle(angle);
            let (min_x, min_y, _, _) = obj.canvas_bbox();
            let dst = obj.commit()[0].dst;
            assert_close(min_x, dst.0);
            assert_close(min_y, dst.1);
        }
    }

    // -- W1: rotation-algorithm selection through the pipeline --------------

    const ALL_ALGS: [TransformAlgorithm; 3] = [
        TransformAlgorithm::RotSprite,
        TransformAlgorithm::CleanEdge,
        TransformAlgorithm::Rotxel,
    ];

    #[test]
    fn algorithm_getter_setter_round_trips() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        assert_eq!(obj.algorithm(), TransformAlgorithm::RotSprite);
        for alg in ALL_ALGS {
            obj.set_algorithm(alg);
            assert_eq!(obj.algorithm(), alg);
        }
    }

    /// Every algorithm is selectable and flows through `commit`/`render(None)`:
    /// no panic, deterministic, palette-pure, alpha-binary, and the selection
    /// actually changes the output at a generic angle.
    #[test]
    fn algorithm_selection_flows_through_render_and_commit() {
        let (buf, w, h) = fixture_transparent_4x4();
        let palette: std::collections::BTreeSet<[u8; 4]> = buf
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_angle(30.0);

        let mut outputs = Vec::new();
        for alg in ALL_ALGS {
            obj.set_algorithm(alg);
            assert_eq!(obj.algorithm(), alg);

            let committed = obj.commit();
            let rendered = obj.render(None);
            assert_eq!(committed.len(), 1, "{alg:?}");
            assert_eq!(rendered.len(), committed.len(), "{alg:?}");
            assert_eq!(rendered[0].w, committed[0].w, "{alg:?}");
            assert_eq!(rendered[0].h, committed[0].h, "{alg:?}");
            assert_eq!(
                rendered[0].buf, committed[0].buf,
                "render==commit for {alg:?}"
            );

            // Deterministic.
            assert_eq!(obj.commit()[0].buf, committed[0].buf, "{alg:?}");

            // Palette-pure and no anti-aliasing.
            for p in committed[0].buf.chunks_exact(4) {
                assert!(
                    palette.contains(&[p[0], p[1], p[2], p[3]]),
                    "non-palette {p:?} with {alg:?}"
                );
                assert!(p[3] == 0 || p[3] == 255, "partial alpha with {alg:?}");
            }
            outputs.push(committed[0].buf.clone());
        }

        assert!(
            outputs[0] != outputs[1] || outputs[0] != outputs[2] || outputs[1] != outputs[2],
            "algorithm selection did not change the pipeline output"
        );
    }

    /// Exact 0/90/180/270° results are byte-identical across all algorithms
    /// through the full pipeline.
    #[test]
    fn algorithm_selection_exact_angles_identical() {
        let (buf, w, h) = fixture_transparent_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        for angle in [0.0f32, 90.0, 180.0, 270.0] {
            obj.set_angle(angle);
            let mut outs = Vec::new();
            for alg in ALL_ALGS {
                obj.set_algorithm(alg);
                outs.push(obj.commit()[0].buf.clone());
            }
            assert!(
                outs.iter().all(|o| *o == outs[0]),
                "exact angle {angle}° must be identical across algorithms"
            );
        }
    }

    /// The algorithm is not a geometry input: dims and placement are identical
    /// across algorithm choices.
    #[test]
    fn algorithm_does_not_change_dims_or_placement() {
        let (buf, w, h) = fixture_4x2();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (3.0, 4.0));
        obj.set_angle(30.0);
        obj.resize(1.5, 1.5);
        let mut dims = Vec::new();
        let mut dsts = Vec::new();
        for alg in ALL_ALGS {
            obj.set_algorithm(alg);
            let c = obj.commit();
            dims.push((c[0].w, c[0].h));
            dsts.push(c[0].dst);
        }
        assert!(dims.iter().all(|d| *d == dims[0]), "dims differ: {dims:?}");
        assert!(dsts.iter().all(|d| *d == dsts[0]), "dst differs: {dsts:?}");
    }

    /// `set_algorithm` survives a session round-trip (API W2 relies on).
    #[test]
    fn algorithm_persists_through_session() {
        let (buf, w, h) = fixture_3x3();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_algorithm(TransformAlgorithm::Rotxel);
        let session = TransformSession::begin(obj);
        assert_eq!(
            session.object().algorithm(),
            TransformAlgorithm::Rotxel,
            "algorithm must be readable through the session"
        );
    }

    // -- Y2-B: override-algorithm preview (fast drag vs selected) ------------

    /// `render_with_algorithm` renders with the OVERRIDE despite a different
    /// stored algorithm, and never mutates the stored selection. The override
    /// output matches an object that stores the override algorithm.
    #[test]
    fn render_with_algorithm_overrides_stored_algorithm_without_mutating() {
        let (buf, w, h) = fixture_transparent_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_angle(30.0);
        obj.set_algorithm(TransformAlgorithm::CleanEdge);

        let overridden = obj.render_with_algorithm(None, TransformAlgorithm::Rotxel);

        // A clone storing Rotxel produces the same bytes as the override.
        let mut stored =
            TransformObject::lift(layer(fixture_transparent_4x4().0, 4, 4, 0), (0.0, 0.0));
        stored.set_angle(30.0);
        stored.set_algorithm(TransformAlgorithm::Rotxel);
        let stored_out = stored.render(None);
        assert_eq!(overridden.len(), 1);
        assert_eq!(overridden[0].w, stored_out[0].w);
        assert_eq!(overridden[0].h, stored_out[0].h);
        assert_eq!(
            overridden[0].buf, stored_out[0].buf,
            "override must render as the override algorithm"
        );

        // The stored algorithm still drives the plain render and differs at a
        // generic angle (the override actually took effect).
        let selected = obj.render(None);
        assert_ne!(
            overridden[0].buf, selected[0].buf,
            "override must beat the stored algorithm at a generic angle"
        );
        assert_eq!(
            obj.algorithm(),
            TransformAlgorithm::CleanEdge,
            "render_with_algorithm must not mutate the stored algorithm"
        );
    }

    /// Commit ALWAYS uses the stored (selected) algorithm, even right after a
    /// fast override preview: the preview==commit guarantee is preserved.
    #[test]
    fn commit_uses_selected_algorithm_even_after_fast_override_preview() {
        let (buf, w, h) = fixture_transparent_4x4();
        let mut obj = TransformObject::lift(layer(buf, w, h, 0), (0.0, 0.0));
        obj.set_angle(30.0);
        obj.set_algorithm(TransformAlgorithm::CleanEdge);

        // Simulate a live drag preview rendered with the FAST algorithm.
        let preview = obj.render_with_algorithm(Some(64), TransformAlgorithm::Rotxel);
        assert_eq!(preview.len(), 1);

        let committed = obj.commit();
        let expected = obj.render_with_algorithm(None, TransformAlgorithm::CleanEdge);
        assert_eq!(committed.len(), 1);
        assert_eq!(
            committed[0].buf, expected[0].buf,
            "commit must use the selected algorithm, not the preview override"
        );
        assert_eq!(obj.algorithm(), TransformAlgorithm::CleanEdge);
    }
}
