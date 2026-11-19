//! Animation preview dock panel (id 204): a dynamic nest showing the active
//! frame of the shared sprite sheet, with a pinned transport bar along the
//! bottom.
//!
//! **Dockable and floatable:** `PanelMetadata::new("playback", true, ...)` gives
//! it `can_pop_out == true`, so its header offers Float while docked and Dock
//! while floating. The content nest is Dynamic (zoom/pan over the frame) and
//! holds one [`FramePreviewImage`] plus a screen-space transport overlay. Every
//! gesture is emitted as an [`AnimationPanelEvent`] for the App shell to apply —
//! the panel is a pure view over the host cells and never mutates the model.

use egui::{pos2, vec2, Align, Layout, Rect, Sense, UiBuilder};

use crate::ui::animation_hosts::{AnimationPanelEvent, AnimationPanelHost};
use crate::ui::panel_dock::animation_preview::{FramePreviewFeed, FramePreviewImage};
use crate::ui::panel_dock::nest::{Component, ComponentPlacement, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};
use crate::ui::theme::SkinState;

/// The dock id for the animation preview panel.
pub const ANIMATION_PREVIEW_PANEL_ID: u64 = 204;

// Transport glyphs. Real Nerdfont wiring (registering the Nerdfont atlas with
// the egui context) is a later step; these codepoints are stable placeholders
// and render through the fallback font until then.
const GLYPH_PLAY: &str = crate::ui::icons::PLAY;
const GLYPH_STOP: &str = crate::ui::icons::PAUSE;
const GLYPH_RESET: &str = "\u{f0e2}";
const PREV_LABEL: &str = crate::ui::icons::PREV;
const NEXT_LABEL: &str = crate::ui::icons::NEXT;
const LOOP_LABEL: &str = crate::ui::icons::LOOP;

/// Height of the pinned transport bar, in points.
const TRANSPORT_HEIGHT: f32 = 30.0;

/// Fixed minimum width for one transport button cell and the gap between
/// buttons. Wide glyph labels (e.g. `Loop`) grow past the minimum, so every
/// transport button keeps a stable footprint without clipping its label.
const CONTROL_BUTTON_WIDTH: f32 = 22.0;
const CONTROL_GAP: f32 = 4.0;

/// The bottom transport bar: play/stop + stop-and-reset on the left, prev/next
/// in the center, and the loop toggle on the right.
///
/// An overlay component, so it stays fixed in screen space while the frame
/// zooms/pans beneath it.
struct AnimationPreviewControls {
    host: AnimationPanelHost,
}

impl Component for AnimationPreviewControls {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, chrome: &PanelChrome) {
        // Overlays receive a screen-space view whose clip IS the nest viewport,
        // so pinning to the bottom of the clip pins to the bottom of the panel.
        let viewport = view.clip();
        let bar = Rect::from_min_max(
            pos2(
                viewport.min.x,
                (viewport.max.y - TRANSPORT_HEIGHT).max(viewport.min.y),
            ),
            viewport.max,
        );
        // A flat theme-colored band keeps the glyphs legible over the frame.
        ui.painter()
            .rect_filled(bar, 0.0, chrome.colors.panel_header_bg32());

        // Three equal column rects: left transport, centered stepping, right
        // loop. Each zone is exactly one third with the same inset, so the
        // split stays equal while the groups inside keep even padding.
        let (left, center, right) = transport_zone_rects(bar);

        // Left: stop-and-reset immediately left of the play/stop toggle.
        let playing = *self.host.playing.borrow();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(left)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_GAP;
                if transport_button(ui, chrome, GLYPH_RESET, ButtonStyle::plain())
                    .on_hover_text("Stop and reset to frame 0")
                    .clicked()
                {
                    self.host
                        .events
                        .borrow_mut()
                        .push(AnimationPanelEvent::StopReset);
                }
                let (label, hover) = if playing {
                    (GLYPH_STOP, "Stop")
                } else {
                    (GLYPH_PLAY, "Play")
                };
                if transport_button(ui, chrome, label, ButtonStyle::toggled(playing))
                    .on_hover_text(hover)
                    .clicked()
                {
                    let event = if playing {
                        AnimationPanelEvent::Stop
                    } else {
                        AnimationPanelEvent::Play
                    };
                    self.host.events.borrow_mut().push(event);
                }
            },
        );

        // Center: prev/next frame stepping, horizontally centered in the column.
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(center)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = CONTROL_GAP;
                let group = 2.0 * CONTROL_BUTTON_WIDTH + CONTROL_GAP;
                ui.add_space(((ui.available_width() - group) * 0.5).max(0.0));
                if transport_button(ui, chrome, PREV_LABEL, ButtonStyle::plain())
                    .on_hover_text("Previous frame")
                    .clicked()
                {
                    self.host
                        .events
                        .borrow_mut()
                        .push(AnimationPanelEvent::PrevFrame);
                }
                if transport_button(ui, chrome, NEXT_LABEL, ButtonStyle::plain())
                    .on_hover_text("Next frame")
                    .clicked()
                {
                    self.host
                        .events
                        .borrow_mut()
                        .push(AnimationPanelEvent::NextFrame);
                }
            },
        );

        // Right: loop toggle, applied to the ACTIVE animation (its index comes
        // from the list snapshot), centered in the column.
        let active = self
            .host
            .list
            .borrow()
            .animations
            .iter()
            .find(|row| row.active)
            .map(|row| row.index);
        let mut loop_on = *self.host.loop_on.borrow();
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(right)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                let loop_width = transport_button_width(ui, LOOP_LABEL);
                ui.add_space(((ui.available_width() - loop_width) * 0.5).max(0.0));
                if transport_button(ui, chrome, LOOP_LABEL, ButtonStyle::toggled(loop_on))
                    .on_hover_text("Loop playback")
                    .clicked()
                {
                    loop_on = !loop_on;
                    // `loop_on` is a host UI cell, not model state; writing it
                    // back keeps the toggle responsive this frame while the
                    // App applies the actual change from the emitted event.
                    *self.host.loop_on.borrow_mut() = loop_on;
                    if let Some(index) = active {
                        self.host
                            .events
                            .borrow_mut()
                            .push(AnimationPanelEvent::SetLoop { index, on: loop_on });
                    }
                }
            },
        );
    }
}

