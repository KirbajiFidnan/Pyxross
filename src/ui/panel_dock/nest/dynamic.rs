//! Dynamic nest painting: content mapped through the camera and clipped to the
//! viewport, with canvas-identical input (plain-scroll zoom, middle-drag pan).
//! Content paints in the host panel's layer below the overlay components.

use egui::{Rect, Ui, Vec2};

use crate::ui::canvas::scroll_zoom_factor;
use crate::ui::input_capture::{ctrl_wheel, InputCapture, SurfaceId};

use super::super::skin::PanelChrome;
use super::{nest_is_usable, nest_viewport, ComponentPlacement, NestContent, NestReset, NestView};

/// Paints a dynamic nest: content components mapped by the camera, then
/// overlay components fixed in screen space, both clipped to the viewport.
pub fn show_dynamic(nest: &mut NestContent, ui: &mut Ui, chrome: &PanelChrome) {
    let full = ui.available_rect_before_wrap();
    ui.set_min_size(full.size());
    let viewport = nest_viewport(full);
    if !nest_is_usable(viewport) {
        // The panel is collapsed to a frame strip: hide every component and
        // skip input handling. The camera and pending resets are left
        // untouched so the nest re-renders exactly as before once the panel
        // grows again.
        return;
    }
    let origin = viewport.min;

    handle_input(nest, ui, viewport, chrome.surface, chrome.capture);
    match nest.take_reset() {
        Some(NestReset::Pan) => nest.camera.reset_pan(),
        Some(NestReset::Zoom) => nest.camera.reset_zoom(),
        None => {}
    }

    // Content and overlays paint in the host panel's own layer: content first
    // (mapped by the camera), then overlays on top. Painting in-layer keeps the
    // nest inside its panel's z band and guarantees overlays stay above the
    // zoomed/panned content.
    let content_view = NestView::new(origin, nest.camera.scale(), nest.camera.offset(), viewport);
    ui.set_clip_rect(viewport);
    for component in &mut nest.components {
        match component.placement() {
            ComponentPlacement::Content => component.ui(ui, &content_view, chrome),
            ComponentPlacement::Overlay => {}
        }
    }

    // Overlay components paint in screen space, still clipped to the viewport.
    let overlay_view = NestView::screen(viewport);
    for component in &mut nest.components {
        match component.placement() {
            ComponentPlacement::Content => {}
            ComponentPlacement::Overlay => component.ui(ui, &overlay_view, chrome),
        }
    }
}

/// Routes the nest's zoom/pan/scroll input through the shared capture.
///
/// The gesture belongs to the topmost surface under the pointer once it
/// starts; an owned pan keeps tracking until release, and scroll deltas are
/// consumed so parent surfaces do not also react.
fn handle_input(
    nest: &mut NestContent,
    ui: &mut Ui,
    viewport: Rect,
    surface: SurfaceId,
    capture: &InputCapture,
) {
    let pointer = ui.input(|input| input.pointer.latest_pos());

    let middle_pressed =
        ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Middle));
    let middle_released =
        ui.input(|input| input.pointer.button_released(egui::PointerButton::Middle));
    if middle_released && capture.is_owner(surface) {
        capture.release();
    }
    if ui.input(|input| input.pointer.middle_down())
        && capture.handles_buttons(surface, middle_pressed)
    {
        let delta = ui.input(|input| input.pointer.delta());
        nest.camera.pan_by(delta);
    }

    if !capture.handles_wheel(surface) {
        return;
    }

    // Plain scroll zooms around the pointer (matching the canvas); Ctrl+scroll
    // is reserved for the pen brush size on the canvas and does nothing here.
    // egui turns Ctrl+wheel into `zoom_delta`, so that path is skipped too.
    let zoom_delta = ui.input(|input| input.zoom_delta());
    let scroll_y = ui.input(|input| input.smooth_scroll_delta.y);
    if !ctrl_wheel(ui) {
        let factor = if zoom_delta != 1.0 {
            zoom_delta
        } else if scroll_y != 0.0 {
            scroll_zoom_factor(scroll_y)
        } else {
            1.0
        };
        if factor != 1.0 {
            if let Some(p) = pointer {
                nest.camera.zoom_around(p, viewport.min, factor);
            }
        }
    }

    // Consume scroll so the parent ScrollArea does not react.
    ui.input_mut(|input| {
        input.smooth_scroll_delta = Vec2::ZERO;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_zoom_factor_matches_the_canvas_rate() {
        assert_eq!(scroll_zoom_factor(0.0), 1.0);
        assert!(scroll_zoom_factor(40.0) > 1.1);
        assert!(scroll_zoom_factor(-40.0) < 1.0);
    }
}
