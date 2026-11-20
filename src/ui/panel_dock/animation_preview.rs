//! Animation preview nest component: paints one frame REGION of the shared
//! sprite-sheet texture inside a dynamic nest.
//!
//! The App publishes the composited sheet texture, its full size and the active
//! frame's pixel region through [`FramePreviewFeed`] every frame. The component
//! maps the region to world space (one sheet pixel = one world unit at the
//! origin) and through the nest camera, so zoom/pan behave exactly like the
//! canvas preview. The panel owns no project data.

use std::cell::Cell;
use std::rc::Rc;

use egui::{pos2, vec2, Rect, TextureId, Vec2};

use crate::core::math::Rect2i;
use crate::ui::theme::SKIN_TINT;

use super::nest::{Component, NestView};
use super::skin::PanelChrome;

/// The sheet region the preview shows, published by the App each frame:
/// `(texture, full sheet size, active frame pixel region)`.
#[derive(Clone, Default)]
pub struct FramePreviewFeed(Rc<Cell<Option<(TextureId, Vec2, Rect2i)>>>);

impl FramePreviewFeed {
    /// Publishes the sheet texture, its full size and the active frame region.
    pub fn set(&self, image: Option<(TextureId, Vec2, Rect2i)>) {
        self.0.set(image);
    }

    /// The published image, if any.
    pub fn get(&self) -> Option<(TextureId, Vec2, Rect2i)> {
        self.0.get()
    }
}

/// Paints the active frame region of the shared sheet at the world origin.
pub struct FramePreviewImage {
    pub feed: FramePreviewFeed,
}

impl FramePreviewImage {
    pub fn new(feed: FramePreviewFeed) -> Self {
        Self { feed }
    }
}

impl Component for FramePreviewImage {
    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, _chrome: &PanelChrome) {
        let Some((texture, sheet_size, region)) = self.feed.get() else {
            let center = view.clip().center();
            ui.painter().text(
                center,
                egui::Align2::CENTER_CENTER,
                "No preview",
                egui::FontId::proportional(12.0),
                ui.visuals().weak_text_color(),
            );
            return;
        };
        // Degenerate regions (or sheets) would divide by zero / paint nothing.
        if region.is_empty() || sheet_size.x <= 0.0 || sheet_size.y <= 0.0 {
            return;
        }
        // World space is the frame's pixel size: one sheet pixel is one world
        // unit, so the camera zooms the frame without resampling it.
        let world = Rect::from_min_size(egui::Pos2::ZERO, vec2(region.w as f32, region.h as f32));
        if !view.visible_world_rect().intersects(world) {
            return;
        }
        // UV is the region as a fraction of the full sheet. egui UVs are
        // top-left origin, matching Rect2i's y-down pixel space, so this is a
        // plain division with no Y flip.
        let uv = Rect::from_min_max(
            pos2(
                region.x as f32 / sheet_size.x,
                region.y as f32 / sheet_size.y,
            ),
            pos2(
                region.right() as f32 / sheet_size.x,
                region.bottom() as f32 / sheet_size.y,
            ),
        );
        // `Shape::image` paints the region through the (clipped) nest painter.
        // NEAREST filtering is an upload-time property of the sheet texture:
        // the App uploads it with `TextureOptions::NEAREST` so pixels stay
        // crisp at every zoom. `SKIN_TINT` is the theme's neutral tint.
        view.painter(ui).image(
            texture,
            view.world_rect_to_screen(world),
            uv,
            SKIN_TINT,
        );
    }
}
