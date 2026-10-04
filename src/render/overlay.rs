//! Overlay computation — onion-skin ghosts and marching-ants dash geometry.
//!
//! Pure geometry + logic: no wgpu, no egui. Core types only ([`Rect2i`],
//! [`Color`], [`OnionConfig`]), fully unit-testable. The UI layer converts
//! these into painted shapes.

use crate::core::anim::OnionConfig;
use crate::core::color::Color;
use crate::core::math::Rect2i;

/// One tinted region to draw as an onion-skin ghost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnionGhost {
    pub rect: Rect2i,
    pub tint: Color,
}

/// Computes the onion-skin ghosts for `current` in `regions`.
///
/// Deterministic, nearest-first: previous frames `current-1, current-2, …`
/// (bounded by [`OnionConfig::prev_frames`]), then next frames
/// `current+1, current+2, …` (bounded by [`OnionConfig::next_frames`]).
/// Each region is clipped to `canvas`; empty results are skipped. No
/// wrap-around (linear, like Aseprite). Order within the vec: all prevs then
/// all nexts, nearest prev first, nearest next first.
///
/// Returns an empty vec when onion is disabled, `regions` is empty, or
/// `current` is out of range.
pub fn onion_ghosts(
    regions: &[Rect2i],
    current: usize,
    cfg: OnionConfig,
    canvas: Rect2i,
) -> Vec<OnionGhost> {
    if !cfg.has_onion() || regions.is_empty() || current >= regions.len() {
        return Vec::new();
    }
    let mut ghosts = Vec::new();
    let prev_count = cfg.prev_frames() as usize;
    let next_count = cfg.next_frames() as usize;
    // Previous frames, nearest first: current-1, current-2, …
    for i in 1..=prev_count {
        if current < i {
            break;
        }
        let clipped = regions[current - i].clamp_to(canvas);
        if !clipped.is_empty() {
            ghosts.push(OnionGhost {
                rect: clipped,
                tint: cfg.prev_tint(),
            });
        }
    }
    // Next frames, nearest first: current+1, current+2, …
    for i in 1..=next_count {
        let idx = current + i;
        if idx >= regions.len() {
            break;
        }
        let clipped = regions[idx].clamp_to(canvas);
        if !clipped.is_empty() {
            ghosts.push(OnionGhost {
                rect: clipped,
                tint: cfg.next_tint(),
            });
        }
    }
    ghosts
}

/// Marching-ants dash length in canvas pixels.
pub const ANT_DASH_PX: f32 = 3.0;
/// Marching-ants gap length in canvas pixels.
pub const ANT_GAP_PX: f32 = 3.0;
/// Marching-ants animation speed in canvas pixels per millisecond.
///
/// Deliberately restrained (3 px/s): the 6 px dash+gap period takes **2 seconds**
/// for one full rotation, so the dashes read as a gentle drift rather than a
/// blink. Fast/high-contrast border animation is a photosensitivity hazard,
/// so the speed and the border's alpha ([`ANT_STROKE_ALPHA`]) are both kept
/// low; brightness never changes at all.
pub const ANT_SPEED_PX_PER_MS: f32 = 0.003;

/// Mask marching-ants dash length in canvas pixels.
///
/// Deliberately smaller than the rectangular [`ANT_DASH_PX`] so a mask
/// boundary reads as a fine, dense dotted outline rather than a chunky dash.
pub const MASK_DASH_PX: f32 = 2.0;

/// Mask marching-ants gap length in canvas pixels (smaller than the
/// rectangular [`ANT_GAP_PX`]).
pub const MASK_GAP_PX: f32 = 2.0;

/// Alpha applied to the theme's marching-ants token so the rendered border is
/// a low-contrast guide instead of a full-brightness line.
pub const ANT_STROKE_ALPHA: u8 = 120;

/// Alpha applied to the theme's grey under-stroke token so the solid baseline
/// under the dashes reads steadily without competing with them.
///
/// ~200/255 (≈78%): higher than the dash alpha ([`ANT_STROKE_ALPHA`] = 120)
/// because the under-stroke colour is mid-grey ([96, 96, 96]) and grey over
/// the dark canvas needs more opacity to be legible, while the white dashes
/// stay dim so the border still reads as one low-contrast guide. The value is
/// fixed by construction — [`ant_under_stroke_color`] takes no time input — so
/// the under-line cannot flash.
pub const ANT_UNDER_STROKE_ALPHA: u8 = 200;

/// Dash offset in pixels for a given time: the pattern shifts by
/// [`ANT_SPEED_PX_PER_MS`] per millisecond, wrapping at the dash+gap period.
pub fn ant_phase(time_ms: u64) -> f32 {
    ((time_ms as f32) * ANT_SPEED_PX_PER_MS) % (ANT_DASH_PX + ANT_GAP_PX)
}

/// The marching-ants border color at `time_ms`: `base` (the theme token) with
/// its alpha scaled to [`ANT_STROKE_ALPHA`].  Deliberately independent of
/// `time_ms` — only the dash *phase* advances, the brightness never does, so
/// the border cannot flash.
pub fn ant_stroke_color(base: Color, _time_ms: u64) -> Color {
    let alpha = (u32::from(base.a) * u32::from(ANT_STROKE_ALPHA) / 255) as u8;
    Color::rgba(base.r, base.g, base.b, alpha)
}

/// The solid under-stroke for the marching-ants border: `base` (the theme's
/// grey `marching_ants_under` token) with its alpha scaled to
/// [`ANT_UNDER_STROKE_ALPHA`].
///
/// Takes no time input, unlike [`ant_stroke_color`]: the baseline under the
/// animated dashes is fixed and can never flash.
pub fn ant_under_stroke_color(base: Color) -> Color {
    let alpha = (u32::from(base.a) * u32::from(ANT_UNDER_STROKE_ALPHA) / 255) as u8;
    Color::rgba(base.r, base.g, base.b, alpha)
}

/// The 4 edges of `rect` (clockwise from top-left, inclusive pixel edges like
/// [`Rect2i::right`]/[`Rect2i::bottom`]), split into dash segments offset by
/// [`ant_phase`]: dashes of [`ANT_DASH_PX`] separated by [`ANT_GAP_PX`],
/// walking the closed perimeter. Corner segments may be shorter (partial
/// dashes). Deterministic. Empty rect → empty vec.
pub fn ant_segments(rect: Rect2i, time_ms: u64) -> Vec<((f32, f32), (f32, f32))> {
    if rect.is_empty() {
        return Vec::new();
    }
    let w = rect.w as f32;
    let h = rect.h as f32;
    let perimeter = 2.0 * (w + h);
    let period = ANT_DASH_PX + ANT_GAP_PX;
    let phase = ant_phase(time_ms);

    let mut segments = Vec::new();
    let mut pos = 0.0_f32;
    while pos < perimeter {
        let cycle = (pos + phase) % period;
        if cycle < ANT_DASH_PX {
            // In a dash: emit the segment, clamped to the perimeter end.
            let dash_end = (pos + (ANT_DASH_PX - cycle)).min(perimeter);
            segments.push((perimeter_point(rect, pos), perimeter_point(rect, dash_end)));
            pos = dash_end;
        } else {
            // In a gap: skip to the next dash start.
            pos += period - cycle;
        }
    }
    segments
}

/// Maps a distance along the closed perimeter (clockwise from the top-left
/// corner) to a point. The perimeter length is `2*(w+h)`; corners sit at
/// positions `0`, `w`, `w+h`, `2w+h`, wrapping back to the top-left.
fn perimeter_point(rect: Rect2i, pos: f32) -> (f32, f32) {
    let w = rect.w as f32;
    let h = rect.h as f32;
    let x = rect.x as f32;
    let y = rect.y as f32;
    let right = rect.right() as f32;
    let bottom = rect.bottom() as f32;
    if pos < w {
        (x + pos, y)
    } else if pos < w + h {
        (right, y + (pos - w))
    } else if pos < 2.0 * w + h {
        (right - (pos - w - h), bottom)
    } else {
        (x, bottom - (pos - 2.0 * w - h))
    }
}

/// A boundary edge in integer canvas coordinates: `(start, end)` pixel
/// corners, as yielded by
/// [`crate::core::select::Selection::outline_segments`]. Aliased so the
/// marching-ants signatures stay readable.
pub type BoundarySegment = ((i32, i32), (i32, i32));

