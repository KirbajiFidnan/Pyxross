//! Transform gizmo overlay — geometry, hit-testing (D55), painting.
//!
//! Pure egui: no wgpu, no winit. Provides a canvas-space four-corner quad
//! representation, screen-space handle geometry, hit-testing, and egui paint
//! calls. All geometry functions are pure and deterministic — fully
//! unit-testable without a painter.
//!
//! The gizmo is a QUAD, not an axis-aligned box: the four corners come from
//! [`crate::core::transform::TransformObject::canvas_corners`], so the outline,
//! handles and rotation hints ROTATE WITH the transformed pixels.
//!
//! # Handle layout (shown unrotated)
//!
//! ```text
//!    NW-rot ×    [NW]·[Top]·[NE]    × NE-rot
//!                |           |
//!               [L] — translate — [R]
//!                |   (centre •)   |
//!               [SW]·[Bottom]·[SE]
//!    SW-rot ×                        × SE-rot
//! ```
//!
//! - **Corners** → corner scale handles at `corners[0..4]` mapped
//!   `TL→ScaleNW`, `TR→ScaleNE`, `BR→ScaleSE`, `BL→ScaleSW` (drawn as 8×8 px
//!   squares, hit-tested as 15×15 px squares so they are easy to grab).
//! - **Edge midpoints** → edge scale handles at the midpoints of consecutive
//!   corners: `TL-TR→ScaleTop`, `TR-BR→ScaleRight`, `BR-BL→ScaleBottom`,
//!   `BL-TL→ScaleLeft` (8×8 px drawn inside a 15×15 px hit square).
//! - **Outward diagonals just past each corner** → rotation zones
//!   (17×17 px hit squares, NOT painted): [`GizmoHit::Rotate`], placed
//!   `ROT_OUTSET_D` from the corner along its outward diagonal
//!   (`normalize(corner − centre)`, carrying the legacy √2 magnitude so the
//!   offset keeps its per-axis component scale). Strictly disjoint from the
//!   corner hit square — directly on a corner resizes, slightly outside
//!   diagonally rotates.
//! - **Quad centre** → a read-only centre mark (`paint`'s crosshair), NOT a
//!   handle. The pivot is always the centre of the transformed pixels and the
//!   user cannot move it. `paint` keeps a `pivot` argument only so the mark
//!   can be drawn; `hit_test` has no pivot argument at all.
//! - **Quad interior** → [`GizmoHit::Translate`] (point-in-quad, so the centre
//!   mark is the translate interior too).
//!
//! Hit-testing runs in priority order — rotation zones, corners, edges, then
//! the translate interior — so smaller, more-specific handles always win over
//! the catch-all interior.

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Drawn size of the corner/edge scale handles in screen pixels.
const HANDLE_PX: f32 = 8.0;
/// Hit-test size of the corner/edge scale handles in screen pixels. Larger
/// than the drawn square so the handles are easy to grab: the painted 8×8
/// square sits inside a 15×15 hit target.
const HANDLE_HIT_PX: f32 = 15.0;
/// Size of the (unpainted) rotation-zone hit squares in screen pixels.
const ROT_ZONE_PX: f32 = 17.0;
/// Gap kept between a corner's hit square and its rotation zone, in screen
/// pixels, so the two hit areas are STRICTLY disjoint at every zoom.
const ROT_GAP_PX: f32 = 2.0;
/// Distance from each corner to its rotation-zone centre, as a PER-AXIS
/// component offset along the corner's outward diagonal, in screen pixels.
/// (The outward direction carries the legacy √2 magnitude, so this is the
/// component offset, not the Euclidean distance.) Chosen so the zone's inner
/// edge (`ROT_OUTSET_D − ROT_ZONE_PX/2`) clears the corner hit square's
/// half-width (`HANDLE_HIT_PX/2`) by [`ROT_GAP_PX`]: with the current sizes
/// `7.5 + 8.5 + 2 = 18` px from the corner, versus the corner's 7.5 px
/// half-width — a 2 px gap, at any zoom.
///
/// Directly on a corner (anywhere inside the 15×15 hit square) therefore
/// resizes; slightly outside along the diagonal rotates.
const ROT_OUTSET_D: f32 = HANDLE_HIT_PX * 0.5 + ROT_ZONE_PX * 0.5 + ROT_GAP_PX;
/// Half-length of the painted (read-only) centre mark's crosshair arms in
/// screen pixels.
const PIVOT_ARM_PX: f32 = 5.0;
/// Length of one dash (and of the gap between dashes) of the quad outline,
/// in screen pixels.
const BBOX_DASH_PX: f32 = 4.0;
/// Duration of the per-handle hover highlight fade, in seconds.
const HOVER_ANIM_SECS: f32 = 0.1;
/// Half-extent (screen pixels) of the L-shaped rotation hint: the bracket is a
/// `2 * ROT_L_HALF_PX` square, comfortably smaller than the
/// [`ROT_ZONE_PX`] hit square it sits on.
const ROT_L_HALF_PX: f32 = 5.0;
/// Legacy diagonal magnitude: the outward direction is `unit · √2`, so
/// [`ROT_OUTSET_D`] stays the per-axis component offset the corner/disjoint
/// geometry was tuned against.
const SQRT_2: f32 = std::f32::consts::SQRT_2;

// ---------------------------------------------------------------------------
// GizmoHit
// ---------------------------------------------------------------------------

/// Which gizmo element the pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoHit {
    /// No handle hit.
    None,
    /// Inside the quad — translate gesture.
    Translate,
    /// Corner scale handles.
    ScaleNW,
    ScaleNE,
    ScaleSE,
    ScaleSW,
    /// Edge-midpoint scale handles.
    ScaleTop,
    ScaleBottom,
    ScaleLeft,
    ScaleRight,
    /// Rotation zone (diagonal, just outside a corner).
    Rotate,
}

// ---------------------------------------------------------------------------
// Screen-space helpers
// ---------------------------------------------------------------------------

/// Convert a canvas-space point to screen space.
fn canvas_to_screen(
    cx: f32,
    cy: f32,
    origin: egui::Pos2,
    pan: (i32, i32),
    scale: f32,
) -> egui::Pos2 {
    egui::pos2(
        origin.x + pan.0 as f32 + cx * scale,
        origin.y + pan.1 as f32 + cy * scale,
    )
}

/// Average of the four canvas-space corners — the quad centre.
fn quad_center(corners: [(f32, f32); 4]) -> (f32, f32) {
    (
        (corners[0].0 + corners[1].0 + corners[2].0 + corners[3].0) * 0.25,
        (corners[0].1 + corners[1].1 + corners[2].1 + corners[3].1) * 0.25,
    )
}

