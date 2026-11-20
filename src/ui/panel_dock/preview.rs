//! Preview panel: the first real panel content.
//!
//! Shows the active project's composited canvas inside a dynamic nest, with two
//! overlay buttons that reset the nest camera's pan and zoom. The panel owns no
//! project data: the app publishes the current canvas texture through
//! [`PreviewFeed`] every frame.

use std::cell::Cell;
use std::rc::Rc;

use egui::{Rect, TextureId, Vec2};

use crate::ui::theme::SKIN_TINT;

use super::nest::{Component, ComponentPlacement, NestReset, NestResetHandle, NestView};
use super::skin::{ButtonStyle, PanelChrome};

/// The canvas image the preview shows, published by the app each frame.
#[derive(Clone, Default)]
pub struct PreviewFeed(Rc<Cell<Option<(TextureId, Vec2)>>>);

impl PreviewFeed {
    /// Publishes the current canvas texture and its world size in points.
    pub fn set(&self, image: Option<(TextureId, Vec2)>) {
        self.0.set(image);
    }

    /// The published canvas image, if any.
    pub fn image(&self) -> Option<(TextureId, Vec2)> {
        self.0.get()
    }
}

/// Paints the canvas image at the world origin.
pub struct PreviewImage {
    feed: PreviewFeed,
}

impl PreviewImage {
    pub fn new(feed: PreviewFeed) -> Self {
        Self { feed }
    }
}

impl Component for PreviewImage {
    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, _chrome: &PanelChrome) {
        let Some((texture, size)) = self.feed.image() else {
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
        let world = Rect::from_min_size(egui::Pos2::ZERO, size);
        if !view.visible_world_rect().intersects(world) {
            return;
        }
        view.painter(ui).image(
            texture,
            view.world_rect_to_screen(world),
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            SKIN_TINT,
        );
    }
}

/// The panel's two camera reset buttons, pinned to the nest's top-left.
pub struct PreviewControls {
    reset: NestResetHandle,
}

impl PreviewControls {
    pub fn new(reset: NestResetHandle) -> Self {
        Self { reset }
    }
}

impl Component for PreviewControls {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, chrome: &PanelChrome) {
        let vp = view.clip();
        let bar = Rect::from_min_max(vp.min + Vec2::splat(4.0), vp.max - Vec2::splat(4.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if chrome
                    .button(ui, "Reset pan", super::skin::ButtonStyle::plain())
                    .clicked()
                {
                    self.reset.request(NestReset::Pan);
                }
                if chrome
                    .button(ui, "Reset zoom", super::skin::ButtonStyle::plain())
                    .clicked()
                {
                    self.reset.request(NestReset::Zoom);
                }
            });
        });
    }
}

/// Reusable zoom-and-pan camera state for right-hand preview panes.
#[derive(Clone, Copy, Debug)]
pub struct PreviewViewport {
    pub zoom: f32,
    pub pan: Vec2,
    pub generation: u64,
}

impl Default for PreviewViewport {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewViewport {
    pub const fn new() -> Self {
        Self {
            zoom: 1.0,
            pan: Vec2::ZERO,
            generation: u64::MAX,
        }
    }

    pub fn reset(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    pub fn sync_generation(&mut self, gen: u64) {
        if self.generation != gen {
            self.generation = gen;
            self.reset();
        }
    }

    /// Handle scroll-to-zoom, drag-to-pan, and double-click-to-fit inside
    /// `viewport_rect`.
    pub fn handle_interaction(
        &mut self,
        ui: &egui::Ui,
        viewport_rect: Rect,
        response: &egui::Response,
    ) {
        if response.double_clicked() {
            self.reset();
        }
        if response.dragged_by(egui::PointerButton::Primary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let (scroll_y, pointer_pos) =
                ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
            if scroll_y.abs() > 0.01 {
                let factor = (1.0 + scroll_y * 0.0025).clamp(0.5, 2.0);
                let old_zoom = self.zoom;
                let new_zoom = (old_zoom * factor).clamp(0.25, 16.0);
                if (new_zoom - old_zoom).abs() > 1e-4 {
                    let center = viewport_rect.center();
                    let anchor = pointer_pos.unwrap_or(center) - center;
                    let ratio = new_zoom / old_zoom;
                    self.pan = anchor - (anchor - self.pan) * ratio;
                    self.zoom = new_zoom;
                }
            }
        }
    }

    /// Transform a fitted base rect around `viewport_center` using `zoom` and `pan`.
    pub fn transform_rect(&self, base_rect: Rect, viewport_center: egui::Pos2) -> Rect {
        let min = viewport_center + (base_rect.min - viewport_center) * self.zoom + self.pan;
        let size = base_rect.size() * self.zoom;
        Rect::from_min_size(min, size)
    }

    /// Draw the compact footer bar with zoom percentage, hints, and a Fit button.
    pub fn draw_footer(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &PanelChrome,
        footer_rect: Rect,
    ) {
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(footer_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                let pct = (self.zoom * 100.0).round() as i32;
                ui.label(
                    egui::RichText::new(format!("{pct}% · Scroll: zoom · Drag: pan")).weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if chrome.button(ui, "Fit", ButtonStyle::plain()).clicked() {
                        self.reset();
                    }
                });
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_roundtrips_the_published_image() {
        let feed = PreviewFeed::default();
        assert_eq!(feed.image(), None);
        feed.set(Some((TextureId::Managed(3), Vec2::new(128.0, 96.0))));
        assert_eq!(
            feed.image(),
            Some((TextureId::Managed(3), Vec2::new(128.0, 96.0)))
        );
        feed.set(None);
        assert_eq!(feed.image(), None);
    }

    #[test]
    fn controls_request_the_matching_reset() {
        let reset = NestResetHandle::default();
        let mut controls = PreviewControls::new(reset.clone());
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Canvas,
        );
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            controls.ui(ui, &NestView::screen(ui.max_rect()), &chrome);
        });
        output.textures_delta.clear();
        assert_eq!(reset.take(), None, "no request without a click");
    }
}
