//! Frame Edit dock panel (id 203): a SECOND, EDITABLE viewport onto the same
//! canvas, focused on the active animation's frame-edit cursor frame.
//!
//! **Dockable and floatable:** `PanelMetadata::new("frame editor", true, ...)`
//! gives it `can_pop_out == true`, so its header offers Float while docked and
//! Dock while floating. It is registered in the BOTTOM dock between the
//! Animations panel (left, id 201) and the Preview panel (right, id 204).
//!
//! Unlike the Preview panel (a pure view over a [`FramePreviewFeed`]), this
//! panel hosts its OWN [`CanvasWidget`] so the artist can draw on the same
//! layers through the SAME interaction pipeline the main canvas uses. The App
//! publishes the frame-edit camera / canvas view / overlay / texture id into
//! [`FrameEditHost`] before the dock renders, and drains the widget's
//! [`CanvasInteractions`] after rendering to feed them into the App's gesture
//! handler. The widget's world camera is independent of the main camera, so
//! panning/zooming here never moves the main canvas.
//!
//! The panel is otherwise a pure view: every button/input gesture is emitted as
//! a [`FrameEditPanelEvent`] for the App shell to apply.
//!
//! [`FramePreviewFeed`]: crate::ui::panel_dock::animation_preview::FramePreviewFeed

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use egui::{pos2, vec2, Align, Layout, Rect, Sense, UiBuilder};

use crate::core::camera::Camera;
use crate::core::math::Rect2i;
use crate::ui::canvas::{CanvasInteractions, CanvasOverlay, CanvasView, CanvasWidget};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};
use crate::ui::theme::SkinState;

/// The dock id for the Frame Edit panel.
pub const FRAME_EDIT_PANEL_ID: u64 = 203;

/// Height of the pinned control bar along the bottom, in points.
const CONTROL_BAR_HEIGHT: f32 = 30.0;

/// Fixed dimensions and spacing keep both button pairs visually consistent.
const CONTROL_BUTTON_WIDTH: f32 = 22.0;
const CONTROL_GAP: f32 = 4.0;

/// Width of the delay numeric input, sized to match a default `DragValue` in
/// the Tool Property panel so the field reads as a normal theme input.
const CONTROL_DELAY_INPUT_WIDTH: f32 = 44.0;

/// Onion-skin toggles (ghost of previous/next frame) and frame stepping
/// arrows (`◀` / `▶`).
const ONION_PREV_LABEL: &str = crate::ui::icons::ONION_PREV;
const ONION_NEXT_LABEL: &str = crate::ui::icons::ONION_NEXT;
const PREV_LABEL: &str = "\u{25c0}";
const NEXT_LABEL: &str = "\u{25b6}";

/// A gesture emitted by the Frame Edit panel's control bar.
///
/// The panel never mutates the model; the App shell applies these against the
/// frame-edit cursor.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameEditPanelEvent {
    /// Set the frame-edit cursor frame's delay in milliseconds.
    SetFrameDelay { ms: u32 },
    /// Toggle the previous-frame onion skin.
    OnionPrev(bool),
    /// Toggle the next-frame onion skin.
    OnionNext(bool),
    /// Move the frame-edit cursor one frame back.
    PrevFrame,
    /// Move the frame-edit cursor one frame forward.
    NextFrame,
}

/// Shared cells for the Frame Edit panel.
///
/// The App writes the model snapshot (cursor index, frame count, delay text,
/// onion flags, camera, view, overlay, texture) before the dock renders and
/// drains `events` / `interactions` after. The panel content holds a clone and
/// is otherwise a pure view.
#[derive(Clone)]
pub struct FrameEditHost {
    /// The frame-edit cursor index (separate from playback's current index).
    pub index: Rc<Cell<usize>>,
    /// The active animation's frame count.
    pub frame_count: Rc<Cell<usize>>,
    /// In-progress delay text for the cursor frame; seeded by the App when the
    /// cursor frame (or its delay) changes so typing survives across frames.
    pub delay_buffer: Rc<RefCell<String>>,
    /// The `(cursor index, delay_ms)` the buffer was last seeded from.
    pub delay_seed: Rc<Cell<(usize, u32)>>,
    /// Whether the previous-frame onion skin is shown.
    pub onion_prev: Rc<Cell<bool>>,
    /// Whether the next-frame onion skin is shown.
    pub onion_next: Rc<Cell<bool>>,
    /// Gestures emitted by the control bar.
    pub events: Rc<RefCell<Vec<FrameEditPanelEvent>>>,
    /// The panel's private editable canvas widget.
    pub widget: Rc<RefCell<CanvasWidget>>,
    /// The panel's private camera (independent of the main canvas camera).
    pub camera: Rc<Cell<Camera>>,
    /// The overlay painted on the frame-edit viewport (onion ghosts).
    pub overlay: Rc<RefCell<CanvasOverlay>>,
    /// The canvas view (sizes/brush/etc.) for this viewport.
    pub view: Rc<RefCell<CanvasView>>,
    /// The shared full-sheet texture id, published each frame.
    pub image_id: Rc<Cell<Option<egui::TextureId>>>,
    /// The sheet sub-rect this viewport displays: the frame-edit cursor frame's
    /// `region().rect()`. `None` means no frame is available.
    pub region: Rc<Cell<Option<Rect2i>>>,
    /// The interactions the widget produced during its last render, drained by
    /// the App through the same gesture pipeline as the main canvas.
    pub interactions: Rc<RefCell<Option<CanvasInteractions>>>,
}

