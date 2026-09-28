//! Pure camera math for the dynamic nest.
//!
//! A [`NestCamera`] is a float scale plus a screen-space offset. It is
//! deliberately NOT `core::camera::Camera`: that one is integer-percent and
//! canvas-pixel specific (10-3200%, i32 pan), which does not fit arbitrary
//! nest content. All math here is pure and unit-tested; the dynamic nest
//! translates it into an egui transform layer each frame.

use egui::{pos2, Pos2, Rect, Vec2};

/// Float-scale/offset camera for nest content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NestCamera {
    scale: f32,
    offset: Vec2,
}

impl NestCamera {
    /// Smallest zoom: 0.1x.
    pub const MIN_SCALE: f32 = 0.1;
    /// Largest zoom: 16x.
    pub const MAX_SCALE: f32 = 16.0;

    /// Identity camera: 1x zoom, no offset.
    pub fn new() -> Self {
        Self {
            scale: 1.0,
            offset: Vec2::ZERO,
        }
    }

    /// The current zoom factor.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The screen-space translation applied after scaling.
    pub fn offset(&self) -> Vec2 {
        self.offset
    }

    /// Zooms by `factor`, keeping the world point under `pointer` fixed on
    /// screen. `origin` is the viewport's top-left in screen space.
    ///
    /// The scale is clamped to [`Self::MIN_SCALE`]..=[`Self::MAX_SCALE`]; when
    /// the clamp rejects the zoom the camera is left untouched, so the offset
    /// never drifts at a boundary.
    pub fn zoom_around(&mut self, pointer: Pos2, origin: Pos2, factor: f32) {
        let new_scale = (self.scale * factor).clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        if new_scale == self.scale {
            return;
        }
        // World point currently under the pointer, in world coordinates.
        let world = (pointer - origin - self.offset) / self.scale;
        self.scale = new_scale;
        // Re-anchor: the same world point must land back under the pointer.
        self.offset = pointer - origin - world * new_scale;
    }

    /// Pans by `delta` screen pixels.
    /// Resets the pan to the world origin at the current scale.
    pub fn reset_pan(&mut self) {
        self.offset = Vec2::ZERO;
    }

    /// Resets the zoom to 100% (1 canvas unit per point), keeping the pan.
    pub fn reset_zoom(&mut self) {
        self.scale = 1.0;
    }

    pub fn pan_by(&mut self, delta: Vec2) {
        self.offset += delta;
    }

    /// Maps a world-space point to screen space for the given viewport origin.
    pub fn world_to_screen(&self, origin: Pos2, world: Pos2) -> Pos2 {
        origin + self.offset + world.to_vec2() * self.scale
    }

    /// The world-space rect visible through a `viewport` whose top-left is
    /// `origin` — the clip rect for the transformed content layer. The offset
    /// shifts the visible region, so the clip starts at `-offset / scale`.
    pub fn world_viewport(&self, origin: Pos2, viewport: Vec2) -> Rect {
        let _ = origin;
        Rect::from_min_size(
            pos2(-self.offset.x / self.scale, -self.offset.y / self.scale),
            viewport / self.scale,
        )
    }
}

