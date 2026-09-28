//! Static nest: fixed-scale content measured through egui's scroll machinery,
//! with custom skinned scrollbars and a shared pan/scroll/wheel offset.

use egui::{Ui, UiBuilder, Vec2};

use super::super::skin::PanelChrome;
use super::scrollbar;
use super::{nest_is_usable, nest_viewport, ComponentPlacement, NestContent, NestView};

/// Paints a static nest: content at fixed scale, top-left aligned, inside a
/// hidden-bar ScrollArea that measures overflow and clamps the offset. Custom
/// skinned bars are painted on top; middle-drag pan writes the same offset.
/// Overlay components paint in screen space, pinned to the viewport and
/// clipped to it, so they never scroll with the content.
pub fn show_static(nest: &mut NestContent, ui: &mut Ui, chrome: &PanelChrome) {
    let full = ui.available_rect_before_wrap();
    ui.set_min_size(full.size());
    let viewport = nest_viewport(full);
    if !nest_is_usable(viewport) {
        // The panel is collapsed to a frame strip: hide every component and
        // the scrollbars. The shared offset is left untouched so the content
        // re-renders exactly as before once the panel grows again.
        return;
    }
    let id = ui.id().with("nest-static");

    // The native scroll fade is a gradient mesh that would fight the custom
    // bars; disable it for this subtree only (clone-on-write, not global).
    ui.spacing_mut().scroll.fade.strength = 0.0;

    // While another surface owns a button gesture, wheel input is masked away
    // from this nest (the scroll area reacts to the raw input otherwise).
    if chrome.capture.owner().is_some() && !chrome.capture.is_owner(chrome.surface) {
        ui.input_mut(|input| input.smooth_scroll_delta = Vec2::ZERO);
    }

    // Content and the scroll machinery live in a child ui whose max_rect is
    // the inset viewport; the outer ui still reserves the full panel size.
    let mut content_size = Vec2::ZERO;
    let mut offset = nest.offset;
    ui.scope_builder(UiBuilder::new().max_rect(viewport), |inner| {
        let output = egui::ScrollArea::both()
            .id_salt(id)
            .auto_shrink([false, false])
            .min_scrolled_width(0.0)
            .min_scrolled_height(0.0)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .scroll_offset(offset)
            .show(inner, |scroll| {
                for component in &mut nest.components {
                    match component.placement() {
                        ComponentPlacement::Content => {
                            component.ui(scroll, &NestView::screen(viewport), chrome)
                        }
                        ComponentPlacement::Overlay => {}
                    }
                }
            });
        content_size = output.content_size;
        offset = output.state.offset;
    });

    // Middle-drag pan writes the same offset the scrollbars read. The gesture
    // is owned by this surface once started and keeps tracking until release.
    let middle_pressed =
        ui.input(|input| input.pointer.button_pressed(egui::PointerButton::Middle));
    let middle_released =
        ui.input(|input| input.pointer.button_released(egui::PointerButton::Middle));
    if middle_released && chrome.capture.is_owner(chrome.surface) {
        chrome.capture.release();
    }
    if ui.input(|input| input.pointer.button_released(egui::PointerButton::Primary))
        && chrome.capture.is_owner(chrome.surface)
    {
        chrome.capture.release();
    }
    let middle_panning = ui.input(|input| input.pointer.middle_down())
        && chrome
            .capture
            .handles_buttons(chrome.surface, middle_pressed);
    if middle_panning {
        let delta = ui.input(|input| input.pointer.delta());
        let max_offset = (content_size - viewport.size()).max(Vec2::ZERO);
        offset = (offset + delta).clamp(Vec2::ZERO, max_offset);
    }
    nest.offset = offset;

    scrollbar::paint_bars(ui, chrome, viewport, content_size, &mut nest.offset, id);

    // Overlay components paint in screen space, pinned to the viewport and
    // clipped to it; they do not scroll with the content.
    ui.scope_builder(UiBuilder::new().max_rect(viewport), |overlay| {
        overlay.set_clip_rect(viewport);
        for component in &mut nest.components {
            match component.placement() {
                ComponentPlacement::Content => {}
                ComponentPlacement::Overlay => {
                    component.ui(overlay, &NestView::screen(viewport), chrome)
                }
            }
        }
    });
}
