//! Camera — integer-snapped zoom (10–3200%) and pan state.
//!
//! Coordinate convention: a canvas pixel at canvas coord `c` spans screen pixels
//! `[pan + c*scale, pan + (c+1)*scale)`.
//!
//! Zoom levels are integer percentages in `10..=3200` (nearest-neighbor everywhere).

use crate::core::math::Rect2i;

/// Integer-snapped zoom (10%–3200%) and pan state.
///
/// `zoom_percent` is an integer percentage in `[MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT]`;
/// `pan` is the canvas-space offset of the viewport's top-left corner, in canvas
/// pixels. All screen↔canvas mapping is nearest-neighbor (floor/round), never filtered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Camera {
    zoom_percent: u32,
    pan_x: i32,
    pan_y: i32,
}

impl Camera {
    /// Minimum zoom: 10%.
    pub const MIN_ZOOM_PERCENT: u32 = 10;
    /// Maximum zoom: 3200%.
    pub const MAX_ZOOM_PERCENT: u32 = 3200;

    /// New camera: 100% zoom, pan (0, 0).
    pub fn new() -> Self {
        Self {
            zoom_percent: 100,
            pan_x: 0,
            pan_y: 0,
        }
    }

    /// Current zoom as an integer percentage.
    pub fn zoom_percent(&self) -> u32 {
        self.zoom_percent
    }

    /// Set the zoom, clamped to `[MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT]`.
    pub fn set_zoom_percent(&mut self, pct: u32) {
        self.zoom_percent = pct.clamp(Self::MIN_ZOOM_PERCENT, Self::MAX_ZOOM_PERCENT);
    }

    /// Step to the next integer zoom level (100 → 101). No-op at `MAX_ZOOM_PERCENT`.
    pub fn zoom_in(&mut self) {
        if self.zoom_percent < Self::MAX_ZOOM_PERCENT {
            self.zoom_percent += 1;
        }
    }

    /// Step to the previous integer zoom level (100 → 99). No-op at `MIN_ZOOM_PERCENT`.
    pub fn zoom_out(&mut self) {
        if self.zoom_percent > Self::MIN_ZOOM_PERCENT {
            self.zoom_percent -= 1;
        }
    }

    /// Zoom as a float scale factor (`zoom_percent / 100`).
    pub fn scale(&self) -> f64 {
        self.zoom_percent as f64 / 100.0
    }

    /// Current pan: canvas-space offset of the viewport's top-left corner.
    pub fn pan(&self) -> (i32, i32) {
        (self.pan_x, self.pan_y)
    }

    /// Set the pan directly.
    pub fn set_pan(&mut self, x: i32, y: i32) {
        self.pan_x = x;
        self.pan_y = y;
    }

    /// Pan by a delta in canvas pixels.
    pub fn pan_by(&mut self, dx: i32, dy: i32) {
        self.pan_x += dx;
        self.pan_y += dy;
    }

    /// Map a screen (viewport) position to a canvas pixel coordinate.
    ///
    /// `floor((s - pan) / scale)`, computed with exact integer math:
    /// `((s - pan) * 100).div_euclid(zoom)` — `div_euclid` is floor division for a
    /// positive divisor. Floor is the correct nearest-neighbor semantics: it aligns
    /// the pixel grid so a canvas pixel at coord `c` occupies screen pixels
    /// `[pan + c*scale, pan + (c+1)*scale)`.
    pub fn screen_to_canvas(&self, sx: i32, sy: i32) -> (i32, i32) {
        let zoom = self.zoom_percent as i64;
        let cx = ((sx as i64 - self.pan_x as i64) * 100).div_euclid(zoom);
        let cy = ((sy as i64 - self.pan_y as i64) * 100).div_euclid(zoom);
        (sat_i32(cx), sat_i32(cy))
    }

    /// Map a canvas pixel coordinate to a screen (viewport) position.
    ///
    /// `round(c * scale + pan)`, computed with exact integer math
    /// (`round-half-away-from-zero`, matching `f64::round`). At 100% zoom this is the
    /// exact inverse of [`Self::screen_to_canvas`].
    pub fn canvas_to_screen(&self, cx: i32, cy: i32) -> (i32, i32) {
        let zoom = self.zoom_percent as i64;
        let sx = round_div(cx as i64 * zoom + self.pan_x as i64 * 100, 100);
        let sy = round_div(cy as i64 * zoom + self.pan_y as i64 * 100, 100);
        (sat_i32(sx), sat_i32(sy))
    }