impl Default for FrameEditHost {
    fn default() -> Self {
        Self {
            index: Rc::new(Cell::new(0)),
            frame_count: Rc::new(Cell::new(0)),
            delay_buffer: Rc::new(RefCell::new(String::new())),
            delay_seed: Rc::new(Cell::new((usize::MAX, 0))),
            onion_prev: Rc::new(Cell::new(false)),
            onion_next: Rc::new(Cell::new(false)),
            events: Rc::new(RefCell::new(Vec::new())),
            widget: Rc::new(RefCell::new(CanvasWidget::new())),
            camera: Rc::new(Cell::new(Camera::new())),
            overlay: Rc::new(RefCell::new(CanvasOverlay::None)),
            view: Rc::new(RefCell::new(CanvasView::default())),
            image_id: Rc::new(Cell::new(None)),
            region: Rc::new(Cell::new(None)),
            interactions: Rc::new(RefCell::new(None)),
        }
    }
}

impl FrameEditHost {
    /// The dock id this host's viewport routes through the gesture router.
    pub fn surface(&self) -> crate::ui::input_capture::SurfaceId {
        crate::ui::input_capture::SurfaceId::Panel(PanelId::new(FRAME_EDIT_PANEL_ID))
    }
}

/// Panel content: the editable frame-edit viewport plus its pinned control bar.
struct FrameEditPanelContent {
    host: FrameEditHost,
}

impl PanelContent for FrameEditPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let host = &self.host;
        let full = ui.max_rect();
        let bar_height = CONTROL_BAR_HEIGHT.min(full.height() * 0.5).max(0.0);
        let canvas_rect = Rect::from_min_max(
            full.min,
            pos2(full.max.x, (full.max.y - bar_height).max(full.min.y)),
        );
        let bar_rect = Rect::from_min_max(pos2(full.min.x, canvas_rect.max.y), full.max);

        // The viewport: the App published this frame's camera/view/overlay and
        // the shared sheet texture id. Rendering through the panel's own
        // CanvasWidget gives it the full draw/pan/zoom pipeline.
        let camera = host.camera.get();
        let view = host.view.borrow().clone();
        let overlay = host.overlay.borrow().clone();
        let image_id = host.image_id.get();
        ui.scope_builder(UiBuilder::new().max_rect(canvas_rect), |ui| match image_id {
            Some(image_id) => {
                let mut widget = host.widget.borrow_mut();
                widget.set_overlay(overlay);
                widget.set_surface(host.surface());
                // Fix 1: scope the viewport to the cursor frame's region (the
                // full sheet size still comes from `view.canvas_size` for UVs).
                widget.set_sheet_region(host.region.get());
                let interactions =
                    widget.ui(ui, chrome.colors, chrome.capture, image_id, view, camera);
                host.camera.set(interactions.updated_camera);
                *host.interactions.borrow_mut() = Some(interactions);
            }
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label("No frame");
                });
            }
        });

        draw_controls(ui, bar_rect, chrome, host);
    }
}

