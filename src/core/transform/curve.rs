//! Dynamic-line curve transform (Phase 2) — a bendable spline through two
//! fixed anchors and four draggable gizmos.
//!
//! A [`CurveTransform`] starts as a straight line: the four gizmos sit at the
//! 1/5, 2/5, 3/5 and 4/5 points, splitting the line into five equal segments.
//! Dragging a gizmo moves that control point; the object is then the uniform
//! Catmull-Rom spline through the six control points
//! `[start, g1, g2, g3, g4, end]`. The first/last segments duplicate the
//! endpoint as their phantom control point, the standard endpoint rule.
//!
//! Flattening is deterministic: each segment is sampled at roughly two points
//! per canvas pixel of chord length (at least two), so the polyline is stable
//! while dragging and the stroke rasterizer covers it without gaps. Pure: no
//! egui, no I/O, no randomness.

use crate::core::math::Rect2i;

/// Number of draggable gizmos along the line.
pub const GIZMO_COUNT: usize = 4;
/// Number of equal segments the gizmos split the line into.
pub const SEGMENT_COUNT: usize = GIZMO_COUNT + 1;
/// Total control points: the two anchors plus the four gizmos.
pub const CONTROL_POINTS: usize = GIZMO_COUNT + 2;
/// Flattening density: sub-samples per canvas pixel of chord length.
const SAMPLES_PER_PIXEL: f32 = 2.0;
/// Gizmo hit radius in canvas pixels (mirrors the 8 px screen handles at 100%).
pub const GIZMO_HIT_RADIUS: f32 = 8.0;
/// Curve hit radius in canvas pixels (a double-click within this is "on the
/// object", not on empty space).
pub const CURVE_HIT_RADIUS: f32 = 2.0;

/// A bendable dynamic line: six canvas-space control points in the order
/// `[start, g1, g2, g3, g4, end]`.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveTransform {
    points: [(f32, f32); CONTROL_POINTS],
}

impl CurveTransform {
    /// A straight line from `start` to `end` with the four gizmos at the
    /// 1/5-spaced interior points (five equal segments).
    pub fn line(start: (i32, i32), end: (i32, i32)) -> Self {
        let s = (start.0 as f32, start.1 as f32);
        let e = (end.0 as f32, end.1 as f32);
        let mut points = [(0.0f32, 0.0f32); CONTROL_POINTS];
        points[0] = s;
        points[CONTROL_POINTS - 1] = e;
        for (index, point) in points
            .iter_mut()
            .enumerate()
            .take(CONTROL_POINTS - 1)
            .skip(1)
        {
            let t = index as f32 / SEGMENT_COUNT as f32;
            *point = (s.0 + (e.0 - s.0) * t, s.1 + (e.1 - s.1) * t);
        }
        Self { points }
    }

    /// All six control points, `[start, g1, g2, g3, g4, end]`.
    pub fn points(&self) -> &[(f32, f32); CONTROL_POINTS] {
        &self.points
    }

    /// The two fixed outer anchors `(start, end)`.
    pub fn anchors(&self) -> ((f32, f32), (f32, f32)) {
        (self.points[0], self.points[CONTROL_POINTS - 1])
    }

    /// The four draggable gizmos in order.
    pub fn gizmos(&self) -> [(f32, f32); GIZMO_COUNT] {
        let mut out = [(0.0f32, 0.0f32); GIZMO_COUNT];
        for (index, slot) in out.iter_mut().enumerate() {
            *slot = self.points[index + 1];
        }
        out
    }

    /// The gizmo at `index`, or `None` when out of range.
    pub fn gizmo(&self, index: usize) -> Option<(f32, f32)> {
        (index < GIZMO_COUNT).then_some(self.points[index + 1])
    }

    /// Moves the gizmo at `index` to `to`; out-of-range indices are ignored.
    pub fn set_gizmo(&mut self, index: usize, to: (f32, f32)) {
        if index < GIZMO_COUNT {
            self.points[index + 1] = to;
        }
    }