/// Map the four canvas-space corners to screen space.
fn screen_corners(
    corners: [(f32, f32); 4],
    origin: egui::Pos2,
    pan: (i32, i32),
    scale: f32,
) -> [egui::Pos2; 4] {
    [
        canvas_to_screen(corners[0].0, corners[0].1, origin, pan, scale),
        canvas_to_screen(corners[1].0, corners[1].1, origin, pan, scale),
        canvas_to_screen(corners[2].0, corners[2].1, origin, pan, scale),
        canvas_to_screen(corners[3].0, corners[3].1, origin, pan, scale),
    ]
}

/// The outward direction of each corner: `normalize(corner − centre) · √2`.
///
/// The √2 factor preserves the legacy diagonal magnitude so [`ROT_OUTSET_D`]
/// remains the per-axis component offset the corner/rotation-zone geometry was
/// tuned against (an axis-aligned square gives exactly `(±1, ±1)` per corner).
fn corner_dirs(corners: [(f32, f32); 4]) -> [(f32, f32); 4] {
    let c = quad_center(corners);
    let mut out = [(0.0, 0.0); 4];
    for (i, dir) in out.iter_mut().enumerate() {
        let v = (corners[i].0 - c.0, corners[i].1 - c.1);
        let len = v.0.hypot(v.1);
        *dir = if len <= 1e-9 {
            (0.0, 0.0)
        } else {
            (v.0 / len * SQRT_2, v.1 / len * SQRT_2)
        };
    }
    out
}

/// Point-in-quad for a convex quad given in clockwise order (screen space).
/// Boundary points count as inside; a degenerate quad returns `false`.
fn point_in_quad(p: egui::Pos2, q: [egui::Pos2; 4]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let a = q[i];
        let b = q[(i + 1) % 4];
        let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
        if cross.abs() > 1e-6 {
            let s = cross.signum();
            if sign == 0.0 {
                sign = s;
            } else if s != sign {
                return false;
            }
        }
    }
    sign != 0.0
}

/// An axis-aligned rect in screen space paired with its handle identity.
struct ScreenHandle {
    /// Hit-test rectangle. May be larger than the drawn handle (see
    /// [`HANDLE_HIT_PX`]).
    hit_rect: egui::Rect,
    /// Painted rectangle, when the handle is visible. `None` for the invisible
    /// hit zones (the rotation zones).
    draw_rect: Option<egui::Rect>,
    hit: GizmoHit,
}

/// Build all screen-space handle rects for the current gizmo quad.
///
/// Order matters — it IS the hit-test priority: rotation zones first, then
/// the corner squares, then the edge squares. The translate interior is tested
/// separately, after every handle. There is no pivot handle: the centre mark
/// is read-only and hit-tests as the translate interior.
fn build_handles(
    corners: [(f32, f32); 4],
    origin: egui::Pos2,
    pan: (i32, i32),
    scale: f32,
) -> Vec<ScreenHandle> {
    let hs = HANDLE_PX;
    let hit = HANDLE_HIT_PX;
    let p = screen_corners(corners, origin, pan, scale);
    let dirs = corner_dirs(corners);

    let mut handles = Vec::with_capacity(12);

    // 1. Rotation zones (priority 1): 17×17 hit squares placed
    //    `ROT_OUTSET_D` out along each corner's outward diagonal. Not painted.
    //    Placed so they are strictly disjoint from the corner hit squares.
    for i in 0..4 {
        let d = dirs[i];
        let zone_center = egui::pos2(p[i].x + d.0 * ROT_OUTSET_D, p[i].y + d.1 * ROT_OUTSET_D);
        handles.push(ScreenHandle {
            hit_rect: egui::Rect::from_center_size(
                zone_center,
                egui::vec2(ROT_ZONE_PX, ROT_ZONE_PX),
            ),
            draw_rect: None,
            hit: GizmoHit::Rotate,
        });
    }

    // 2. Corner scale handles (priority 2): 15×15 hit, 8×8 drawn, at the
    //    rotated quad corners.
    for (i, hit_kind) in [
        (0usize, GizmoHit::ScaleNW),
        (1, GizmoHit::ScaleNE),
        (2, GizmoHit::ScaleSE),
        (3, GizmoHit::ScaleSW),
    ] {
        handles.push(ScreenHandle {
            hit_rect: egui::Rect::from_center_size(p[i], egui::vec2(hit, hit)),
            draw_rect: Some(egui::Rect::from_center_size(p[i], egui::vec2(hs, hs))),
            hit: hit_kind,
        });
    }

    // 3. Edge scale handles (priority 3): 15×15 hit, 8×8 drawn squares on the
    //    midpoints of consecutive corners.
    for (i, hit_kind) in [
        (0usize, GizmoHit::ScaleTop),
        (1, GizmoHit::ScaleRight),
        (2, GizmoHit::ScaleBottom),
        (3, GizmoHit::ScaleLeft),
    ] {
        let a = p[i];
        let b = p[(i + 1) % 4];
        let mid = egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
        handles.push(ScreenHandle {
            hit_rect: egui::Rect::from_center_size(mid, egui::vec2(hit, hit)),
            draw_rect: Some(egui::Rect::from_center_size(mid, egui::vec2(hs, hs))),
            hit: hit_kind,
        });
    }

    handles
}

// ---------------------------------------------------------------------------
// Hit-testing (D55)
// ---------------------------------------------------------------------------

/// Hit-test a screen-space position against the gizmo quad's handles.
///
/// Priority order (spec): rotation zones → corner squares → edge squares,
/// then the translate interior last as a catch-all (a point-in-quad test on
/// the rotated corners). Returns [`GizmoHit::None`] when the pointer is
/// outside all handles and the quad.
pub fn hit_test(
    screen_pos: egui::Pos2,
    corners: [(f32, f32); 4],
    origin: egui::Pos2,
    pan: (i32, i32),
    scale: f32,
) -> GizmoHit {
    let handles = build_handles(corners, origin, pan, scale);

    // Test specific handles first, in the priority order they were built.
    for h in &handles {
        if h.hit_rect.contains(screen_pos) {
            return h.hit;
        }
    }

    // Test the quad interior (translate) last.
    let quad = screen_corners(corners, origin, pan, scale);
    if point_in_quad(screen_pos, quad) {
        return GizmoHit::Translate;
    }

    GizmoHit::None
}