/// The solid under-line geometry for a mask's boundary: one float line segment
/// per integer boundary unit segment, in input order.
///
/// `segments` is the boundary
/// [`crate::core::select::Selection::outline_segments`] yields for a
/// non-rectangular mask (integer unit edges). The painting layer strokes each
/// returned segment once in [`ant_under_stroke_color`] to lay a fixed grey
/// baseline under the animated dashes. Pure coordinate conversion: no phase,
/// no time.
pub fn ant_under_segments(segments: &[BoundarySegment]) -> Vec<((f32, f32), (f32, f32))> {
    segments
        .iter()
        .map(|&((x0, y0), (x1, y1))| ((x0 as f32, y0 as f32), (x1 as f32, y1 as f32)))
        .collect()
}

/// Orients `path` clockwise in y-down screen coordinates.
///
/// A clockwise polygon has negative signed shoelace area
/// `Σ cross(p_{i+1}, p_i)`, so a counter-clockwise path is reversed. Paths with
/// zero area are left untouched.
fn orient_clockwise(mut path: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let n = path.len();
    let mut area2 = 0.0_f64;
    for i in 0..n {
        let (x0, y0) = path[i];
        let (x1, y1) = path[(i + 1) % n];
        area2 += f64::from(x1) * f64::from(y0) - f64::from(x0) * f64::from(y1);
    }
    if area2 > 0.0 {
        path.reverse();
    }
    path
}

/// Splits a mask boundary's unit segments into connected components, as groups
/// of indices into `segments` (each group sorted ascending).
///
/// Two segments join the same component when they share an endpoint vertex.
/// Degenerate (zero-length) segments are dropped: they carry no boundary and
/// never join a component. A valid mask yields one component per disjoint
/// boundary loop — separate blobs are separate components, and the outer loop
/// and a hole are separate components too.
///
/// Deterministic: components are emitted in ascending first-index order, so the
/// split does not depend on any hash iteration order (the vertex map is only
/// probed, never iterated for output).
fn boundary_components(segments: &[BoundarySegment]) -> Vec<Vec<usize>> {
    let mut by_vertex: std::collections::HashMap<(i32, i32), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, &((x0, y0), (x1, y1))) in segments.iter().enumerate() {
        if (x0, y0) == (x1, y1) {
            continue;
        }
        by_vertex.entry((x0, y0)).or_default().push(i);
        by_vertex.entry((x1, y1)).or_default().push(i);
    }
    let mut visited = vec![false; segments.len()];
    let mut components = Vec::new();
    for seed in 0..segments.len() {
        if visited[seed] {
            continue;
        }
        let ((x0, y0), (x1, y1)) = segments[seed];
        if (x0, y0) == (x1, y1) {
            visited[seed] = true; // degenerate edges never join a component
            continue;
        }
        visited[seed] = true;
        let mut stack = vec![seed];
        let mut component = Vec::new();
        while let Some(i) = stack.pop() {
            component.push(i);
            let ((ax, ay), (bx, by)) = segments[i];
            for vertex in [(ax, ay), (bx, by)] {
                if let Some(neighbours) = by_vertex.get(&vertex) {
                    for &j in neighbours {
                        if !visited[j] {
                            visited[j] = true;
                            stack.push(j);
                        }
                    }
                }
            }
        }
        component.sort_unstable();
        components.push(component);
    }
    components
}

/// Chains one connected boundary `component` (ascending indices into
/// `segments`) into its closed clockwise loops.
///
/// The chain starts from the component's first unused segment and repeatedly
/// appends the far endpoint of the first unused segment that begins *or* ends
/// at the current chain end — segments stored with their endpoints reversed are
/// flipped to match. A walk that returns to its seed closes a loop; when
/// segments remain untouched after a close, the component holds another loop (a
/// selection with a hole, or two loops pinched at a shared vertex), so the walk
/// restarts from the next unused seed. This recovers *every* loop, not just the
/// first.
///
/// Implementation: instead of rescanning `component` for the next edge every
/// step (O(n²)), each vertex keeps its incident segment indices in ascending
/// order and a monotonic cursor past already-used entries. Because an edge is
/// never un-used, the cursor only moves forward, so the whole component chains
/// in O(n). The first unused entry the cursor lands on is *exactly* the edge the
/// historical ascending scan picked, so the resulting loops (ordering,
/// orientation, seed order) are bit-identical.
///
/// Each loop is oriented clockwise ([`orient_clockwise`]) — including a hole
/// loop, matching the historical single-loop convention. A walk that fails to
/// close (never for a valid mask) is skipped rather than emitting garbage.
fn chain_component_loops(
    segments: &[BoundarySegment],
    component: &[usize],
) -> Vec<Vec<(f32, f32)>> {
    use std::collections::HashMap;

    // Vertex -> incident segment indices, ascending (component is ascending).
    // Degenerate self-loops are never in a component (see `boundary_components`)
    // but are filtered here too so the far-endpoint choice is unambiguous.
    let mut incident: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for &i in component {
        let ((x0, y0), (x1, y1)) = segments[i];
        if (x0, y0) == (x1, y1) {
            continue;
        }
        incident.entry((x0, y0)).or_default().push(i);
        incident.entry((x1, y1)).or_default().push(i);
    }
    // Per-vertex cursor: index of the first possibly-unused entry above.
    let mut cursor: HashMap<(i32, i32), usize> = HashMap::new();

    let mut used = vec![false; segments.len()];
    let mut loops = Vec::new();
    for &seed in component {
        if used[seed] {
            continue;
        }
        let start = (segments[seed].0 .0 as f32, segments[seed].0 .1 as f32);
        let mut chain = vec![start];
        let mut current = start;
        let mut closed = false;
        loop {
            let key = (current.0 as i32, current.1 as i32);
            let mut next: Option<(f32, f32)> = None;
            if let Some(list) = incident.get(&key) {
                let cur = cursor.entry(key).or_insert(0);
                while *cur < list.len() && used[list[*cur]] {
                    *cur += 1;
                }
                if *cur < list.len() {
                    let i = list[*cur];
                    used[i] = true;
                    let ((x0, y0), (x1, y1)) = segments[i];
                    let a = (x0 as f32, y0 as f32);
                    let b = (x1 as f32, y1 as f32);
                    next = Some(if a == current { b } else { a });
                }
            }
            let Some(next) = next else {
                // Defensive: no continuation — not a closed loop.
                break;
            };
            if next == start {
                // Loop closed; the closing edge is implicit.
                closed = true;
                break;
            }
            current = next;
            chain.push(next);
        }
        if closed {
            loops.push(orient_clockwise(chain));
        }
    }
    loops
}

/// Chains a mask boundary's unit segments into per-component closed polygons,
/// each oriented clockwise in screen coordinates (y-down).
///
/// `segments` is the boundary
/// [`crate::core::select::Selection::outline_segments`] yields for a
/// non-rectangular mask: integer unit edges that are **not** in walking order.
/// They are split into connected components by shared vertices
/// ([`boundary_components`]) and every component is chained into its own
/// clockwise loop(s) ([`chain_component_loops`]). A plain mask yields one loop;
/// disjoint blobs and a selection with a hole each yield one loop per
/// component; a component pinched at a shared vertex yields one loop per
/// sub-loop.
///
/// Deterministic. Degenerate segments are ignored. Empty input (or a boundary
/// that cannot be chained) yields an empty vec.
pub fn mask_polylines(segments: &[BoundarySegment]) -> Vec<Vec<(f32, f32)>> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut loops = Vec::new();
    for component in boundary_components(segments) {
        loops.extend(chain_component_loops(segments, &component));
    }
    loops
}