/// The pinned control bar: a single row of three equal zones, left→right:
/// `Delay [__] ms` | `[onion prev][onion next]` | `[frame prev][frame next]`.
fn draw_controls(
    ui: &mut egui::Ui,
    bar_rect: Rect,
    chrome: &PanelChrome,
    host: &FrameEditHost,
) {
    // Three groups need at least a little room each; below this the bar would
    // degenerate into inverted rects.
    if bar_rect.width() < 60.0 || bar_rect.height() < 8.0 {
        return;
    }
    ui.painter()
        .rect_filled(bar_rect, 0.0, chrome.colors.panel_header_bg32());
    ui.painter().hline(
        bar_rect.left()..=bar_rect.right(),
        bar_rect.top(),
        egui::Stroke::new(1.0, chrome.colors.panel_border32()),
    );

    let (left, center, right) = control_zone_rects(bar_rect);

    // LEFT: the always-visible delay input box for the cursor frame.
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(left)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            ui.label(egui::RichText::new("Delay").small());
            let mut buffer = host.delay_buffer.borrow_mut();
            let response = ui.add(
                egui::TextEdit::singleline(&mut *buffer)
                    .desired_width(CONTROL_DELAY_INPUT_WIDTH)
                    .hint_text("__"),
            );
            if response.changed() {
                if let Ok(ms) = buffer.trim().parse::<u32>() {
                    host.events
                        .borrow_mut()
                        .push(FrameEditPanelEvent::SetFrameDelay { ms: ms.max(1) });
                }
            }
            ui.label(egui::RichText::new("ms").small());
        },
    );

    // CENTER: onion-skin toggles, in order prev then next, horizontally centered.
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(center)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = CONTROL_GAP;
            let group = 2.0 * CONTROL_BUTTON_WIDTH + CONTROL_GAP;
            ui.add_space(((ui.available_width() - group) * 0.5).max(0.0));
            let prev_on = host.onion_prev.get();
            if control_button(ui, chrome, ONION_PREV_LABEL, ButtonStyle::toggled(prev_on))
                .on_hover_text("Previous frame onion skin")
                .clicked()
            {
                let on = !prev_on;
                host.onion_prev.set(on);
                host.events
                    .borrow_mut()
                    .push(FrameEditPanelEvent::OnionPrev(on));
            }
            let next_on = host.onion_next.get();
            if control_button(ui, chrome, ONION_NEXT_LABEL, ButtonStyle::toggled(next_on))
                .on_hover_text("Next frame onion skin")
                .clicked()
            {
                let on = !next_on;
                host.onion_next.set(on);
                host.events
                    .borrow_mut()
                    .push(FrameEditPanelEvent::OnionNext(on));
            }
        },
    );

    // RIGHT: frame prev then frame next (`<` `>`), centered. Fix 3: always
    // enabled when there is at least one frame (stepping wraps).
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(right)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = CONTROL_GAP;
            let group = 2.0 * CONTROL_BUTTON_WIDTH + CONTROL_GAP;
            ui.add_space(((ui.available_width() - group) * 0.5).max(0.0));
            let enabled = host.frame_count.get() > 0;
            if control_button(
                ui,
                chrome,
                PREV_LABEL,
                ButtonStyle {
                    enabled,
                    ..ButtonStyle::plain()
                },
            )
                .on_hover_text("Previous frame")
                .clicked()
            {
                host.events
                    .borrow_mut()
                    .push(FrameEditPanelEvent::PrevFrame);
            }
            if control_button(
                ui,
                chrome,
                NEXT_LABEL,
                ButtonStyle {
                    enabled,
                    ..ButtonStyle::plain()
                },
            )
                .on_hover_text("Next frame")
                .clicked()
            {
                host.events
                    .borrow_mut()
                    .push(FrameEditPanelEvent::NextFrame);
            }
        },
    );
}

/// Draws a fixed-size control button while preserving the panel's button skin.
fn control_button(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    label: &str,
    style: ButtonStyle,
) -> egui::Response {
    let size = vec2(CONTROL_BUTTON_WIDTH, ui.spacing().interact_size.y);
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

/// The three equal control-bar zones, left→right, with matching inner padding.
///
/// Pure geometry so the split is structurally testable.
pub(crate) fn control_zone_rects(bar_rect: Rect) -> (Rect, Rect, Rect) {
    let inset = vec2(4.0, 2.0);
    let third = bar_rect.width() / 3.0;
    let left = Rect::from_min_max(
        bar_rect.min,
        pos2(bar_rect.min.x + third, bar_rect.max.y),
    )
    .shrink2(inset);
    let center = Rect::from_min_max(
        pos2(bar_rect.min.x + third, bar_rect.min.y),
        pos2(bar_rect.min.x + 2.0 * third, bar_rect.max.y),
    )
    .shrink2(inset);
    let right = Rect::from_min_max(
        pos2(bar_rect.min.x + 2.0 * third, bar_rect.min.y),
        bar_rect.max,
    )
    .shrink2(inset);
    (left, center, right)
}

/// Dock spec for the Frame Edit panel (dock id `PanelId::new(203)`).
///
/// Dockable and floatable (`can_pop_out == true`), Dynamic nest semantics: the
/// panel content hosts an editable viewport with its own camera.
pub fn frame_edit_panel_spec(host: &FrameEditHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(FRAME_EDIT_PANEL_ID),
        metadata: PanelMetadata::new("Frame Editor", true, vec2(220.0, 150.0)),
        placement,
        floating_rect: Rect::from_min_size(pos2(120.0, 300.0), vec2(320.0, 240.0)),
        content: Box::new(FrameEditPanelContent { host: host.clone() }),
    }
}