    /// The canvas-space rect visible in a `view_w × view_h` viewport.
    ///
    /// Convention: anchored at the pan position (the canvas coordinate under the
    /// viewport's top-left corner) and spanning `ceil(view_w / scale)` ×
    /// `ceil(view_h / scale)` canvas pixels, each dimension clamped to ≥ 1. Because
    /// pan is an integer, the viewport's left/top edge is always grid-aligned, so this
    /// is exactly the set of canvas pixels whose screen spans intersect the viewport;
    /// the `ceil` covers the right/bottom edge where the viewport may cut through a
    /// pixel. Computed with exact integer math: `ceil(v / scale) = (v*100 + zoom - 1) / zoom`.
    pub fn canvas_visible_rect(&self, view_w: i32, view_h: i32) -> Rect2i {
        let zoom = self.zoom_percent as i64;
        let w = ((view_w as i64 * 100 + zoom - 1) / zoom).max(1) as i32;
        let h = ((view_h as i64 * 100 + zoom - 1) / zoom).max(1) as i32;
        Rect2i::new(self.pan_x, self.pan_y, w, h)
    }

    /// Integer zoom percentage that fits the whole canvas in the viewport.
    ///
    /// `floor(min(100 * view_w / canvas_w, 100 * view_h / canvas_h))`, clamped to
    /// `[MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT]`. The slower axis (larger canvas /
    /// smaller viewport) wins, so the whole canvas is visible with letterboxing on the
    /// other axis. A 0 result from integer math on oversized canvases clamps to
    /// `MIN_ZOOM_PERCENT`.
    pub fn fit_zoom_percent(canvas_w: i32, canvas_h: i32, view_w: i32, view_h: i32) -> u32 {
        let cw = canvas_w.max(1) as i64;
        let ch = canvas_h.max(1) as i64;
        let vw = view_w.max(0) as i64;
        let vh = view_h.max(0) as i64;
        let by_w = 100 * vw / cw;
        let by_h = 100 * vh / ch;
        by_w.min(by_h)
            .clamp(Self::MIN_ZOOM_PERCENT as i64, Self::MAX_ZOOM_PERCENT as i64) as u32
    }