    /// The flattened spline as canvas-space float points.
    pub fn polyline(&self) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        for segment in 0..CONTROL_POINTS - 1 {
            let p0 = if segment == 0 {
                self.points[0]
            } else {
                self.points[segment - 1]
            };
            let p1 = self.points[segment];
            let p2 = self.points[segment + 1];
            let p3 = if segment + 2 < CONTROL_POINTS {
                self.points[segment + 2]
            } else {
                self.points[CONTROL_POINTS - 1]
            };
            let steps = segment_steps(p1, p2);
            // The first sample of a later segment repeats the previous
            // segment's last sample, so skip it to keep the polyline dense.
            let first = usize::from(segment != 0);
            for step in first..=steps {
                let t = step as f32 / steps as f32;
                out.push(catmull_rom(p0, p1, p2, p3, t));
            }
        }
        out
    }

    /// The integer canvas points a stroke rasterizer stamps along the curve.
    pub fn samples(&self) -> Vec<(i32, i32)> {
        let mut out: Vec<(i32, i32)> = Vec::new();
        for (x, y) in self.polyline() {
            let point = (x.round() as i32, y.round() as i32);
            if out.last() != Some(&point) {
                out.push(point);
            }
        }
        out
    }

    /// Index of the gizmo within `radius` canvas pixels of `pt`, nearest first.
    pub fn hit_gizmo(&self, pt: (f32, f32), radius: f32) -> Option<usize> {
        let radius2 = radius * radius;
        let mut best: Option<(usize, f32)> = None;
        for (index, gizmo) in self.gizmos().iter().enumerate() {
            let d2 = (gizmo.0 - pt.0).powi(2) + (gizmo.1 - pt.1).powi(2);
            if d2 <= radius2 && best.is_none_or(|(_, best2)| d2 < best2) {
                best = Some((index, d2));
            }
        }
        best.map(|(index, _)| index)
    }

    /// Shortest distance from `pt` to the flattened curve, in canvas pixels.
    pub fn distance_to_curve(&self, pt: (f32, f32)) -> f32 {
        let polyline = self.polyline();
        match polyline.len() {
            0 => f32::INFINITY,
            1 => point_distance(pt, polyline[0]),
            _ => polyline
                .windows(2)
                .map(|w| point_segment_distance(pt, w[0], w[1]))
                .fold(f32::INFINITY, f32::min),
        }
    }

    /// Whether `pt` is on a gizmo or on the curve itself (the double-click
    /// exclusion region): `false` means empty space.
    pub fn hits_object(&self, pt: (f32, f32)) -> bool {
        self.hit_gizmo(pt, GIZMO_HIT_RADIUS).is_some()
            || self.distance_to_curve(pt) <= CURVE_HIT_RADIUS
    }

    /// Axis-aligned bounding box of the flattened curve, inflated by `margin`
    /// canvas pixels; `None` for an empty polyline.
    pub fn bounds(&self, margin: i32) -> Option<Rect2i> {
        let polyline = self.polyline();
        let first = *polyline.first()?;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (first.0, first.1, first.0, first.1);
        for (x, y) in &polyline {
            min_x = min_x.min(*x);
            min_y = min_y.min(*y);
            max_x = max_x.max(*x);
            max_y = max_y.max(*y);
        }
        let min_x = min_x.floor() as i32 - margin;
        let min_y = min_y.floor() as i32 - margin;
        let max_x = max_x.ceil() as i32 + margin;
        let max_y = max_y.ceil() as i32 + margin;
        Some(Rect2i::new(min_x, min_y, max_x - min_x, max_y - min_y))
    }
}

/// Uniform Catmull-Rom interpolation for one axis.
fn catmull_axis(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * b
        + (c - a) * t
        + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2
        + (3.0 * b - a - 3.0 * c + d) * t3)
}

/// One point of the uniform Catmull-Rom spline through `p1..p2` with `p0`/`p3`
/// as neighbours.
fn catmull_rom(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    (
        catmull_axis(p0.0, p1.0, p2.0, p3.0, t),
        catmull_axis(p0.1, p1.1, p2.1, p3.1, t),
    )
}