/// Colors for the transform gizmo overlay, sourced from the active theme.
///
/// The App shell builds this from [`crate::ui::theme::ThemeColors`] tokens
/// (`gizmo_outline`, `gizmo_fill_normal`, `gizmo_fill_hover`, `gizmo_pivot`);
/// the [`Default`] values mirror the pre-theme hardcoded palette so the gizmo
/// stays self-contained for headless use and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GizmoColors {
    /// Quad outline and handle strokes.
    pub outline: egui::Color32,
    /// Fill of scale handles in their normal state.
    pub fill_normal: egui::Color32,
    /// Fill of the hovered handle.
    pub fill_hover: egui::Color32,
    /// Centre-mark crosshair color.
    pub pivot: egui::Color32,
}

impl Default for GizmoColors {
    fn default() -> Self {
        Self {
            outline: egui::Color32::from_rgb(255, 255, 255),
            fill_normal: egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180),
            fill_hover: egui::Color32::from_rgb(100, 180, 255),
            pivot: egui::Color32::from_rgb(255, 100, 100),
        }
    }
}

// ---------------------------------------------------------------------------
// Rotation-hint L
// ---------------------------------------------------------------------------

/// Pure geometry for the L-shaped rotation hint at one corner.
///
/// `corner` is the corner's SCREEN position and `dir` its outward direction
/// (`normalize(corner − centre) · √2`, the same value [`build_handles`] uses).
/// Returns the 3 screen-space points `[arm, bend, arm]` of the corner bracket:
/// two perpendicular segments meeting at `bend`.
///
/// The bracket is a `2 * ROT_L_HALF_PX` square centred on the corner's rotation
/// zone (`corner + dir * ROT_OUTSET_D`) and rotated so its bend points
/// outward. For an axis-aligned bbox the bracket frame is exactly the screen
/// x/y axes (so it reproduces the pre-quad geometry); for a rotated quad the
/// whole bracket rotates with the content.
pub(crate) fn rotate_zone_l(corner: egui::Pos2, dir: (f32, f32)) -> [egui::Pos2; 3] {
    let len = dir.0.hypot(dir.1);
    let (ux, uy) = if len <= 1e-9 {
        (0.0, 0.0)
    } else {
        (dir.0 / len, dir.1 / len)
    };
    let zone = egui::pos2(
        corner.x + dir.0 * ROT_OUTSET_D,
        corner.y + dir.1 * ROT_OUTSET_D,
    );
    // The bend is the outermost point of the bracket.
    let bend = egui::pos2(
        zone.x + dir.0 * ROT_L_HALF_PX,
        zone.y + dir.1 * ROT_L_HALF_PX,
    );
    // The bracket's two arm axes are the unit outward rotated by ±45°; for a
    // diagonal `dir` these are exactly the screen x/y axes, matching the
    // pre-quad L exactly.
    const INV_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let a = egui::vec2((ux - uy) * INV_SQRT2, (ux + uy) * INV_SQRT2);
    let b = egui::vec2((ux + uy) * INV_SQRT2, (uy - ux) * INV_SQRT2);
    let arm_a = egui::pos2(
        bend.x - 2.0 * ROT_L_HALF_PX * a.x,
        bend.y - 2.0 * ROT_L_HALF_PX * a.y,
    );
    let arm_b = egui::pos2(
        bend.x - 2.0 * ROT_L_HALF_PX * b.x,
        bend.y - 2.0 * ROT_L_HALF_PX * b.y,
    );
    [arm_a, bend, arm_b]
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// Paint the gizmo overlay onto the egui painter.
///
/// Draws the quad outline as a thin DASHED perimeter (a ~4 px dash / ~4 px
/// gap, 1 px `colors.outline`), the four corner scale handles and the four
/// edge scale handles (filled squares), the four L-shaped rotation hints, and
/// a read-only centre mark (a crosshair in `colors.pivot`) at `pivot`. The
/// centre mark is NOT interactive: it just shows the rotation/scale centre.
///
/// Hover is animated: each handle fades between `colors.fill_normal` and
/// `colors.fill_hover` over [`HOVER_ANIM_SECS`] using
/// [`egui::Context::animate_bool_with_time`] under a stable, per-handle
/// [`egui::Id`], giving a soft ~100 ms transition.
///
/// `hovered` is the handle the pointer is over. `active` is the handle
/// currently being dragged (`None` when idle); an active handle is painted
/// with a distinct emphasis — the fully-opaque hover color brightened — that
/// takes visual precedence over hover. Because all four rotation zones share
/// [`GizmoHit::Rotate`], hovering or dragging one emphasises all four (active
/// over hover). All colors come from `colors` (built from the active theme by
/// the App shell; [`GizmoColors::default`] mirrors the pre-theme palette).
pub fn paint(
    painter: &egui::Painter,
    corners: [(f32, f32); 4],
    pivot: (f32, f32),
    origin: egui::Pos2,
    pan: (i32, i32),
    scale: f32,
    hovered: GizmoHit,
    active: Option<GizmoHit>,
    colors: GizmoColors,
) {
    let handles = build_handles(corners, origin, pan, scale);
    let p = screen_corners(corners, origin, pan, scale);
    let dirs = corner_dirs(corners);

    let stroke = egui::Stroke::new(1.0, colors.outline);
    let fill_normal = colors.fill_normal;
    let fill_hover = colors.fill_hover;

    // Quad outline: a thin dashed closed perimeter (4 px dash / 4 px gap)
    // rather than a solid stroke, so the gizmo reads as a guide overlay and
    // lets the underlying pixels show through.
    let mut dashes = Vec::new();
    egui::Shape::dashed_line_many(
        &[p[0], p[1], p[2], p[3], p[0]],
        stroke,
        BBOX_DASH_PX,
        BBOX_DASH_PX,
        &mut dashes,
    );
    painter.extend(dashes);

    // L-shaped rotation hints at the four rotation zones (one per corner,
    // oriented along the rotated outward diagonal). These are a VISUAL
    // affordance only — the hit geometry is unchanged (`GizmoHit::Rotate`
    // covers all four zones, tested before the corners). Because there is a
    // single `Rotate` variant, hovering or dragging any one zone emphasises all
    // four hints; `active` takes precedence over hover.
    let l_color = if active == Some(GizmoHit::Rotate) {
        // Same brightened emphasis the active scale handles use.
        fill_hover.to_opaque().gamma_multiply(1.35)
    } else if hovered == GizmoHit::Rotate {
        fill_hover
    } else {
        colors.outline
    };
    let l_stroke = egui::Stroke::new(1.0, l_color);
    for i in 0..4 {
        let [a, bend, c] = rotate_zone_l(p[i], dirs[i]);
        painter.line_segment([a, bend], l_stroke);
        painter.line_segment([bend, c], l_stroke);
    }

    // Corner + edge scale handles (rotation zones are not painted as squares;
    // the centre mark is a separate crosshair below). The painted rect is the
    // (smaller) draw_rect; the hit target may extend past it.
    //
    // Hover/animation: egui is immediate-mode, so the fade is stored in the
    // context memory keyed by a stable per-handle id; `animate_bool_with_time`
    // returns a 0→1 factor we LERP across. `active` wins over hover.
    for (idx, h) in handles.iter().enumerate() {
        let Some(draw) = h.draw_rect else {
            continue;
        };

        let is_hovered = h.hit == hovered;
        let anim_id = egui::Id::new(("gizmo_hover", idx));
        let t = painter
            .ctx()
            .animate_bool_with_time(anim_id, is_hovered, HOVER_ANIM_SECS);
        let hover_fill = fill_normal.lerp_to_gamma(fill_hover, t);

        let is_active = active == Some(h.hit);
        let (fill, size) = if is_active {
            // Distinct emphasis derived from the existing tokens: the hover
            // color, fully opaque and brightened along with a small size bump.
            let emphasis = fill_hover.to_opaque().gamma_multiply(1.35);
            (emphasis, HANDLE_PX + 2.0)
        } else {
            (hover_fill, HANDLE_PX)
        };

        let rect = if is_active {
            egui::Rect::from_center_size(draw.center(), egui::vec2(size, size))
        } else {
            draw
        };

        painter.rect_filled(rect, 0, fill);
        painter.rect_stroke(rect, 0, stroke, egui::StrokeKind::Inside);
    }

    // Read-only centre mark, visually distinct from the scale handles. It is
    // NOT a hit target: `hit_test` never returns a pivot handle.
    let pivot_screen = canvas_to_screen(pivot.0, pivot.1, origin, pan, scale);
    let cross = egui::Stroke::new(1.0, colors.pivot);
    painter.line_segment(
        [
            egui::pos2(pivot_screen.x - PIVOT_ARM_PX, pivot_screen.y),
            egui::pos2(pivot_screen.x + PIVOT_ARM_PX, pivot_screen.y),
        ],
        cross,
    );
    painter.line_segment(
        [
            egui::pos2(pivot_screen.x, pivot_screen.y - PIVOT_ARM_PX),
            egui::pos2(pivot_screen.x, pivot_screen.y + PIVOT_ARM_PX),
        ],
        cross,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- helpers -------------------------------------------------------------

    /// Identity camera (origin 0, pan 0, scale 1).
    fn identity_cam() -> (egui::Pos2, (i32, i32), f32) {
        (egui::pos2(0.0, 0.0), (0, 0), 1.0)
    }

    /// The four clockwise corners `[TL, TR, BR, BL]` of an axis-aligned rect.
    fn rect_corners(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> [(f32, f32); 4] {
        [
            (min_x, min_y),
            (max_x, min_y),
            (max_x, max_y),
            (min_x, max_y),
        ]
    }

    /// A 100×100 square at the origin (corners at the screen-tested positions).
    fn square_corners() -> [(f32, f32); 4] {
        rect_corners(0.0, 0.0, 100.0, 100.0)
    }

    /// Rotate a quad about its centre by `deg` (screen y-down, clockwise).
    fn rotate_quad(corners: [(f32, f32); 4], deg: f32) -> [(f32, f32); 4] {
        let c = quad_center(corners);
        let t = deg.to_radians();
        let (sn, cs) = (t.sin(), t.cos());
        corners.map(|p| {
            let v = (p.0 - c.0, p.1 - c.1);
            (c.0 + cs * v.0 - sn * v.1, c.1 + sn * v.0 + cs * v.1)
        })
    }

    fn screen_of(corners: [(f32, f32); 4], i: usize) -> egui::Pos2 {
        let (o, pan, s) = identity_cam();
        screen_corners(corners, o, pan, s)[i]
    }

    // -- quad geometry -------------------------------------------------------

    #[test]
    fn quad_center_is_the_corner_average() {
        let corners = rect_corners(0.0, 0.0, 10.0, 6.0);
        assert_eq!(quad_center(corners), (5.0, 3.0));
    }

    #[test]
    fn corner_dirs_are_diagonal_for_an_axis_aligned_square() {
        let dirs = corner_dirs(square_corners());
        assert!((dirs[0].0 + 1.0).abs() < 1e-4 && (dirs[0].1 + 1.0).abs() < 1e-4); // TL
        assert!((dirs[1].0 - 1.0).abs() < 1e-4 && (dirs[1].1 + 1.0).abs() < 1e-4); // TR
        assert!((dirs[2].0 - 1.0).abs() < 1e-4 && (dirs[2].1 - 1.0).abs() < 1e-4); // BR
        assert!((dirs[3].0 + 1.0).abs() < 1e-4 && (dirs[3].1 - 1.0).abs() < 1e-4);
        // BL
    }

    // -- hit_test ------------------------------------------------------------

    #[test]
    fn hit_test_inside_quad_returns_translate() {
        let corners = rect_corners(10.0, 10.0, 90.0, 90.0);
        let (o, pan, s) = identity_cam();
        // Inside the quad, away from every handle.
        assert_eq!(
            hit_test(egui::pos2(50.0, 30.0), corners, o, pan, s),
            GizmoHit::Translate,
        );
    }

    #[test]
    fn hit_test_at_nw_corner_returns_scale_nw() {
        let corners = rect_corners(10.0, 10.0, 90.0, 90.0);
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(10.0, 10.0), corners, o, pan, s),
            GizmoHit::ScaleNW,
        );
    }

    #[test]
    fn hit_test_at_ne_corner_returns_scale_ne() {
        let corners = rect_corners(10.0, 10.0, 90.0, 90.0);
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(90.0, 10.0), corners, o, pan, s),
            GizmoHit::ScaleNE,
        );
    }

    #[test]
    fn hit_test_at_edge_midpoint_returns_scale_variant() {
        let corners = rect_corners(10.0, 10.0, 90.0, 90.0);
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(50.0, 10.0), corners, o, pan, s),
            GizmoHit::ScaleTop,
        );
        assert_eq!(
            hit_test(egui::pos2(50.0, 90.0), corners, o, pan, s),
            GizmoHit::ScaleBottom,
        );
        assert_eq!(
            hit_test(egui::pos2(10.0, 50.0), corners, o, pan, s),
            GizmoHit::ScaleLeft,
        );
        assert_eq!(
            hit_test(egui::pos2(90.0, 50.0), corners, o, pan, s),
            GizmoHit::ScaleRight,
        );
    }

    #[test]
    fn hit_test_at_se_corner_returns_scale_se() {
        let corners = rect_corners(0.0, 0.0, 80.0, 60.0);
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(80.0, 60.0), corners, o, pan, s),
            GizmoHit::ScaleSE,
        );
    }

    #[test]
    fn hit_test_outside_quad_returns_none() {
        let corners = rect_corners(10.0, 10.0, 90.0, 90.0);
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(200.0, 200.0), corners, o, pan, s),
            GizmoHit::None,
        );
    }

    #[test]
    fn hit_test_with_pan_and_zoom() {
        let corners = rect_corners(10.0, 10.0, 20.0, 20.0);
        let origin = egui::pos2(100.0, 100.0);
        let pan = (5, 5);
        let scale = 2.0;
        // Canvas (20,20) → screen 100 + 5 + 20*2 = 145.
        assert_eq!(
            hit_test(egui::pos2(145.0, 145.0), corners, origin, pan, scale),
            GizmoHit::ScaleSE,
        );
        // Canvas (15,15) → screen 135, well inside → translate.
        assert_eq!(
            hit_test(egui::pos2(135.0, 135.0), corners, origin, pan, scale),
            GizmoHit::Translate,
        );
    }

    #[test]
    fn hit_test_rotation_zone_off_corner_and_corner_on_corner() {
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        // (119, −19): 19 px out along the NE diagonal, inside the 17×17 zone.
        assert_eq!(
            hit_test(egui::pos2(119.0, -19.0), corners, o, pan, s),
            GizmoHit::Rotate,
        );
        // A point 1 px off the corner stays on the corner handle.
        assert_eq!(
            hit_test(egui::pos2(101.0, -1.0), corners, o, pan, s),
            GizmoHit::ScaleNE,
        );
    }

    #[test]
    fn hit_test_rotation_zones_all_four_corners() {
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(119.0, -19.0), corners, o, pan, s),
            GizmoHit::Rotate,
        ); // NE
        assert_eq!(
            hit_test(egui::pos2(-19.0, -19.0), corners, o, pan, s),
            GizmoHit::Rotate,
        ); // NW
        assert_eq!(
            hit_test(egui::pos2(119.0, 119.0), corners, o, pan, s),
            GizmoHit::Rotate,
        ); // SE
        assert_eq!(
            hit_test(egui::pos2(-19.0, 119.0), corners, o, pan, s),
            GizmoHit::Rotate,
        ); // SW
    }

    #[test]
    fn hit_test_edge_midpoints_scale() {
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(50.0, 0.0), corners, o, pan, s),
            GizmoHit::ScaleTop,
        );
        assert_eq!(
            hit_test(egui::pos2(50.0, 100.0), corners, o, pan, s),
            GizmoHit::ScaleBottom,
        );
        assert_eq!(
            hit_test(egui::pos2(0.0, 50.0), corners, o, pan, s),
            GizmoHit::ScaleLeft,
        );
        assert_eq!(
            hit_test(egui::pos2(100.0, 50.0), corners, o, pan, s),
            GizmoHit::ScaleRight,
        );
        // 9 px below the top edge handle is past the 15×15 hit square, so the
        // translate interior wins.
        assert_eq!(
            hit_test(egui::pos2(50.0, 9.0), corners, o, pan, s),
            GizmoHit::Translate,
        );
    }

    #[test]
    fn hit_test_quad_centre_returns_translate_not_a_pivot_handle() {
        // M1: the centre mark is read-only. A point exactly at the quad centre
        // (where the pivot sits) is the translate interior, never a handle.
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(50.0, 50.0), corners, o, pan, s),
            GizmoHit::Translate,
        );
    }

    #[test]
    fn hit_test_rotation_zone_does_not_overlap_corner() {
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        assert_eq!(
            hit_test(egui::pos2(100.0, 0.0), corners, o, pan, s),
            GizmoHit::ScaleNE,
        );
        assert_eq!(
            hit_test(egui::pos2(119.0, -19.0), corners, o, pan, s),
            GizmoHit::Rotate,
        );
    }

    /// O1/angle-0 no-regression: all four transformed positions resolve through
    /// the rotated quad. Rotating a square by 30° moves every handle; the
    /// hit-test must follow the ROTATED corner/edge/midpoint positions (not the
    /// old axis-aligned bbox).
    #[test]
    fn hit_test_resolves_a_rotated_quad_corners_and_edges() {
        let corners = rotate_quad(square_corners(), 30.0);
        let (o, pan, s) = identity_cam();
        // The four rotated corners.
        for (i, expected) in [
            (0usize, GizmoHit::ScaleNW),
            (1, GizmoHit::ScaleNE),
            (2, GizmoHit::ScaleSE),
            (3, GizmoHit::ScaleSW),
        ] {
            let c = screen_of(corners, i);
            assert_eq!(
                hit_test(c, corners, o, pan, s),
                expected,
                "rotated corner {i} at {c:?}"
            );
        }
        // The four rotated edge midpoints.
        for (i, expected) in [
            (0usize, GizmoHit::ScaleTop),
            (1, GizmoHit::ScaleRight),
            (2, GizmoHit::ScaleBottom),
            (3, GizmoHit::ScaleLeft),
        ] {
            let a = screen_of(corners, i);
            let b = screen_of(corners, (i + 1) % 4);
            let mid = egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
            assert_eq!(
                hit_test(mid, corners, o, pan, s),
                expected,
                "rotated edge {i} midpoint at {mid:?}"
            );
        }
        // The centre is the translate interior.
        let c = quad_center(corners);
        assert_eq!(
            hit_test(egui::pos2(c.0, c.1), corners, o, pan, s),
            GizmoHit::Translate,
        );
        // The former (unrotated) NE corner is OUTSIDE the rotated quad now.
        assert_ne!(
            hit_test(egui::pos2(100.0, 0.0), corners, o, pan, s),
            GizmoHit::ScaleNE,
            "the gizmo must NOT resolve the stale unrotated corner"
        );
    }

    #[test]
    fn corner_hit_square_and_rotation_zones_are_strictly_disjoint() {
        // The real geometry: every corner hit square must not intersect ANY
        // rotation zone. This guards the reported corner-grab failure where
        // the zone's rim overlapped the corner square and, because zones are
        // tested first, a corner press returned `Rotate` instead of a scale.
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        let handles = build_handles(corners, o, pan, s);
        let corner_rects: Vec<egui::Rect> = handles
            .iter()
            .filter(|h| {
                matches!(
                    h.hit,
                    GizmoHit::ScaleNW | GizmoHit::ScaleNE | GizmoHit::ScaleSE | GizmoHit::ScaleSW
                )
            })
            .map(|h| h.hit_rect)
            .collect();
        let rotate_rects: Vec<egui::Rect> = handles
            .iter()
            .filter(|h| h.hit == GizmoHit::Rotate)
            .map(|h| h.hit_rect)
            .collect();
        assert_eq!(corner_rects.len(), 4);
        assert_eq!(rotate_rects.len(), 4);
        for (ci, corner) in corner_rects.iter().enumerate() {
            for (ri, zone) in rotate_rects.iter().enumerate() {
                assert!(
                    !corner.intersects(*zone),
                    "corner #{ci} {corner:?} overlaps rotation zone #{ri} {zone:?}"
                );
            }
        }
    }

    #[test]
    fn hit_sizes_and_rotation_geometry_match_new_spec() {
        // Regression guard for the Task J1 geometry change: 15 px corner hit,
        // 17 px rotation zone, 18 px outward offset, and exactly a 2 px gap
        // between the corner hit square and the zone at any zoom.
        assert_eq!(HANDLE_HIT_PX, 15.0);
        assert_eq!(HANDLE_PX, 8.0);
        assert_eq!(ROT_ZONE_PX, 17.0);
        assert_eq!(ROT_OUTSET_D, 18.0);
        assert_eq!(ROT_GAP_PX, 2.0);
        // The gap identity: inner edge of the zone minus corner half-width.
        let gap = (ROT_OUTSET_D - ROT_ZONE_PX * 0.5) - HANDLE_HIT_PX * 0.5;
        assert_eq!(gap, ROT_GAP_PX);
    }

    #[test]
    fn build_handles_hit_rects_use_new_sizes() {
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        let handles = build_handles(corners, o, pan, s);

        for h in &handles {
            match h.hit {
                GizmoHit::ScaleNW
                | GizmoHit::ScaleNE
                | GizmoHit::ScaleSE
                | GizmoHit::ScaleSW
                | GizmoHit::ScaleTop
                | GizmoHit::ScaleBottom
                | GizmoHit::ScaleLeft
                | GizmoHit::ScaleRight => {
                    assert_eq!(h.hit_rect.width(), HANDLE_HIT_PX, "corner/edge hit width");
                    assert_eq!(h.hit_rect.height(), HANDLE_HIT_PX, "corner/edge hit height");
                    let draw = h.draw_rect.expect("scale handles are painted");
                    assert_eq!(draw.width(), HANDLE_PX, "drawn handle stays 8 px");
                    assert_eq!(draw.height(), HANDLE_PX, "drawn handle stays 8 px");
                }
                GizmoHit::Rotate => {
                    assert_eq!(h.hit_rect.width(), ROT_ZONE_PX, "rotation zone width");
                    assert_eq!(h.hit_rect.height(), ROT_ZONE_PX, "rotation zone height");
                    assert!(h.draw_rect.is_none(), "rotation zones are unpainted");
                }
                _ => {}
            }
        }
    }

    // -- paint ---------------------------------------------------------------

    /// One frame of `paint` on an offscreen egui context, returning the frame's
    /// emitted shapes.
    fn paint_shapes(
        corners: [(f32, f32); 4],
        pivot: (f32, f32),
        hovered: GizmoHit,
        active: Option<GizmoHit>,
    ) -> Vec<egui::Shape> {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            paint(
                painter,
                corners,
                pivot,
                egui::pos2(0.0, 0.0),
                (0, 0),
                1.0,
                hovered,
                active,
                GizmoColors::default(),
            );
        });
        // The font/texture deltas are irrelevant to a geometry/color assert and
        // must be released before the output is dropped.
        output.textures_delta.clear();
        output.shapes.into_iter().map(|c| c.shape).collect()
    }

    /// Recursively collect every line segment and every rect fill from a shape
    /// tree, paired with their stroke/fill color.
    fn collect(
        shape: &egui::Shape,
        segments: &mut Vec<([egui::Pos2; 2], egui::Color32)>,
        fills: &mut Vec<(egui::Rect, egui::Color32)>,
    ) {
        match shape {
            egui::Shape::LineSegment { points, stroke } => {
                segments.push((*points, stroke.color));
            }
            egui::Shape::Rect(r) => {
                if r.fill != egui::Color32::TRANSPARENT {
                    fills.push((r.rect, r.fill));
                }
            }
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect(s, segments, fills);
                }
            }
            _ => {}
        }
    }

    fn split(
        shapes: &[egui::Shape],
    ) -> (
        Vec<([egui::Pos2; 2], egui::Color32)>,
        Vec<(egui::Rect, egui::Color32)>,
    ) {
        let mut segments = Vec::new();
        let mut fills = Vec::new();
        for s in shapes {
            collect(s, &mut segments, &mut fills);
        }
        (segments, fills)
    }

    #[test]
    fn paint_outline_is_dashed_not_a_solid_rect() {
        let corners = square_corners();
        let shapes = paint_shapes(corners, (50.0, 50.0), GizmoHit::None, None);
        let (segments, _fills) = split(&shapes);

        // A solid rect stroke would be one `Rect` shape; the dashed perimeter
        // is instead many short line segments. A 100×100 quad with a 4 px
        // dash + 4 px gap yields well over a dozen dash segments.
        let short_dashes: Vec<_> = segments
            .iter()
            .filter(|(p, _)| (p[1] - p[0]).length() <= BBOX_DASH_PX + 1.0)
            .collect();
        assert!(
            short_dashes.len() >= 12,
            "expected many dash segments, got {}",
            short_dashes.len()
        );
        // No segment spans a whole edge (the solid stroke we replaced) — the
        // only longer segments are the L-hint arms / centre crosshair arms.
        assert!(
            segments
                .iter()
                .all(|(p, _)| (p[1] - p[0]).length() <= PIVOT_ARM_PX * 2.0 + 1.0),
            "found a line longer than a crosshair arm (a solid edge slipped through)"
        );
    }

    #[test]
    fn paint_active_handle_differs_from_normal_handle() {
        let corners = square_corners();
        let pivot = (50.0, 50.0);

        let normal_shapes = paint_shapes(corners, pivot, GizmoHit::None, None);
        let active_shapes = paint_shapes(corners, pivot, GizmoHit::None, Some(GizmoHit::ScaleNW));

        let (_, normal_fills) = split(&normal_shapes);
        let (_, active_fills) = split(&active_shapes);

        // Same number of handle fills in both frames (the active square only
        // changes color/size, not count).
        assert_eq!(normal_fills.len(), active_fills.len());

        // The NW corner sits at screen (0, 0). Find each frame's fill centered
        // there and confirm the active one is a different color.
        let at_nw = |fills: &[(egui::Rect, egui::Color32)]| {
            fills
                .iter()
                .find(|(r, _)| (r.center() - egui::pos2(0.0, 0.0)).length() < 0.01)
                .map(|(_, c)| *c)
                .expect("NW handle fill present")
        };
        let normal_nw = at_nw(&normal_fills);
        let active_nw = at_nw(&active_fills);

        assert_ne!(
            normal_nw, active_nw,
            "active handle must be painted with a distinct emphasis color"
        );

        // The other (non-active) handles keep the normal fill.
        let ne_normal = normal_fills
            .iter()
            .find(|(r, _)| (r.center() - egui::pos2(100.0, 0.0)).length() < 0.01)
            .map(|(_, c)| *c)
            .unwrap();
        let ne_active = active_fills
            .iter()
            .find(|(r, _)| (r.center() - egui::pos2(100.0, 0.0)).length() < 0.01)
            .map(|(_, c)| *c)
            .unwrap();
        assert_eq!(ne_normal, ne_active, "non-active handles are unchanged");
    }

    #[test]
    fn paint_active_precedence_over_hover() {
        let corners = square_corners();
        let pivot = (50.0, 50.0);

        // A fresh animation starts at 0 for the hovered handle, so its color
        // equals the normal fill. The active handle must still stand out.
        let active_only = paint_shapes(corners, pivot, GizmoHit::None, Some(GizmoHit::ScaleNW));
        let hover_and_active =
            paint_shapes(corners, pivot, GizmoHit::ScaleNW, Some(GizmoHit::ScaleNW));

        let (_, a) = split(&active_only);
        let (_, b) = split(&hover_and_active);
        let at_nw = |fills: &[(egui::Rect, egui::Color32)]| {
            fills
                .iter()
                .find(|(r, _)| (r.center() - egui::pos2(0.0, 0.0)).length() < 0.01)
                .map(|(_, c)| *c)
                .unwrap()
        };
        assert_eq!(
            at_nw(&a),
            at_nw(&b),
            "active emphasis must win over hover on the same handle"
        );
    }

    /// The corner/edge handle fills follow a ROTATED quad (O1): every painted
    /// fill centre must coincide with a rotated corner or edge midpoint.
    #[test]
    fn paint_handles_follow_a_rotated_quad() {
        let corners = rotate_quad(square_corners(), 35.0);
        let shapes = paint_shapes(corners, (50.0, 50.0), GizmoHit::None, None);
        let (_, fills) = split(&shapes);
        let p = screen_corners(corners, egui::pos2(0.0, 0.0), (0, 0), 1.0);

        // Every handle fill centre is one of the 8 rotated corner/edge points.
        let mut expected: Vec<egui::Pos2> = p.to_vec();
        for i in 0..4 {
            let a = p[i];
            let b = p[(i + 1) % 4];
            expected.push(egui::pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5));
        }
        assert_eq!(fills.len(), 8, "4 corners + 4 edges");
        for (rect, _) in &fills {
            let centre = rect.center();
            assert!(
                expected.iter().any(|e| (*e - centre).length() < 0.01),
                "handle fill at {centre:?} is not on the rotated quad"
            );
        }
        // And the old axis-aligned NE corner is no longer a handle.
        assert!(
            !fills
                .iter()
                .any(|(r, _)| (r.center() - egui::pos2(100.0, 0.0)).length() < 0.01),
            "the stale unrotated corner must not be a handle"
        );
    }

    // -- rotation-hint L -----------------------------------------------------

    #[test]
    fn rotate_zone_l_placement_and_orientation_all_corners() {
        let corners = square_corners();
        let (o, pan, s) = identity_cam();
        let p = screen_corners(corners, o, pan, s);
        let dirs = corner_dirs(corners);

        // (corner index, corner screen pos, outward diagonal)
        let cases = [
            (0usize, egui::pos2(0.0, 0.0), (-1.0, -1.0)),
            (1, egui::pos2(100.0, 0.0), (1.0, -1.0)),
            (2, egui::pos2(100.0, 100.0), (1.0, 1.0)),
            (3, egui::pos2(0.0, 100.0), (-1.0, 1.0)),
        ];

        for (i, c, (dx, dy)) in cases {
            assert!(
                (dirs[i].0 - dx).abs() < 1e-4 && (dirs[i].1 - dy).abs() < 1e-4,
                "corner {i} dir {:?} != ({dx},{dy})",
                dirs[i]
            );
            let [a, bend, b] = rotate_zone_l(p[i], dirs[i]);

            // The zone centre, matching `build_handles`.
            let zone = egui::pos2(c.x + dx * ROT_OUTSET_D, c.y + dy * ROT_OUTSET_D);

            // The bend is the outermost point: zone centre plus the outward
            // half-extent.
            let expected_bend =
                egui::pos2(zone.x + dx * ROT_L_HALF_PX, zone.y + dy * ROT_L_HALF_PX);
            assert!(
                (bend - expected_bend).length() < 1e-3,
                "corner {i}: bend {bend:?} != {expected_bend:?}"
            );

            // The bracket's bounding-box centre is the zone centre.
            let min_x = a.x.min(bend.x).min(b.x);
            let max_x = a.x.max(bend.x).max(b.x);
            let min_y = a.y.min(bend.y).min(b.y);
            let max_y = a.y.max(bend.y).max(b.y);
            let centre = egui::pos2((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
            assert!(
                (centre - zone).length() < 1e-3,
                "corner {i}: bracket centre {centre:?} != zone {zone:?}"
            );

            // The bracket is a 2*HALF square.
            assert!(
                (max_x - min_x - 2.0 * ROT_L_HALF_PX).abs() < 1e-3
                    && (max_y - min_y - 2.0 * ROT_L_HALF_PX).abs() < 1e-3,
                "corner {i}: bracket is not a 2*ROT_L_HALF_PX square"
            );

            // Two segments meeting at the bend are perpendicular.
            let v1 = bend - a;
            let v2 = b - bend;
            assert!(
                v1.dot(v2).abs() < 1e-3,
                "corner {i}: segments are not perpendicular (dot={})",
                v1.dot(v2)
            );
            assert!(v1.length() > 1e-3 && v2.length() > 1e-3);

            // The bend points OUTWARD (farther from the quad centre).
            let centre_bb = egui::pos2(50.0, 50.0);
            assert!(
                (bend - centre_bb).length() > (c - centre_bb).length(),
                "corner {i}: bend must lie outward of the corner"
            );

            // The L sits inside its rotation-zone square (minimal coverage).
            let zone_rect =
                egui::Rect::from_center_size(zone, egui::vec2(ROT_ZONE_PX, ROT_ZONE_PX));
            assert!(
                zone_rect.contains(a) && zone_rect.contains(bend) && zone_rect.contains(b),
                "corner {i}: L escapes its rotation zone"
            );
        }
    }

    #[test]
    fn rotate_zone_l_respects_camera() {
        // Pan/zoom: the L must follow the corner exactly like `build_handles`.
        let corners = rect_corners(10.0, 10.0, 20.0, 20.0);
        let origin = egui::pos2(100.0, 100.0);
        let pan = (5, 5);
        let scale = 2.0;
        // Canvas (20,10) → screen = 100 + 5 + 20*2 = 145, 100 + 5 + 10*2 = 125.
        let p = screen_corners(corners, origin, pan, scale);
        let dirs = corner_dirs(corners);
        // Corner index 1 = TR = canvas (20,10), outward diagonal (1,-1).
        let [a, bend, b] = rotate_zone_l(p[1], dirs[1]);
        let expected_bend = egui::pos2(
            145.0 + ROT_OUTSET_D + ROT_L_HALF_PX,
            125.0 - ROT_OUTSET_D - ROT_L_HALF_PX,
        );
        assert!((bend - expected_bend).length() < 1e-3);
        // The two arms lie on the bracket's edges; both are exactly 2*HALF
        // from the bend and perpendicular to each other.
        assert!(((bend - a).length() - 2.0 * ROT_L_HALF_PX).abs() < 1e-3);
        assert!(((b - bend).length() - 2.0 * ROT_L_HALF_PX).abs() < 1e-3);
        assert!((bend - a).dot(b - bend).abs() < 1e-3);
    }

    /// All 8 expected L segments (4 corners × 2) for `corners` under identity cam.
    fn expected_l_segments(corners: [(f32, f32); 4]) -> Vec<[egui::Pos2; 2]> {
        let (o, pan, s) = identity_cam();
        let p = screen_corners(corners, o, pan, s);
        let dirs = corner_dirs(corners);
        let mut out = Vec::new();
        for i in 0..4 {
            let [a, bend, b] = rotate_zone_l(p[i], dirs[i]);
            out.push([a, bend]);
            out.push([bend, b]);
        }
        out
    }

    fn same_segment(p: [egui::Pos2; 2], q: [egui::Pos2; 2]) -> bool {
        (p[0] - q[0]).length() < 1e-3 && (p[1] - q[1]).length() < 1e-3
    }

    #[test]
    fn paint_draws_four_l_hints_with_normal_color() {
        let corners = square_corners();
        let shapes = paint_shapes(corners, (50.0, 50.0), GizmoHit::None, None);
        let (segments, _fills) = split(&shapes);
        let outline = GizmoColors::default().outline;

        // Every expected L segment is emitted with the normal outline color,
        // exactly once.
        for expected in expected_l_segments(corners) {
            let hits = segments
                .iter()
                .filter(|(p, c)| same_segment(*p, expected) && *c == outline)
                .count();
            assert_eq!(hits, 1, "missing/duplicated L segment {expected:?}");
        }

        // The four bends sit in the four distinct quadrants around the centre.
        let bends: Vec<egui::Pos2> = (0..4)
            .map(|i| {
                let (o, pan, s) = identity_cam();
                let p = screen_corners(corners, o, pan, s);
                let dirs = corner_dirs(corners);
                rotate_zone_l(p[i], dirs[i])[1]
            })
            .collect();
        assert_eq!(bends.len(), 4);
        for (i, bx) in bends.iter().enumerate() {
            for by in bends.iter().skip(i + 1) {
                assert!(
                    (bx.x - by.x).signum() != 0.0 && (bx.y - by.y).signum() != 0.0,
                    "bends should occupy distinct quadrants"
                );
            }
        }
    }

    #[test]
    fn paint_l_hints_emphasise_on_hover_and_active() {
        let corners = square_corners();
        let colors = GizmoColors::default();
        let expected = expected_l_segments(corners);

        // Color of each expected L segment in a given frame.
        let l_colors = |shapes: &[egui::Shape]| -> Vec<egui::Color32> {
            let (segments, _) = split(shapes);
            expected
                .iter()
                .map(|exp| {
                    segments
                        .iter()
                        .find(|(p, _)| same_segment(*p, *exp))
                        .map(|(_, c)| *c)
                        .expect("L segment present")
                })
                .collect()
        };

        let normal = l_colors(&paint_shapes(corners, (50.0, 50.0), GizmoHit::None, None));
        let hovered = l_colors(&paint_shapes(corners, (50.0, 50.0), GizmoHit::Rotate, None));
        let active = l_colors(&paint_shapes(
            corners,
            (50.0, 50.0),
            GizmoHit::None,
            Some(GizmoHit::Rotate),
        ));

        assert_eq!(normal.len(), 8);
        assert_eq!(hovered.len(), 8);
        assert_eq!(active.len(), 8);

        // Normal: outline. Hover: fill_hover. Active: brightened emphasis.
        assert!(normal.iter().all(|c| *c == colors.outline));
        assert!(hovered.iter().all(|c| *c == colors.fill_hover));
        let emphasis = colors.fill_hover.to_opaque().gamma_multiply(1.35);
        assert!(active.iter().all(|c| *c == emphasis));
        assert_ne!(normal[0], hovered[0]);
        assert_ne!(hovered[0], active[0]);

        // Active wins over hover when both name Rotate.
        let both = l_colors(&paint_shapes(
            corners,
            (50.0, 50.0),
            GizmoHit::Rotate,
            Some(GizmoHit::Rotate),
        ));
        assert!(
            both.iter().all(|c| *c == emphasis),
            "active emphasis must win over hover for the rotation hints"
        );
    }
}