    /// Zoom around a canvas anchor point (e.g. the cursor), keeping that canvas point
    /// under the same viewport position.
    ///
    /// `pan' = anchor_screen - anchor_canvas * scale'` where
    /// `anchor_screen = anchor_canvas * scale_old + pan_old`, computed in f64 and
    /// rounded to integer pan. `new_zoom` is clamped to `[MIN_ZOOM_PERCENT, MAX_ZOOM_PERCENT]`.
    pub fn anchor_zoom(&mut self, anchor_canvas: (i32, i32), new_zoom: u32) {
        let new_zoom = new_zoom.clamp(Self::MIN_ZOOM_PERCENT, Self::MAX_ZOOM_PERCENT);
        let (acx, acy) = anchor_canvas;
        let scale_old = self.scale();
        let scale_new = new_zoom as f64 / 100.0;
        let anchor_sx = acx as f64 * scale_old + self.pan_x as f64;
        let anchor_sy = acy as f64 * scale_old + self.pan_y as f64;
        self.pan_x = (anchor_sx - acx as f64 * scale_new).round() as i32;
        self.pan_y = (anchor_sy - acy as f64 * scale_new).round() as i32;
        self.zoom_percent = new_zoom;
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

/// `n / d` rounded half away from zero (matches `f64::round`), for `d > 0`.
fn round_div(n: i64, d: i64) -> i64 {
    let q = n / d;
    let r = n % d;
    if 2 * r.abs() >= d {
        q + if n >= 0 { 1 } else { -1 }
    } else {
        q
    }
}

/// Saturate an i64 to the i32 range (deterministic for any input).
fn sat_i32(v: i64) -> i32 {
    v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math::Rect2i;

    #[test]
    fn default_state() {
        let cam = Camera::new();
        assert_eq!(cam.zoom_percent(), 100);
        assert_eq!(cam.pan(), (0, 0));
        assert_eq!(cam.scale(), 1.0);
    }

    #[test]
    fn zoom_clamping() {
        let mut cam = Camera::new();
        cam.set_zoom_percent(5);
        assert_eq!(cam.zoom_percent(), Camera::MIN_ZOOM_PERCENT);
        cam.set_zoom_percent(99_999);
        assert_eq!(cam.zoom_percent(), Camera::MAX_ZOOM_PERCENT);
        cam.set_zoom_percent(150);
        assert_eq!(cam.zoom_percent(), 150);
    }

    #[test]
    fn zoom_in_out_steps() {
        let mut cam = Camera::new();
        cam.zoom_in();
        assert_eq!(cam.zoom_percent(), 101);
        cam.zoom_out();
        assert_eq!(cam.zoom_percent(), 100);
        cam.zoom_out();
        assert_eq!(cam.zoom_percent(), 99);
    }

    #[test]
    fn zoom_boundaries_noop() {
        let mut cam = Camera::new();
        cam.set_zoom_percent(Camera::MIN_ZOOM_PERCENT);
        cam.zoom_out();
        assert_eq!(cam.zoom_percent(), Camera::MIN_ZOOM_PERCENT);
        cam.set_zoom_percent(Camera::MAX_ZOOM_PERCENT);
        cam.zoom_in();
        assert_eq!(cam.zoom_percent(), Camera::MAX_ZOOM_PERCENT);
    }

    #[test]
    fn screen_to_canvas_at_100() {
        let cam = Camera::new();
        assert_eq!(cam.screen_to_canvas(0, 0), (0, 0));
        assert_eq!(cam.screen_to_canvas(7, 3), (7, 3));
        assert_eq!(cam.screen_to_canvas(-1, -1), (-1, -1));
    }

    #[test]
    fn screen_to_canvas_at_100_with_pan() {
        let mut cam = Camera::new();
        cam.set_pan(10, 20);
        assert_eq!(cam.screen_to_canvas(10, 20), (0, 0));
        assert_eq!(cam.screen_to_canvas(17, 23), (7, 3));
        assert_eq!(cam.screen_to_canvas(9, 19), (-1, -1));
    }

    #[test]
    fn screen_to_canvas_at_200_nearest_neighbor() {
        let mut cam = Camera::new();
        cam.set_zoom_percent(200);
        // Canvas pixel 0 covers screen [0, 2): floor semantics.
        assert_eq!(cam.screen_to_canvas(0, 0), (0, 0));
        assert_eq!(cam.screen_to_canvas(1, 0), (0, 0));
        assert_eq!(cam.screen_to_canvas(2, 0), (1, 0));
        assert_eq!(cam.screen_to_canvas(3, 0), (1, 0));
        assert_eq!(cam.screen_to_canvas(4, 0), (2, 0));
        // Negative screen coords floor (not truncate) toward -inf.
        assert_eq!(cam.screen_to_canvas(-1, 0), (-1, 0));
        assert_eq!(cam.screen_to_canvas(-2, 0), (-1, 0));
        assert_eq!(cam.screen_to_canvas(-3, 0), (-2, 0));
    }

    #[test]
    fn canvas_to_screen_inverse_at_100() {
        let cam = Camera::new();
        assert_eq!(cam.canvas_to_screen(1, 0), (1, 0));
        assert_eq!(cam.canvas_to_screen(7, 3), (7, 3));
        // At 100% zoom canvas_to_screen is the exact inverse of screen_to_canvas.
        for s in -5..=5 {
            let (cx, cy) = cam.screen_to_canvas(s, s);
            assert_eq!(cam.canvas_to_screen(cx, cy), (s, s));
        }
    }

    #[test]
    fn round_trip_at_100_with_pan() {
        let mut cam = Camera::new();
        cam.set_pan(10, 20);
        for s in -5..=5 {
            let (cx, cy) = cam.screen_to_canvas(s, s);
            assert_eq!(cam.canvas_to_screen(cx, cy), (s, s));
        }
    }

    #[test]
    fn canvas_to_screen_at_200() {
        let mut cam = Camera::new();
        cam.set_zoom_percent(200);
        assert_eq!(cam.canvas_to_screen(1, 0), (2, 0));
        assert_eq!(cam.canvas_to_screen(0, 0), (0, 0));
        assert_eq!(cam.canvas_to_screen(3, 2), (6, 4));
        // Rounding: canvas 1 at 150% with pan 0 → round(1.5) = 2.
        cam.set_zoom_percent(150);
        assert_eq!(cam.canvas_to_screen(1, 0), (2, 0));
    }

    #[test]
    fn canvas_visible_rect_at_100() {
        let cam = Camera::new();
        assert_eq!(cam.canvas_visible_rect(50, 40), Rect2i::new(0, 0, 50, 40));
        let mut cam = Camera::new();
        cam.set_pan(5, -3);
        assert_eq!(cam.canvas_visible_rect(50, 40), Rect2i::new(5, -3, 50, 40));
    }

    #[test]
    fn canvas_visible_rect_at_200() {
        let mut cam = Camera::new();
        cam.set_zoom_percent(200);
        // Convention: anchored at pan, w = ceil(view_w / scale) = ceil(50/2) = 25,
        // h = ceil(40/2) = 20 (each clamped to ≥ 1).
        assert_eq!(cam.canvas_visible_rect(50, 40), Rect2i::new(0, 0, 25, 20));
    }

    #[test]
    fn fit_zoom_percent_cases() {
        // 32×32 canvas in 320×320 view: floor(min(100*320/32, 100*320/32)) = 1000.
        assert_eq!(Camera::fit_zoom_percent(32, 32, 320, 320), 1000);
        // 100×100 in 50×50: 100*50/100 = 50.
        assert_eq!(Camera::fit_zoom_percent(100, 100, 50, 50), 50);
        // 10×10 in 3200×3200: 100*3200/10 = 32000 → clamped to MAX.
        assert_eq!(
            Camera::fit_zoom_percent(10, 10, 3200, 3200),
            Camera::MAX_ZOOM_PERCENT
        );
        // 10×10 in 320×320: 100*320/10 = 3200.
        assert_eq!(Camera::fit_zoom_percent(10, 10, 320, 320), 3200);
        // 100×10 in 1000×100: by width 100*1000/100 = 1000, by height 100*100/10 = 1000.
        assert_eq!(Camera::fit_zoom_percent(100, 10, 1000, 100), 1000);
        // Oversized canvas: 100*100/10000 = 1 → clamped to MIN.
        assert_eq!(
            Camera::fit_zoom_percent(10000, 10000, 100, 100),
            Camera::MIN_ZOOM_PERCENT
        );
        // Slower axis wins: 200×100 in 100×100 → by width 50, by height 100 → 50.
        assert_eq!(Camera::fit_zoom_percent(200, 100, 100, 100), 50);
    }

    #[test]
    fn anchor_zoom_keeps_point() {
        let mut cam = Camera::new();
        // Canvas point (10,10) at 100% pan (0,0) sits at screen (10,10).
        cam.anchor_zoom((10, 10), 200);
        // pan' = anchor_screen - anchor_canvas * scale' = 10 - 10*2 = -10.
        assert_eq!(cam.zoom_percent(), 200);
        assert_eq!(cam.pan(), (-10, -10));
        // The point stays under the same viewport position.
        assert_eq!(cam.canvas_to_screen(10, 10), (10, 10));
    }

    #[test]
    fn anchor_zoom_clamps_zoom() {
        let mut cam = Camera::new();
        cam.anchor_zoom((0, 0), 99_999);
        assert_eq!(cam.zoom_percent(), Camera::MAX_ZOOM_PERCENT);
    }

    #[test]
    fn integer_zoom_exactness() {
        for pct in [10u32, 100, 3200] {
            let mut cam = Camera::new();
            cam.set_zoom_percent(pct);
            assert_eq!(cam.scale(), pct as f64 / 100.0);
        }
    }

    #[test]
    fn pan_by_moves() {
        let mut cam = Camera::new();
        cam.pan_by(3, -5);
        assert_eq!(cam.pan(), (3, -5));
        cam.pan_by(-3, 5);
        assert_eq!(cam.pan(), (0, 0));
    }
}