/// Sample count for one segment: two per pixel of chord, at least two.
fn segment_steps(p1: (f32, f32), p2: (f32, f32)) -> usize {
    let chord = ((p2.0 - p1.0).powi(2) + (p2.1 - p1.1).powi(2)).sqrt();
    ((chord * SAMPLES_PER_PIXEL).ceil() as usize).max(2)
}

fn point_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Distance from `p` to the segment `a..b`.
fn point_segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let ab = (b.0 - a.0, b.1 - a.1);
    let length2 = ab.0 * ab.0 + ab.1 * ab.1;
    if length2 <= f32::EPSILON {
        return point_distance(p, a);
    }
    let ap = (p.0 - a.0, p.1 - a.1);
    let t = ((ap.0 * ab.0 + ap.1 * ab.1) / length2).clamp(0.0, 1.0);
    point_distance(p, (a.0 + ab.0 * t, a.1 + ab.1 * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_gizmos_split_the_line_into_five_equal_segments() {
        let curve = CurveTransform::line((10, 20), (110, 20));
        let gizmos = curve.gizmos();
        for (index, gizmo) in gizmos.iter().enumerate() {
            let t = (index + 1) as f32 / SEGMENT_COUNT as f32;
            assert!(
                (gizmo.0 - (10.0 + 100.0 * t)).abs() < 1e-4,
                "gizmo {index} x"
            );
            assert!((gizmo.1 - 20.0).abs() < 1e-4, "gizmo {index} y");
        }
        let expected: Vec<(f32, f32)> = (1..=4)
            .map(|k| (10.0 + 100.0 * k as f32 / 5.0, 20.0))
            .collect();
        assert_eq!(gizmos.to_vec(), expected);
    }

    #[test]
    fn straight_line_samples_are_collinear_and_contiguous() {
        let curve = CurveTransform::line((0, 0), (20, 0));
        let samples = curve.samples();
        assert_eq!(samples.first(), Some(&(0, 0)));
        assert_eq!(samples.last(), Some(&(20, 0)));
        for (index, point) in samples.iter().enumerate() {
            assert_eq!(point.1, 0, "sample {index} left the line: {point:?}");
            if let Some(prev) = index.checked_sub(1).and_then(|i| samples.get(i)) {
                assert_eq!(
                    point.0 - prev.0,
                    1,
                    "samples must be adjacent: {prev:?} -> {point:?}"
                );
            }
        }
    }

    #[test]
    fn dragging_a_gizmo_bends_the_curve_through_it() {
        let mut curve = CurveTransform::line((0, 0), (100, 0));
        curve.set_gizmo(1, (40.0, 30.0));
        assert_eq!(curve.gizmo(1), Some((40.0, 30.0)));
        let samples = curve.samples();
        assert!(
            samples.iter().any(|&(_, y)| y >= 20),
            "the bent curve must reach the moved gizmo's height"
        );
        // Both anchors are untouched.
        assert_eq!(curve.anchors(), ((0.0, 0.0), (100.0, 0.0)));
    }

    #[test]
    fn hit_gizmo_finds_the_nearest_within_the_radius() {
        let curve = CurveTransform::line((0, 0), (100, 0));
        assert_eq!(curve.hit_gizmo((20.0, 0.0), GIZMO_HIT_RADIUS), Some(0));
        assert_eq!(curve.hit_gizmo((60.0, 0.0), GIZMO_HIT_RADIUS), Some(2));
        assert_eq!(curve.hit_gizmo((50.0, 40.0), GIZMO_HIT_RADIUS), None);
    }

    #[test]
    fn hits_object_covers_gizmos_and_the_curve_only() {
        let curve = CurveTransform::line((0, 0), (100, 0));
        assert!(
            curve.hits_object((20.0, 0.0)),
            "a gizmo point is on the object"
        );
        assert!(
            curve.hits_object((37.0, 0.0)),
            "a curve point is on the object"
        );
        assert!(
            !curve.hits_object((50.0, 40.0)),
            "empty space is not on the object"
        );
    }

    #[test]
    fn bounds_inflate_the_curve_extent() {
        let curve = CurveTransform::line((10, 10), (30, 10));
        let bounds = curve.bounds(1).expect("a line has bounds");
        assert_eq!(bounds, Rect2i::new(9, 9, 22, 2));
    }
}