impl Default for NestCamera {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn assert_close(actual: Pos2, expected: Pos2) {
        let delta = actual - expected;
        assert!(
            delta.x.abs() < 1e-3 && delta.y.abs() < 1e-3,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn reset_pan_and_zoom_return_to_defaults() {
        let mut camera = NestCamera::new();
        camera.zoom_around(pos2(100.0, 100.0), pos2(10.0, 10.0), 2.5);
        camera.pan_by(vec2(-40.0, 25.0));
        assert!(camera.scale() != 1.0 && camera.offset() != Vec2::ZERO);

        camera.reset_zoom();
        assert_eq!(camera.scale(), 1.0);
        assert!(camera.offset() != Vec2::ZERO, "zoom reset keeps the pan");

        camera.reset_pan();
        assert_eq!(camera.offset(), Vec2::ZERO);
    }

    #[test]
    fn new_camera_is_identity() {
        let camera = NestCamera::new();
        assert_eq!(camera.scale(), 1.0);
        assert_eq!(camera.offset(), Vec2::ZERO);
        assert_eq!(
            camera.world_to_screen(pos2(10.0, 20.0), pos2(5.0, 6.0)),
            pos2(15.0, 26.0)
        );
    }

    #[test]
    fn zoom_around_keeps_the_world_point_under_the_pointer_fixed() {
        let mut camera = NestCamera::new();
        let origin = pos2(40.0, 30.0);
        let pointer = pos2(140.0, 130.0);
        // World point under the pointer before the zoom.
        let world = ((pointer - origin - camera.offset()) / camera.scale()).to_pos2();

        camera.zoom_around(pointer, origin, 2.0);

        assert_eq!(camera.scale(), 2.0);
        assert_close(camera.world_to_screen(origin, world), pointer);
    }

    #[test]
    fn zoom_around_zooms_out_symmetrically() {
        let mut camera = NestCamera::new();
        let origin = pos2(0.0, 0.0);
        let pointer = pos2(100.0, 100.0);
        let world = ((pointer - origin - camera.offset()) / camera.scale()).to_pos2();

        camera.zoom_around(pointer, origin, 0.5);

        assert_eq!(camera.scale(), 0.5);
        assert_close(camera.world_to_screen(origin, world), pointer);
    }

    #[test]
    fn zoom_around_clamps_to_max_scale_without_offset_drift() {
        let mut camera = NestCamera::new();
        camera.zoom_around(pos2(100.0, 100.0), pos2(0.0, 0.0), 100.0);
        assert_eq!(camera.scale(), NestCamera::MAX_SCALE);

        let offset_before = camera.offset();
        // Zooming in further at the boundary must not move the offset.
        camera.zoom_around(pos2(100.0, 100.0), pos2(0.0, 0.0), 2.0);
        assert_eq!(camera.scale(), NestCamera::MAX_SCALE);
        assert_eq!(camera.offset(), offset_before);
    }

    #[test]
    fn zoom_around_clamps_to_min_scale_without_offset_drift() {
        let mut camera = NestCamera::new();
        camera.zoom_around(pos2(100.0, 100.0), pos2(0.0, 0.0), 0.001);
        assert_eq!(camera.scale(), NestCamera::MIN_SCALE);

        let offset_before = camera.offset();
        camera.zoom_around(pos2(100.0, 100.0), pos2(0.0, 0.0), 0.5);
        assert_eq!(camera.scale(), NestCamera::MIN_SCALE);
        assert_eq!(camera.offset(), offset_before);
    }

    #[test]
    fn pan_by_moves_the_offset() {
        let mut camera = NestCamera::new();
        camera.pan_by(vec2(12.0, -7.0));
        assert_eq!(camera.offset(), vec2(12.0, -7.0));
        camera.pan_by(vec2(-2.0, 3.0));
        assert_eq!(camera.offset(), vec2(10.0, -4.0));
    }

    #[test]
    fn world_to_screen_applies_scale_then_offset() {
        let camera = NestCamera {
            scale: 2.0,
            offset: vec2(100.0, 50.0),
        };
        assert_eq!(
            camera.world_to_screen(pos2(0.0, 0.0), pos2(10.0, 20.0)),
            pos2(120.0, 90.0)
        );
        assert_eq!(
            camera.world_to_screen(pos2(40.0, 30.0), pos2(10.0, 20.0)),
            pos2(160.0, 120.0)
        );
    }

    #[test]
    fn world_viewport_scales_the_viewport_into_world_space() {
        let camera = NestCamera {
            scale: 2.0,
            offset: Vec2::ZERO,
        };
        assert_eq!(
            camera.world_viewport(pos2(0.0, 0.0), vec2(640.0, 360.0)),
            Rect::from_min_size(pos2(0.0, 0.0), vec2(320.0, 180.0))
        );
    }

    #[test]
    fn world_viewport_shifts_with_the_offset() {
        let camera = NestCamera {
            scale: 2.0,
            offset: vec2(100.0, 50.0),
        };
        assert_eq!(
            camera.world_viewport(pos2(0.0, 0.0), vec2(640.0, 360.0)),
            Rect::from_min_size(pos2(-50.0, -25.0), vec2(320.0, 180.0))
        );
    }
}