/// Chains a mask boundary's unit segments into a single flat closed polygon
/// path, oriented clockwise in screen coordinates (y-down).
///
/// A single-component mask — the common case, including rectangular and
/// L-shaped selections — returns exactly the one clockwise loop
/// [`mask_polylines`] yields, byte-identical to the historical single-loop
/// behavior.
///
/// A flat `Vec<(f32, f32)>` cannot represent several disjoint closed loops:
/// the consumer draws it as `path[i] -> path[(i + 1) % n]`, so concatenating
/// components would draw spurious chords *between* them. For a multi-component
/// mask (disjoint blobs, or a selection with a hole) this therefore returns
/// only the **first** component's loop — a deterministic, chord-free baseline
/// over the primary component — rather than a bogus combined path or nothing.
/// The animated dashes are unaffected: [`mask_dash_segments`] covers every
/// component. Callers that want a black baseline for *every* component should
/// iterate [`mask_polylines`] instead (the follow-up `paint_ants_mask` change).
///
/// Degenerate segments are ignored; an empty or unchained boundary yields an
/// empty vec.
pub fn mask_polyline(segments: &[BoundarySegment]) -> Vec<(f32, f32)> {
    mask_polylines(segments)
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Maps an arc length along the closed polyline `path` (edge lengths
/// `edge_len`, total perimeter `total`) to a point. `pos == total` wraps onto
/// the first vertex (the loop's closing corner).
///
/// Task II: the production dash walk uses the O(1) [`cursor_point`] instead;
/// this linear lookup survives only for [`dash_edge_pieces`] / the legacy test
/// reference, so it is allowed to be dead outside tests.
#[allow(dead_code)]
fn polyline_point(path: &[(f32, f32)], edge_len: &[f32], total: f32, pos: f32) -> (f32, f32) {
    debug_assert!(total > 0.0);
    if pos >= total {
        return path[0];
    }
    let n = path.len();
    let mut pos = pos;
    for i in 0..n {
        let len = edge_len[i];
        if pos < len {
            let (x0, y0) = path[i];
            let (x1, y1) = path[(i + 1) % n];
            if len <= 0.0 {
                return (x0, y0);
            }
            let t = pos / len;
            return (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        }
        pos -= len;
    }
    // Float rounding at the tail: clamp onto the closing corner.
    path[0]
}

/// O(1) counterpart of [`polyline_point`] for a walk that already knows which
/// edge `pos` falls on: `cursor` is the edge index and `cursor_start` its
/// cumulative arc length (the value [`polyline_point`] reaches by subtracting
/// every preceding edge length).
///
/// Mirrors [`polyline_point`] exactly for the arc positions the dash walk
/// produces — an interior position interpolates `x0 + (x1 - x0) * t` with the
/// same `t = (pos - cursor_start) / len`, and a position that lands exactly on
/// the edge end returns that edge's far vertex — so every emitted point is
/// bit-identical. The zero-length branch is defensive: the walk only emits on
/// edges with `len > 0`.
fn cursor_point(
    path: &[(f32, f32)],
    edge_len: &[f32],
    total: f32,
    cursor: usize,
    cursor_start: f32,
    pos: f32,
) -> (f32, f32) {
    debug_assert!(total > 0.0);
    if pos >= total {
        return path[0];
    }
    let n = path.len();
    let len = edge_len[cursor];
    let (x0, y0) = path[cursor];
    let (x1, y1) = path[(cursor + 1) % n];
    if len <= 0.0 {
        return (x0, y0);
    }
    if pos >= cursor_start + len {
        // `polyline_point` steps one edge further when the position lands on
        // the edge end exactly: it returns that edge's start vertex.
        return path[(cursor + 1) % n];
    }
    let t = (pos - cursor_start) / len;
    (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)
}

/// Dashed marching-ants segments computed directly from **ready** mask
/// polylines — the O(n) form of [`mask_dash_segments`], which avoids
/// re-chaining the boundary.
///
/// `polylines` are the closed, clockwise polygons [`mask_polylines`] yields —
/// one per connected component (separate blobs and a selection's hole each get
/// their own loop). This walks **each** loop's edges once with a single forward
/// cursor and concatenates the results: dashes of [`MASK_DASH_PX`] separated by
/// [`MASK_GAP_PX`] travel *along each path*, turning around corners as the
/// phase advances. Every component starts its walk at the same phase, so the
/// dashes stay in step across components. Because each path is clockwise and
/// the phase only grows with time, the dashes rotate clockwise around every
/// loop. Corner/end dashes may be shorter (partial dashes). Deterministic.
///
/// Every dash is split into one line piece per polyline edge it touches (the
/// same L-kink semantics as [`dash_edge_pieces`]), so a dash crossing a corner
/// is emitted as two pieces meeting exactly at the corner vertex — never a
/// diagonal chord. The consumer draws each returned pair as its own line
/// segment, so a multi-component result draws every component's dashes without
/// any chord between them.
///
/// Output is byte-identical to the historical
/// `mask_dash_segments(segments, time_ms)` evaluated with
/// `polylines = mask_polylines(segments)`, but costs O(perimeter) instead of
/// O(perimeter² / period): the per-dash edge rescan and the per-point linear
/// [`polyline_point`] lookup are replaced by one monotonic cursor over the
/// edges. Callers that already hold the polylines (the UI caches them per
/// selection) call this directly so the black baseline and the dashes share a
/// single chain.
///
/// Empty input, a polyline with fewer than two vertices, or a zero-perimeter
/// polyline yields nothing. No panics.
pub fn mask_dash_segments_from_polylines(
    polylines: &[Vec<(f32, f32)>],
    time_ms: u64,
) -> Vec<((f32, f32), (f32, f32))> {
    // Mask-specific period and phase: the rectangular [`ant_phase`] is bound to
    // the rectangular period, so the mask computes its own over its own,
    // finer period. Shared by every component so their dashes march in step.
    let period = MASK_DASH_PX + MASK_GAP_PX;
    let phase = (time_ms as f32 * ANT_SPEED_PX_PER_MS) % period;

    let mut dashes = Vec::new();
    for path in polylines {
        let n = path.len();
        if n < 2 {
            continue;
        }
        let mut edge_len = Vec::with_capacity(n);
        let mut total = 0.0_f32;
        for i in 0..n {
            let (x0, y0) = path[i];
            let (x1, y1) = path[(i + 1) % n];
            let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            edge_len.push(len);
            total += len;
        }
        if total <= 0.0 {
            continue;
        }
        // Single forward cursor: `cursor` is the first edge not yet fully
        // consumed by a previous dash and `cursor_start` is its cumulative arc
        // length. The walk's dash intervals advance monotonically, so the
        // cursor never rewinds — O(n) per loop instead of O(n) per dash.
        let mut cursor = 0usize;
        let mut cursor_start = 0.0_f32;
        let mut pos = 0.0_f32;
        while pos < total {
            let cycle = (pos + phase) % period;
            if cycle < MASK_DASH_PX {
                // In a dash: split it at every edge boundary so it follows the
                // path, clamped to the loop's end (the closing corner).
                let dash_end = (pos + (MASK_DASH_PX - cycle)).min(total);
                if dash_end <= pos {
                    break; // float no-progress guard
                }
                // Resume from the first edge whose end reaches past the dash
                // start; earlier edges can emit nothing. Keeping `cursor_start`
                // accumulated by the same additions reproduces `edge_start`.
                while cursor < n && cursor_start + edge_len[cursor] <= pos {
                    cursor_start += edge_len[cursor];
                    cursor += 1;
                }
                // Mirror `dash_edge_pieces` for the single, non-wrapping
                // interval `[pos, dash_end]` the walk produces, from the cursor.
                while cursor < n {
                    let len = edge_len[cursor];
                    let edge_start = cursor_start;
                    let edge_end = edge_start + len;
                    if len > 0.0 && edge_start < dash_end && edge_end > pos {
                        let piece_start = pos.max(edge_start);
                        let piece_end = dash_end.min(edge_end);
                        if piece_end > piece_start {
                            dashes.push((
                                cursor_point(
                                    path,
                                    &edge_len,
                                    total,
                                    cursor,
                                    edge_start,
                                    piece_start,
                                ),
                                cursor_point(path, &edge_len, total, cursor, edge_start, piece_end),
                            ));
                        }
                    }
                    // Stop on the edge that contains `dash_end`, leaving the
                    // cursor on it: floating point can hand the FOLLOW-UP dash
                    // an interval starting inside this same edge (a width-epsilon
                    // dash at a period boundary), and the forward-only cursor
                    // must still find it.
                    if edge_end >= dash_end {
                        break;
                    }
                    cursor_start = edge_end;
                    cursor += 1;
                }
                pos = dash_end;
            } else {
                // In a gap: skip to the next dash start.
                pos += period - cycle;
            }
        }
    }
    dashes
}

/// Dashed marching-ants segments along a mask's boundary, marching clockwise.
///
/// `segments` is the boundary
/// [`crate::core::select::Selection::outline_segments`] yields for a
/// non-rectangular mask (integer unit edges, not in walking order). This chains
/// them into one or more closed clockwise polygons (one per disjoint component
/// — separate blobs, and a selection's hole, each get their own loop) and walks
/// them; it is the thin wrapper over [`mask_polylines`] +
/// [`mask_dash_segments_from_polylines`], kept for callers that hold the raw
/// segments. Callers that already have the polylines should call
/// [`mask_dash_segments_from_polylines`] directly and save the chain.
///
/// Empty input (or a boundary that cannot be chained into a closed loop) yields
/// an empty vec.
pub fn mask_dash_segments(
    segments: &[BoundarySegment],
    time_ms: u64,
) -> Vec<((f32, f32), (f32, f32))> {
    mask_dash_segments_from_polylines(&mask_polylines(segments), time_ms)
}

/// Splits the dash spanning arc lengths `[start, end]` into one line piece per
/// polyline edge it touches, so every emitted piece lies on a single edge.
///
/// This is what makes dashes *kink* at corners: a dash crossing a corner
/// becomes two pieces meeting exactly at the corner vertex, instead of one
/// straight chord cutting diagonally across it. Zero-length (degenerate)
/// pieces are skipped. If `end` runs past `total` the interval wraps onto the
/// start of the loop and kinks at the closing corner (`path[0]`); the walk
/// clamps dashes to `total`, so in practice this only guards the helper.
///
/// Task II: the production walk no longer calls this per-dash (it uses the
/// single forward cursor); this survives as the L-kink reference and for the
/// legacy test/benchmark path, so it is allowed to be dead outside tests.
#[allow(dead_code)]
fn dash_edge_pieces(
    path: &[(f32, f32)],
    edge_len: &[f32],
    total: f32,
    start: f32,
    end: f32,
) -> Vec<((f32, f32), (f32, f32))> {
    let mut pieces = Vec::new();
    if total <= 0.0 || end <= start {
        return pieces;
    }
    let n = path.len();
    let mut seg_start = start;
    let mut seg_end = end;
    loop {
        let clamped_end = seg_end.min(total);
        let mut edge_start = 0.0_f32;
        for i in 0..n {
            let len = edge_len[i];
            let edge_end = edge_start + len;
            if len > 0.0 && edge_start < clamped_end && edge_end > seg_start {
                let piece_start = seg_start.max(edge_start);
                let piece_end = clamped_end.min(edge_end);
                if piece_end > piece_start {
                    pieces.push((
                        polyline_point(path, edge_len, total, piece_start),
                        polyline_point(path, edge_len, total, piece_end),
                    ));
                }
            }
            edge_start = edge_end;
            if edge_start >= clamped_end {
                break;
            }
        }
        if seg_end <= total {
            break;
        }
        // Wrap the remainder onto the start of the loop.
        seg_start = 0.0;
        seg_end -= total;
    }
    pieces
}

/// Test-only: the historical O(perimeter² / period) dash walk, which re-scanned
/// every edge from 0 for each dash via [`dash_edge_pieces`]. Kept so tests can
/// prove [`mask_dash_segments_from_polylines`] is output-identical and so the
/// perf probe can measure the BEFORE path. Not part of the production API.
#[cfg(test)]
pub(crate) fn legacy_mask_dash_segments_from_polylines(
    polylines: &[Vec<(f32, f32)>],
    time_ms: u64,
) -> Vec<((f32, f32), (f32, f32))> {
    let period = MASK_DASH_PX + MASK_GAP_PX;
    let phase = (time_ms as f32 * ANT_SPEED_PX_PER_MS) % period;
    let mut dashes = Vec::new();
    for path in polylines {
        let n = path.len();
        if n < 2 {
            continue;
        }
        let mut edge_len = Vec::with_capacity(n);
        let mut total = 0.0_f32;
        for i in 0..n {
            let (x0, y0) = path[i];
            let (x1, y1) = path[(i + 1) % n];
            let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            edge_len.push(len);
            total += len;
        }
        if total <= 0.0 {
            continue;
        }
        let mut pos = 0.0_f32;
        while pos < total {
            let cycle = (pos + phase) % period;
            if cycle < MASK_DASH_PX {
                let dash_end = (pos + (MASK_DASH_PX - cycle)).min(total);
                if dash_end <= pos {
                    break;
                }
                dashes.extend(dash_edge_pieces(path, &edge_len, total, pos, dash_end));
                pos = dash_end;
            } else {
                pos += period - cycle;
            }
        }
    }
    dashes
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // onion_ghosts
    // -----------------------------------------------------------------------

    #[test]
    fn onion_ghosts_prevs_then_nexts_nearest_first() {
        let regions = vec![
            Rect2i::new(0, 0, 8, 8),
            Rect2i::new(10, 0, 8, 8),
            Rect2i::new(20, 0, 8, 8),
            Rect2i::new(30, 0, 8, 8),
            Rect2i::new(40, 0, 8, 8),
        ];
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(2);
        cfg.set_next_frames(2);
        let ghosts = onion_ghosts(&regions, 2, cfg, Rect2i::new(0, 0, 64, 64));
        assert_eq!(ghosts.len(), 4);
        // Prevs nearest first: index 1, then 0, tinted prev.
        assert_eq!(ghosts[0].rect, regions[1]);
        assert_eq!(ghosts[0].tint, cfg.prev_tint());
        assert_eq!(ghosts[1].rect, regions[0]);
        assert_eq!(ghosts[1].tint, cfg.prev_tint());
        // Nexts nearest first: index 3, then 4, tinted next.
        assert_eq!(ghosts[2].rect, regions[3]);
        assert_eq!(ghosts[2].tint, cfg.next_tint());
        assert_eq!(ghosts[3].rect, regions[4]);
        assert_eq!(ghosts[3].tint, cfg.next_tint());
    }

    #[test]
    fn onion_ghosts_clips_to_canvas() {
        let regions = vec![
            Rect2i::new(0, 0, 8, 8),
            Rect2i::new(60, 60, 8, 8), // fully outside the canvas
            Rect2i::new(10, 10, 8, 8), // partially outside the canvas
        ];
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(1);
        cfg.set_next_frames(1);
        let canvas = Rect2i::new(0, 0, 16, 16);
        let ghosts = onion_ghosts(&regions, 1, cfg, canvas);
        // prev = regions[0] fully inside; next = regions[2] clipped to (10,10,6,6).
        assert_eq!(ghosts.len(), 2);
        assert_eq!(ghosts[0].rect, Rect2i::new(0, 0, 8, 8));
        assert_eq!(ghosts[1].rect, Rect2i::new(10, 10, 6, 6));
    }

    #[test]
    fn onion_ghosts_skips_empty_clipped_regions() {
        let regions = vec![
            Rect2i::new(100, 100, 8, 8), // outside the canvas → empty after clip
            Rect2i::new(0, 0, 8, 8),     // current frame
            Rect2i::new(0, 0, 8, 8),     // next frame, fully inside
        ];
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(1);
        cfg.set_next_frames(1);
        let ghosts = onion_ghosts(&regions, 1, cfg, Rect2i::new(0, 0, 16, 16));
        // The outside prev is skipped; only the inside next survives.
        assert_eq!(ghosts.len(), 1);
        assert_eq!(ghosts[0].rect, Rect2i::new(0, 0, 8, 8));
    }

    #[test]
    fn onion_ghosts_has_onion_false_is_empty() {
        let regions = vec![Rect2i::new(0, 0, 8, 8), Rect2i::new(10, 0, 8, 8)];
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(0);
        cfg.set_next_frames(0);
        assert!(!cfg.has_onion());
        assert!(onion_ghosts(&regions, 0, cfg, Rect2i::new(0, 0, 64, 64)).is_empty());
    }

    #[test]
    fn onion_ghosts_empty_regions_or_bad_current_is_empty() {
        let cfg = OnionConfig::new();
        assert!(onion_ghosts(&[], 0, cfg, Rect2i::new(0, 0, 64, 64)).is_empty());
        let regions = vec![Rect2i::new(0, 0, 8, 8)];
        assert!(onion_ghosts(&regions, 1, cfg, Rect2i::new(0, 0, 64, 64)).is_empty());
        assert!(onion_ghosts(&regions, 5, cfg, Rect2i::new(0, 0, 64, 64)).is_empty());
    }

    #[test]
    fn onion_ghosts_no_wrap_around() {
        // current = 0: no prevs (no wrap to the end), only nexts.
        let regions = vec![
            Rect2i::new(0, 0, 8, 8),
            Rect2i::new(10, 0, 8, 8),
            Rect2i::new(20, 0, 8, 8),
        ];
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(3);
        cfg.set_next_frames(3);
        let ghosts = onion_ghosts(&regions, 0, cfg, Rect2i::new(0, 0, 64, 64));
        assert_eq!(ghosts.len(), 2); // only nexts 1 and 2
        assert_eq!(ghosts[0].rect, regions[1]);
        assert_eq!(ghosts[1].rect, regions[2]);
        assert_eq!(ghosts[0].tint, cfg.next_tint());
    }

    #[test]
    fn onion_ghosts_last_frame_has_no_nexts() {
        let regions = vec![Rect2i::new(0, 0, 8, 8), Rect2i::new(10, 0, 8, 8)];
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(3);
        cfg.set_next_frames(3);
        let ghosts = onion_ghosts(&regions, 1, cfg, Rect2i::new(0, 0, 64, 64));
        assert_eq!(ghosts.len(), 1);
        assert_eq!(ghosts[0].rect, regions[0]);
        assert_eq!(ghosts[0].tint, cfg.prev_tint());
    }

    // -----------------------------------------------------------------------
    // ant_phase
    // -----------------------------------------------------------------------

    #[test]
    fn ant_phase_advances_three_px_per_second() {
        // Given: representative timestamps spanning more than one dash period.
        // When: the phase is sampled one second apart.
        // Then: every phase advances by exactly three pixels modulo the period.
        assert_eq!(ant_phase(0), 0.0);
        assert!((ant_phase(1000) - 3.0).abs() < 1e-4);
        for t in [0u64, 37, 123, 500, 999, 1234] {
            let before = ant_phase(t);
            let after = ant_phase(t + 1000);
            let expected = (before + 3.0) % 6.0;
            assert!(
                (after - expected).abs() < 1e-3,
                "t={t}: {before} -> {after}, expected {expected}"
            );
        }
    }

    #[test]
    fn ant_phase_wraps_after_two_seconds() {
        // Given: the six-pixel dash-plus-gap period at three pixels per second.
        // When: the phase is sampled around the two-second wrap.
        // Then: it returns to zero at 2000 ms and immediately advances again.
        assert!((ant_phase(1999) - 5.997).abs() < 1e-3);
        assert!((ant_phase(2000) - 0.0).abs() < 1e-3);
        assert!((ant_phase(2001) - 0.003).abs() < 1e-4);
        assert!((ant_phase(4000) - 0.0).abs() < 1e-3);
    }

    #[test]
    fn ant_phase_stays_in_period() {
        for t in [0u64, 1, 50, 1000, 1_000_000, u64::MAX / 2] {
            let p = ant_phase(t);
            assert!((0.0..6.0).contains(&p), "phase {p} out of range for t={t}");
        }
    }

    // -----------------------------------------------------------------------
    // ant_segments
    // -----------------------------------------------------------------------

    #[test]
    fn ant_segments_empty_rect_is_empty() {
        assert!(ant_segments(Rect2i::ZERO, 0).is_empty());
        assert!(ant_segments(Rect2i::new(0, 0, 0, 10), 0).is_empty());
        assert!(ant_segments(Rect2i::new(0, 0, 10, -1), 0).is_empty());
    }

    #[test]
    fn ant_segments_clockwise_closure_from_top_left() {
        // 32 px perimeter = 5 six-px periods + 2 px, so the trailing dash is
        // cut short exactly at the start corner.
        let rect = Rect2i::new(2, 3, 10, 6);
        let segments = ant_segments(rect, 0);
        assert!(!segments.is_empty());
        // First segment starts at the top-left corner.
        assert_eq!(segments[0].0, (2.0, 3.0));
        // Last segment ends at the top-left corner (closed perimeter).
        assert_eq!(segments.last().unwrap().1, (2.0, 3.0));
        // Every endpoint lies on the rect border.
        for &((x0, y0), (x1, y1)) in &segments {
            for (x, y) in [(x0, y0), (x1, y1)] {
                let on_border = (y == 3.0 && (2.0..=12.0).contains(&x))
                    || (x == 12.0 && (3.0..=9.0).contains(&y))
                    || (y == 9.0 && (2.0..=12.0).contains(&x))
                    || (x == 2.0 && (3.0..=9.0).contains(&y));
                assert!(on_border, "point ({x},{y}) not on the rect border");
            }
        }
    }

    #[test]
    fn ant_segments_total_dash_length_matches_perimeter() {
        // Rect whose perimeter is a multiple of the period: the dash pattern
        // tiles exactly, so total dash = perimeter / 2.
        let rect = Rect2i::new(0, 0, 6, 6); // perimeter 24 = 4 * 6
        let segments = ant_segments(rect, 0);
        let total: f32 = segments
            .iter()
            .map(|&((x0, y0), (x1, y1))| ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt())
            .sum();
        assert!((total - 12.0).abs() < 1e-3, "total dash {total} != 12");

        // General rect: the walk covers the whole perimeter, so dash + gap ==
        // perimeter; the trailing partial (last dash/gap cut short) is bounded
        // by one period.
        let rect = Rect2i::new(1, 2, 10, 7); // perimeter 34
        let segments = ant_segments(rect, 0);
        let total: f32 = segments
            .iter()
            .map(|&((x0, y0), (x1, y1))| ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt())
            .sum();
        let perimeter = 2.0 * (10.0 + 7.0);
        assert!(
            (total - perimeter / 2.0).abs() <= 6.0 + 1e-3,
            "total dash {total} too far from {perimeter}/2"
        );
    }

    #[test]
    fn ant_segments_deterministic() {
        let rect = Rect2i::new(3, 4, 12, 9);
        assert_eq!(ant_segments(rect, 1234), ant_segments(rect, 1234));
    }

    #[test]
    fn ant_segments_phase_shifts_dashes() {
        // Given: a rectangular border whose perimeter spans several dash periods.
        // When: segments are sampled one second apart at three pixels per second.
        // Then: the dash geometry moves and begins three pixels along the top edge.
        let rect = Rect2i::new(0, 0, 12, 12);
        let at_zero = ant_segments(rect, 0);
        let after_one_second = ant_segments(rect, 1000);
        assert_ne!(at_zero, after_one_second);
        assert_eq!(after_one_second[0], ((3.0, 0.0), (6.0, 0.0)));
    }

    #[test]
    fn ant_stroke_color_is_low_contrast_and_time_invariant() {
        let base = Color::rgba(255, 255, 255, 255);
        let early = ant_stroke_color(base, 0);
        let later = ant_stroke_color(base, 123_456);
        assert_eq!(early, later, "the ants color must not vary with time");
        assert!(
            early.a < 200,
            "the ants border must stay low contrast; got alpha {}",
            early.a
        );
        assert!(early.a > 0, "the ants border must remain visible");
        assert_eq!(
            (early.r, early.g, early.b),
            (base.r, base.g, base.b),
            "only the alpha is reduced; the token's hue is preserved"
        );
    }

    // -----------------------------------------------------------------------
    // under-stroke
    // -----------------------------------------------------------------------

    #[test]
    fn ant_under_stroke_color_is_time_independent() {
        // Given: the grey under-stroke base token and many frame timestamps.
        // When: the under-stroke color is sampled while the dash phase advances.
        // Then: every sample is byte-identical, so the under-line can never flash.
        let base = Color::rgba(96, 96, 96, ANT_UNDER_STROKE_ALPHA);
        let first = ant_under_stroke_color(base);
        for frame in 0..64u64 {
            let _advancing_phase = ant_phase(frame * 137);
            assert_eq!(
                ant_under_stroke_color(base),
                first,
                "frame {frame}: the under-stroke must not vary with time"
            );
        }
    }

    #[test]
    fn ant_under_stroke_color_scales_alpha() {
        // Given: an opaque and a half-transparent grey base.
        // When: the under-stroke color is derived.
        // Then: RGB is preserved and alpha is scaled to ANT_UNDER_STROKE_ALPHA.
        let opaque = ant_under_stroke_color(Color::rgba(96, 96, 96, 255));
        assert_eq!(opaque, Color::rgba(96, 96, 96, ANT_UNDER_STROKE_ALPHA));

        let half = ant_under_stroke_color(Color::rgba(10, 20, 30, 128));
        assert_eq!((half.r, half.g, half.b), (10, 20, 30));
        assert_eq!(
            half.a,
            (128 * u32::from(ANT_UNDER_STROKE_ALPHA) / 255) as u8
        );
    }

    #[test]
    fn ant_under_segments_maps_unit_segments_to_float_pairs() {
        // Given: the integer boundary unit segments a mask selection yields.
        // When: the solid under-line geometry is built.
        // Then: every segment is preserved 1:1, in order, as float pairs.
        let units = vec![((2, 3), (3, 3)), ((3, 3), (3, 4))];
        assert_eq!(
            ant_under_segments(&units),
            vec![((2.0, 3.0), (3.0, 3.0)), ((3.0, 3.0), (3.0, 4.0))]
        );
    }

    #[test]
    fn ant_under_segments_empty_is_empty() {
        assert!(ant_under_segments(&[]).is_empty());
    }

    // -----------------------------------------------------------------------
    // mask_polyline / mask_dash_segments
    // -----------------------------------------------------------------------

    /// The boundary unit segments of an L-shaped mask (a 4×4 square missing
    /// its bottom-right 2×2 corner), listed clockwise from the top-left.
    fn l_shape_unit_segments() -> Vec<BoundarySegment> {
        let mut s = Vec::new();
        // Top: (0,0) → (4,0).
        for x in 0..4 {
            s.push(((x, 0), (x + 1, 0)));
        }
        // Right: (4,0) → (4,2).
        for y in 0..2 {
            s.push(((4, y), (4, y + 1)));
        }
        // Notch: (4,2) → (2,2), then (2,2) → (2,4).
        for x in (2..4).rev() {
            s.push(((x + 1, 2), (x, 2)));
        }
        for y in 2..4 {
            s.push(((2, y), (2, y + 1)));
        }
        // Bottom: (2,4) → (0,4).
        for x in (0..2).rev() {
            s.push(((x + 1, 4), (x, 4)));
        }
        // Left: (0,4) → (0,0).
        for y in (0..4).rev() {
            s.push(((0, y + 1), (0, y)));
        }
        s
    }

    /// The boundary unit segments of a `size × size` square, clockwise from
    /// the top-left corner.
    fn square_unit_segments(size: i32) -> Vec<BoundarySegment> {
        let mut s = Vec::new();
        for x in 0..size {
            s.push(((x, 0), (x + 1, 0)));
        }
        for y in 0..size {
            s.push(((size, y), (size, y + 1)));
        }
        for x in (0..size).rev() {
            s.push(((x + 1, size), (x, size)));
        }
        for y in (0..size).rev() {
            s.push(((0, y + 1), (0, y)));
        }
        s
    }

    /// Translates every endpoint of `segments` by `(dx, dy)`.
    fn translate_unit_segments(
        segments: &[BoundarySegment],
        dx: i32,
        dy: i32,
    ) -> Vec<BoundarySegment> {
        segments
            .iter()
            .map(|&((x0, y0), (x1, y1))| ((x0 + dx, y0 + dy), (x1 + dx, y1 + dy)))
            .collect()
    }

    /// Deterministic scramble: reverse the list (flipping every segment's
    /// orientation), swap adjacent pairs — destroying any walking order — then
    /// rotate so the first segment starts at `start`.
    fn scramble_from(segments: Vec<BoundarySegment>, start: (i32, i32)) -> Vec<BoundarySegment> {
        let mut v: Vec<BoundarySegment> = segments.into_iter().rev().collect();
        for i in (0..v.len() - 1).step_by(2) {
            v.swap(i, i + 1);
        }
        let pos = v
            .iter()
            .position(|&((x0, y0), _)| (x0, y0) == start)
            .expect("the start vertex must begin some segment");
        v.rotate_left(pos);
        v
    }

    /// Signed shoelace area ×2 over `path`, using the same y-down convention
    /// as [`mask_polyline`]: negative ⇒ clockwise.
    fn signed_area2(path: &[(f32, f32)]) -> f64 {
        let mut area = 0.0_f64;
        for i in 0..path.len() {
            let (x0, y0) = path[i];
            let (x1, y1) = path[(i + 1) % path.len()];
            area += f64::from(x1) * f64::from(y0) - f64::from(x0) * f64::from(y1);
        }
        area
    }

    /// Whether `p` lies on the boundary segment `s` (within a tiny tolerance —
    /// a dash boundary can land mid-segment).
    fn on_segment(s: BoundarySegment, p: (f32, f32)) -> bool {
        let eps = 1e-3;
        let ((x0, y0), (x1, y1)) = s;
        let (x0, y0, x1, y1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len2 = dx * dx + dy * dy;
        if len2 <= 0.0 {
            return (p.0 - x0).abs() < eps && (p.1 - y0).abs() < eps;
        }
        let t = ((p.0 - x0) * dx + (p.1 - y0) * dy) / len2;
        let (cx, cy) = (x0 + dx * t, y0 + dy * t);
        (cx - p.0).abs() < eps && (cy - p.1).abs() < eps && (-eps..=1.0 + eps).contains(&t)
    }

    /// Whether `p` lies on the union of the boundary segments.
    fn on_any_segment(segments: &[BoundarySegment], p: (f32, f32)) -> bool {
        segments.iter().any(|&s| on_segment(s, p))
    }

    /// Whether `a` and `b` both lie on one and the same boundary segment — the
    /// property that proves a dash piece did not cut across an edge boundary.
    fn on_same_segment(segments: &[BoundarySegment], a: (f32, f32), b: (f32, f32)) -> bool {
        segments
            .iter()
            .any(|&s| on_segment(s, a) && on_segment(s, b))
    }

    #[test]
    fn mask_dash_segments_empty_is_empty() {
        assert!(mask_dash_segments(&[], 0).is_empty());
        assert!(mask_dash_segments(&[], 1_000).is_empty());
        assert!(mask_polyline(&[]).is_empty());
    }

    #[test]
    fn mask_dash_segments_chains_unsorted_segments_into_clockwise_loop() {
        // Given: an L-shaped mask boundary whose unit segments are handed over
        // in a scrambled order (reversed + pair-swapped).
        // When: the dashes are computed at phase zero.
        // Then: the segments were chained into one closed loop — the dashes
        // are non-empty and every dash endpoint lies on the boundary union.
        let ordered = l_shape_unit_segments();
        let scrambled = scramble_from(ordered.clone(), (0, 0));
        assert_ne!(scrambled, ordered, "the scramble must not be a no-op");
        let dashes = mask_dash_segments(&scrambled, 0);
        assert!(
            !dashes.is_empty(),
            "the L perimeter (17 px) must yield dashes"
        );
        for &(p0, p1) in &dashes {
            assert!(
                on_any_segment(&ordered, p0),
                "dash start {p0:?} lies off the boundary"
            );
            assert!(
                on_any_segment(&ordered, p1),
                "dash end {p1:?} lies off the boundary"
            );
        }
    }

    #[test]
    fn mask_dash_segments_is_clockwise() {
        // Given: an asymmetric L-shaped mask boundary in a scrambled order.
        // When: the polyline is ordered.
        // Then: it is a single clockwise (y-down) loop: the signed shoelace
        // area is negative, its magnitude matches the L's area (12 px² → 2×
        // area = 24), and every step is exactly one unit.
        let scrambled = scramble_from(l_shape_unit_segments(), (0, 0));
        let path = mask_polyline(&scrambled);
        assert!(path.len() >= 4);
        let area2 = signed_area2(&path);
        assert!(
            area2 < 0.0,
            "the ordered path must be clockwise; got {area2}"
        );
        assert!(
            (area2 + 24.0).abs() < 1e-3,
            "the L has area 12 px² (2× area = 24); got {area2}"
        );
        for i in 0..path.len() {
            let a = path[i];
            let b = path[(i + 1) % path.len()];
            let step = (b.0 - a.0).abs() + (b.1 - a.1).abs();
            assert!((step - 1.0).abs() < 1e-4, "non-unit step {a:?} → {b:?}");
        }
    }

    #[test]
    fn mask_dash_segments_phase_advances() {
        // Given: a 12×12 square mask boundary (scrambled, but chained from the
        // top-left corner).
        // When: the dashes are sampled one second apart.
        // Then: the dash geometry moves — the first white piece advances along
        // the top edge — the clockwise path-following analogue of
        // `ant_segments_phase_shifts_dashes`.
        let scrambled = scramble_from(square_unit_segments(12), (0, 0));
        let at_zero = mask_dash_segments(&scrambled, 0);
        let after_one_second = mask_dash_segments(&scrambled, 1_000);
        assert_ne!(at_zero, after_one_second);
        // Mask period 4 (dash 2 + gap 2): at t=0 the first dash covers [0,2),
        // so its first unit-edge piece is (0,0)→(1,0); at t=1000 the phase is
        // 3, so the dash starts at arc length 1 and its first piece is
        // (1,0)→(2,0).
        assert_eq!(at_zero[0], ((0.0, 0.0), (1.0, 0.0)));
        assert_eq!(after_one_second[0], ((1.0, 0.0), (2.0, 0.0)));
    }

    #[test]
    fn mask_dash_segments_kinks_at_corners_without_diagonals() {
        // Given: an L-shaped mask boundary built from unit segments, scrambled.
        // When: the dashes are sampled across more than one animation period
        // (period = MASK_DASH_PX + MASK_GAP_PX = 4 px at 3 px/s ≈ 1333 ms).
        // Then: no emitted piece cuts across a corner — every piece is
        // axis-aligned and has both endpoints on one and the same input
        // segment — and at least one dash actually kinks: two consecutive
        // pieces meeting at a shared vertex with different orientations.
        let ordered = l_shape_unit_segments();
        let scrambled = scramble_from(ordered.clone(), (0, 0));
        let mut saw_kink = false;
        for t in 0..1400u64 {
            let pieces = mask_dash_segments(&scrambled, t);
            for &((x0, y0), (x1, y1)) in &pieces {
                assert!(
                    (x1 - x0).abs() < 1e-4 || (y1 - y0).abs() < 1e-4,
                    "diagonal piece ({x0},{y0})→({x1},{y1}) cuts a corner at t={t}"
                );
                assert!(
                    on_same_segment(&ordered, (x0, y0), (x1, y1)),
                    "piece ({x0},{y0})→({x1},{y1}) spans two edges at t={t}"
                );
            }
            for window in pieces.windows(2) {
                let a = window[0];
                let b = window[1];
                // Consecutive pieces of the same dash connect end-to-start.
                if (a.1 .0 - b.0 .0).abs() < 1e-4 && (a.1 .1 - b.0 .1).abs() < 1e-4 {
                    let a_horizontal = (a.1 .1 - a.0 .1).abs() < 1e-4;
                    let b_horizontal = (b.1 .1 - b.0 .1).abs() < 1e-4;
                    if a_horizontal != b_horizontal {
                        saw_kink = true;
                    }
                }
            }
        }
        assert!(
            saw_kink,
            "expected at least one dash to kink at a corner with different orientations"
        );
    }

    #[test]
    fn mask_dash_edge_pieces_wrap_the_closing_corner() {
        // Given: the ordered L polyline and its edge lengths.
        // When: a dash interval is split that runs past the loop end (the
        // helper's wrap path; the walk itself clamps to `total`).
        // Then: it is emitted as edge-local pieces that meet at the closing
        // corner `path[0]`, and none is a diagonal.
        let path = mask_polyline(&l_shape_unit_segments());
        let n = path.len();
        let mut edge_len = Vec::with_capacity(n);
        let mut total = 0.0_f32;
        for i in 0..n {
            let (x0, y0) = path[i];
            let (x1, y1) = path[(i + 1) % n];
            let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
            edge_len.push(len);
            total += len;
        }
        let pieces = dash_edge_pieces(&path, &edge_len, total, total - 1.0, total + 1.0);
        assert!(!pieces.is_empty());
        for &((x0, y0), (x1, y1)) in &pieces {
            assert!(
                (x1 - x0).abs() < 1e-4 || (y1 - y0).abs() < 1e-4,
                "wrapped piece ({x0},{y0})→({x1},{y1}) is diagonal"
            );
        }
        // The piece ending at the loop end and the piece starting at the loop
        // start both land on the closing corner `path[0]`.
        let meets_closing = pieces
            .iter()
            .any(|&(_, (x1, y1))| (x1 - path[0].0).abs() < 1e-4 && (y1 - path[0].1).abs() < 1e-4)
            && pieces.iter().any(|&((x0, y0), _)| {
                (x0 - path[0].0).abs() < 1e-4 && (y0 - path[0].1).abs() < 1e-4
            });
        assert!(
            meets_closing,
            "the wrapped dash must kink at the closing corner {path0:?}",
            path0 = path[0]
        );
    }

    // -----------------------------------------------------------------------
    // mask_polylines / multi-component mask_dash_segments
    // -----------------------------------------------------------------------

    /// Total arc length of a dash-piece list.
    fn total_length(pieces: &[((f32, f32), (f32, f32))]) -> f32 {
        pieces
            .iter()
            .map(|&((x0, y0), (x1, y1))| ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt())
            .sum()
    }

    #[test]
    fn mask_polylines_empty_is_empty() {
        assert!(mask_polylines(&[]).is_empty());
        assert!(mask_polyline(&[]).is_empty());
    }

    #[test]
    fn mask_polylines_single_component_matches_mask_polyline() {
        // Given: a single closed L boundary in scrambled order.
        // When: the per-component loops are computed.
        // Then: there is exactly one loop, it is clockwise, and the flat
        // `mask_polyline` returns that same loop unchanged.
        let scrambled = scramble_from(l_shape_unit_segments(), (0, 0));
        let loops = mask_polylines(&scrambled);
        assert_eq!(loops.len(), 1, "a single closed boundary yields one loop");
        assert!(signed_area2(&loops[0]) < 0.0, "the loop must be clockwise");
        assert_eq!(
            mask_polyline(&scrambled),
            loops[0],
            "single-component mask_polyline must be unchanged"
        );
    }

    #[test]
    fn mask_polylines_recovers_corner_pinched_loops() {
        // Given: two 1×1 pixels touching only at the corner (1,1) — one
        // connected boundary component pinched at that shared vertex.
        // When: the loops are extracted.
        // Then: the pinch is split into two clockwise loops instead of
        // returning nothing, and dashes are still produced.
        let mut segments = Vec::new();
        for (px, py) in [(0, 0), (1, 1)] {
            segments.extend(translate_unit_segments(&square_unit_segments(1), px, py));
        }
        let loops = mask_polylines(&segments);
        assert_eq!(
            loops.len(),
            2,
            "a corner-pinched component must split into two loops"
        );
        for path in &loops {
            assert_eq!(path.len(), 4);
            assert!(signed_area2(path) < 0.0);
        }
        assert!(!mask_dash_segments(&segments, 0).is_empty());
    }

    #[test]
    fn mask_polylines_splits_disjoint_components() {
        // Given: two disjoint 4×4 squares combined and scrambled together.
        // When: the loops are computed.
        // Then: one clockwise 16-vertex loop per square, and no loop mixes the
        // two components.
        let comp_a = square_unit_segments(4); // (0,0)..(4,4)
        let comp_b = translate_unit_segments(&square_unit_segments(4), 10, 10); // (10,10)..(14,14)
        let mut combined = comp_a.clone();
        combined.extend(comp_b.iter().copied());
        let scrambled = scramble_from(combined, (0, 0));

        let loops = mask_polylines(&scrambled);
        assert_eq!(loops.len(), 2, "two disjoint squares must yield two loops");
        for path in &loops {
            assert_eq!(path.len(), 16, "each 4×4 square has 16 unit edges");
            assert!(signed_area2(path) < 0.0, "each component must be clockwise");
            assert!(
                path.iter().all(|&(x, _)| x <= 4.0 || x >= 10.0),
                "a loop must not mix vertices from both components: {path:?}"
            );
        }
    }

    #[test]
    fn mask_dash_segments_covers_disjoint_components() {
        // Given: two disjoint squares and their combined, scrambled boundary.
        // When: the dashes are computed across the animation period.
        // Then: every piece is axis-aligned and lies on a single input segment
        // (no chord jumps between the components), both components receive
        // dashes, and at t=0 the total dash length is half of both perimeters.
        let comp_a = square_unit_segments(4);
        let comp_b = translate_unit_segments(&square_unit_segments(4), 10, 10);
        let mut combined = comp_a.clone();
        combined.extend(comp_b.iter().copied());
        let scrambled = scramble_from(combined.clone(), (0, 0));

        let in_a = |p: (f32, f32)| (0.0..=4.0).contains(&p.0) && (0.0..=4.0).contains(&p.1);
        let in_b = |p: (f32, f32)| (10.0..=14.0).contains(&p.0) && (10.0..=14.0).contains(&p.1);

        for t in [0u64, 137, 500, 1_333] {
            let dashes = mask_dash_segments(&scrambled, t);
            assert!(
                !dashes.is_empty(),
                "multi-component dashes must not be empty at t={t}"
            );
            assert!(
                dashes.iter().any(|&(p0, p1)| in_a(p0) && in_a(p1)),
                "no dash covers component A at t={t}"
            );
            assert!(
                dashes.iter().any(|&(p0, p1)| in_b(p0) && in_b(p1)),
                "no dash covers component B at t={t}"
            );
            for &(p0, p1) in &dashes {
                assert!(
                    on_same_segment(&combined, p0, p1),
                    "dash {p0:?}→{p1:?} jumps between components at t={t}"
                );
                assert!(
                    (p1.0 - p0.0).abs() < 1e-4 || (p1.1 - p0.1).abs() < 1e-4,
                    "dash {p0:?}→{p1:?} is diagonal at t={t}"
                );
            }
        }

        // Each 16 px perimeter spans 4 mask periods, so exactly half is dash:
        // 8 + 8 = 16.
        let dashes = mask_dash_segments(&scrambled, 0);
        assert!(
            (total_length(&dashes) - 16.0).abs() < 1e-3,
            "total dash {} != 16",
            total_length(&dashes)
        );
    }

    #[test]
    fn mask_dash_segments_covers_hole_and_outer_loop() {
        // Given: an annular selection — a 6×6 outer boundary plus a 2×2 hole
        // loop — combined and scrambled.
        // When: the loops and dashes are computed.
        // Then: the hole is its own clockwise loop, both the outer boundary and
        // the hole receive dashes, and no piece spans the two loops.
        let outer = square_unit_segments(6); // (0,0)..(6,6), perimeter 24
        let hole = translate_unit_segments(&square_unit_segments(2), 2, 2); // (2,2)..(4,4), perimeter 8
        let mut combined = outer.clone();
        combined.extend(hole.iter().copied());
        let scrambled = scramble_from(combined.clone(), (0, 0));

        let loops = mask_polylines(&scrambled);
        assert_eq!(
            loops.len(),
            2,
            "an annular selection yields outer + hole loops"
        );
        for path in &loops {
            assert!(
                signed_area2(path) < 0.0,
                "outer and hole loops both follow the clockwise convention"
            );
        }

        let dashes = mask_dash_segments(&scrambled, 0);
        assert!(!dashes.is_empty());
        assert!(
            dashes
                .iter()
                .any(|&(p0, p1)| on_any_segment(&outer, p0) && on_any_segment(&outer, p1)),
            "no dash covers the outer loop"
        );
        assert!(
            dashes
                .iter()
                .any(|&(p0, p1)| on_any_segment(&hole, p0) && on_any_segment(&hole, p1)),
            "no dash covers the hole loop"
        );
        for &(p0, p1) in &dashes {
            assert!(
                on_same_segment(&combined, p0, p1),
                "a dash {p0:?}→{p1:?} spans the outer loop and the hole"
            );
        }
        // 24/2 + 8/2 = 16.
        assert!(
            (total_length(&dashes) - 16.0).abs() < 1e-3,
            "total dash {} != 16",
            total_length(&dashes)
        );
    }

    #[test]
    fn mask_polyline_multi_component_has_no_cross_chords() {
        // Given: two disjoint squares.
        // When: the flat, single-loop `mask_polyline` representation is needed.
        // Then: it is the first component's loop verbatim — a real loop whose
        // every edge lies on one input segment — so the consumer's
        // `path[i] -> path[(i+1)%n]` walk cannot draw a chord between the
        // components.
        let mut combined = square_unit_segments(4);
        combined.extend(translate_unit_segments(&square_unit_segments(4), 10, 10));
        let scrambled = scramble_from(combined.clone(), (0, 0));

        let loops = mask_polylines(&scrambled);
        assert_eq!(loops.len(), 2);
        let flat = mask_polyline(&scrambled);
        assert_eq!(
            flat, loops[0],
            "multi-component returns the first loop verbatim"
        );
        assert_eq!(flat.len(), 16);
        for i in 0..flat.len() {
            assert!(
                on_same_segment(&combined, flat[i], flat[(i + 1) % flat.len()]),
                "flat multi-component path has a cross-component chord"
            );
        }
    }

    #[test]
    fn ant_const_period_sanity() {
        assert_eq!(ANT_DASH_PX + ANT_GAP_PX, 6.0);
    }

    // -----------------------------------------------------------------------
    // mask_dash_segments_from_polylines (Task II: O(n) cached polylines)
    // -----------------------------------------------------------------------

    /// A spread of real mask boundaries: single-component square and L, a
    /// corner-pinched pair, disjoint blobs, and an annular (hole) selection.
    fn dash_fixture_boundaries() -> Vec<Vec<BoundarySegment>> {
        let l = scramble_from(l_shape_unit_segments(), (0, 0));
        let square = scramble_from(square_unit_segments(12), (0, 0));

        let mut pinched = Vec::new();
        for (px, py) in [(0, 0), (1, 1)] {
            pinched.extend(translate_unit_segments(&square_unit_segments(1), px, py));
        }
        let pinched = scramble_from(pinched, (0, 0));

        let mut disjoint = square_unit_segments(4);
        disjoint.extend(translate_unit_segments(&square_unit_segments(4), 10, 10));
        let disjoint = scramble_from(disjoint, (0, 0));

        let mut ring = square_unit_segments(6);
        ring.extend(translate_unit_segments(&square_unit_segments(2), 2, 2));
        let ring = scramble_from(ring, (0, 0));

        vec![l, square, pinched, disjoint, ring]
    }

    /// The timestamps exercised by the preservation tests — phase 0, mid-dash,
    /// mid-gap, period wraps and several animation seconds.
    const DASH_TEST_TIMES: [u64; 10] = [0, 1, 137, 500, 999, 1_000, 1_234, 2_000, 4_000, 123_456];

    #[test]
    fn mask_dash_segments_from_polylines_matches_wrapper() {
        // Given: real mask boundaries and a range of animation phases.
        // When: the dashes come from the prechained polylines vs. the historical
        // segments entry point (which chains then walks).
        // Then: the results are point-identical — the wrapper is exactly the
        // chain plus the new walk.
        for segs in dash_fixture_boundaries() {
            let polylines = mask_polylines(&segs);
            for t in DASH_TEST_TIMES {
                assert_eq!(
                    mask_dash_segments_from_polylines(&polylines, t),
                    mask_dash_segments(&segs, t),
                    "t={t}"
                );
            }
        }
    }

    #[test]
    fn mask_dash_segments_from_polylines_matches_legacy_walk() {
        // Given: the same real mask boundaries and animation phases.
        // When: the new O(n) cursor walk is compared with the historical
        // per-dash edge rescan (`legacy_mask_dash_segments_from_polylines`).
        // Then: every emitted point is bit-identical — the refactor preserved
        // output exactly, including corner kinks, phase and multi-component
        // ordering.
        for segs in dash_fixture_boundaries() {
            let polylines = mask_polylines(&segs);
            for t in DASH_TEST_TIMES {
                assert_eq!(
                    mask_dash_segments_from_polylines(&polylines, t),
                    legacy_mask_dash_segments_from_polylines(&polylines, t),
                    "t={t}"
                );
            }
            // Dense sweep across several animation periods: floating point can
            // produce width-epsilon dashes exactly at a period boundary, and
            // those must survive the cursor walk unchanged.
            for t in (0u64..=4_000).step_by(7) {
                assert_eq!(
                    mask_dash_segments_from_polylines(&polylines, t),
                    legacy_mask_dash_segments_from_polylines(&polylines, t),
                    "dense t={t}"
                );
            }
            for t in (10_000u64..=40_000).step_by(97) {
                assert_eq!(
                    mask_dash_segments_from_polylines(&polylines, t),
                    legacy_mask_dash_segments_from_polylines(&polylines, t),
                    "wide t={t}"
                );
            }
        }
    }

    #[test]
    fn mask_dash_segments_from_polylines_degenerate_is_empty() {
        assert!(mask_dash_segments_from_polylines(&[], 0).is_empty());
        // Fewer than two vertices.
        assert!(mask_dash_segments_from_polylines(&[vec![(0.0, 0.0)]], 0).is_empty());
        // Zero perimeter: every vertex coincides.
        assert!(
            mask_dash_segments_from_polylines(&[vec![(1.0, 1.0), (1.0, 1.0), (1.0, 1.0)]], 0)
                .is_empty()
        );
    }

    #[test]
    fn mask_dash_segments_from_polylines_scales_to_large_perimeter() {
        // Correctness at scale: the cached-polyline walk must reproduce the
        // historical walk on a large boundary (side 256 → perimeter 1024).
        let polylines = mask_polylines(&square_unit_segments(256));
        assert_eq!(
            mask_dash_segments_from_polylines(&polylines, 1_234),
            legacy_mask_dash_segments_from_polylines(&polylines, 1_234),
            "the O(n) walk must match the legacy walk on a large perimeter"
        );

        // Speed guard: the historical O(n²/period) walk on the measured hotspot
        // (side 1024 → perimeter 4096) took ~28.8 ms/frame; the O(n) walk must
        // finish well inside a deliberately generous absolute bound (a linear
        // walk is orders of magnitude below it, so this only trips on a
        // quadratic regression, never on CI scheduling jitter).
        let polylines_1024 = mask_polylines(&square_unit_segments(1024));
        let start = std::time::Instant::now();
        let dashes = mask_dash_segments_from_polylines(&polylines_1024, 1_234);
        let elapsed = start.elapsed().as_secs_f64();
        assert!(!dashes.is_empty(), "the large perimeter must yield dashes");
        println!(
            "mask_dash_segments_from_polylines: perimeter 4096 → {} pieces in {elapsed:.6}s",
            dashes.len()
        );
        assert!(
            elapsed < 10.0,
            "the O(n) walk must stay under a generous bound; took {elapsed}s"
        );
    }
}