/// The width a transport button reserves for `label`: the label plus padding,
/// floored at [`CONTROL_BUTTON_WIDTH`] so glyph buttons share a stable size.
fn transport_button_width(ui: &egui::Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, ui.visuals().text_color());
    (galley.size().x + 8.0).max(CONTROL_BUTTON_WIDTH)
}

/// Draws a fixed-size transport button while preserving the theme button skin.
/// Mirrors the frame-edit panel's control-bar button so both bars match.
fn transport_button(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    label: &str,
    style: ButtonStyle,
) -> egui::Response {
    let size = vec2(
        transport_button_width(ui, label),
        ui.spacing().interact_size.y,
    );
    if chrome.skin("button").is_none() {
        return ui
            .add_enabled_ui(style.enabled, |ui| {
                ui.add_sized(size, egui::Button::selectable(style.selected, label))
            })
            .inner;
    }

    let (rect, response) = ui
        .add_enabled_ui(style.enabled, |ui| ui.allocate_exact_size(size, Sense::click()))
        .inner;
    let state = if !response.enabled() {
        SkinState::Disabled
    } else if response.is_pointer_button_down_on() {
        SkinState::Pressed
    } else if response.hovered() {
        SkinState::Hover
    } else {
        SkinState::Normal
    };
    chrome.paint_slice(ui.painter(), "button", state, rect);
    if style.selected {
        let wash = chrome.colors.selection_bg_fill32().gamma_multiply(0.5);
        ui.painter().rect_filled(rect, 2.0, wash);
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.painter().with_clip_rect(rect).text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        ui.visuals().text_color(),
    );
    response
}

/// The three equal transport-bar zones, left→right, with matching inner
/// padding so each logical column is exactly one third of the bar.
fn transport_zone_rects(bar: Rect) -> (Rect, Rect, Rect) {
    let inset = vec2(4.0, 2.0);
    let third = bar.width() / 3.0;
    let left = Rect::from_min_max(bar.min, pos2(bar.min.x + third, bar.max.y)).shrink2(inset);
    let center = Rect::from_min_max(
        pos2(bar.min.x + third, bar.min.y),
        pos2(bar.min.x + 2.0 * third, bar.max.y),
    )
    .shrink2(inset);
    let right = Rect::from_min_max(
        pos2(bar.min.x + 2.0 * third, bar.min.y),
        bar.max,
    )
    .shrink2(inset);
    (left, center, right)
}

/// Panel content: the dynamic nest holding the frame image and transport bar.
struct AnimationPreviewPanelContent {
    nest: NestContent,
}

impl PanelContent for AnimationPreviewPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Dynamic nest holding the frame image (content) and the transport overlay.
pub(crate) fn build_animation_preview_nest(
    host: &AnimationPanelHost,
    feed: FramePreviewFeed,
) -> NestContent {
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(FramePreviewImage::new(feed))
        .push(AnimationPreviewControls { host: host.clone() });
    nest
}

/// Dock spec for the animation preview panel (dock id `PanelId::new(204)`).
///
/// Dockable and floatable: `can_pop_out == true`.
pub fn animation_preview_panel_spec(
    host: &AnimationPanelHost,
    feed: FramePreviewFeed,
    placement: PanelPlacement,
) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(ANIMATION_PREVIEW_PANEL_ID),
        metadata: PanelMetadata::new("playback", true, vec2(240.0, 180.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(80.0, 80.0), vec2(320.0, 260.0)),
        content: Box::new(AnimationPreviewPanelContent {
            nest: build_animation_preview_nest(host, feed),
        }),
    }
}
