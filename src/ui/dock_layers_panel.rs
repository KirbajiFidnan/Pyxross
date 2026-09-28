//! Layers dock panel: hierarchical layer rows with drag-and-drop reordering.
//!
//! A static nest holds two components: the scrollable row list (content) and
//! the pinned action header (overlay). The header never scrolls; the rows
//! scroll under it. Every gesture is emitted as a [`LayerPanelEvent`] for the
//! App shell to apply — the panel never mutates the model.

use egui::vec2;

use crate::core::model::LayerId;
use crate::ui::dock_hosts::{LayerPanelHost, LayerRow};
use crate::ui::layers::LayerPanelEvent;
use crate::ui::panel_dock::nest::{Component, ComponentPlacement, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

use super::dock_layers_settings::LayerSettingsContent;

/// The dock id for the Layers panel.
const LAYERS_PANEL_ID: u64 = 101;

/// Dock id for the per-layer settings floating panel.
pub const LAYER_SETTINGS_PANEL_ID: u64 = 102;

/// Height of one layer row.
const ROW_HEIGHT: f32 = 28.0;

/// Height of the pinned header band (matches the dock header height).
const HEADER_HEIGHT: f32 = 28.0;

/// Horizontal indent per depth level.
const DEPTH_INDENT: f32 = 12.0;

/// Width of the expand-arrow slot. Leaves reserve the same slot so names
/// align by depth; the slot is wide enough for the arrow glyph plus padding.
const ARROW_SLOT: f32 = 32.0;

/// Pinned action header: Add/Delete on the left, Merge/New group on the right.
///
/// An overlay component, so it stays fixed while the row list scrolls.
struct LayersHeaderComponent {
    host: LayerPanelHost,
}

impl Component for LayersHeaderComponent {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow();
        ui.horizontal(|ui| {
            if chrome
                .button(ui, "+ Add", ButtonStyle::plain())
                .on_hover_text("Add a new layer")
                .clicked()
            {
                self.host.events.borrow_mut().push(LayerPanelEvent::Add);
            }
            let delete_style = ButtonStyle {
                enabled: view.delete_enabled,
                ..ButtonStyle::plain()
            };
            let delete = chrome
                .button(ui, "🗑 Delete", delete_style)
                .on_hover_text("Delete the active layer");
            if delete.clicked() {
                if let Some(active) = view.active {
                    self.host
                        .events
                        .borrow_mut()
                        .push(LayerPanelEvent::Remove(active));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let new_group = chrome
                    .button(ui, "New group", ButtonStyle::plain())
                    .on_hover_text("Wrap the active layer in a new group");
                if new_group.clicked() {
                    self.host
                        .events
                        .borrow_mut()
                        .push(LayerPanelEvent::CreateGroup);
                }
                let merge_style = ButtonStyle {
                    enabled: view.merge_enabled,
                    ..ButtonStyle::plain()
                };
                let merge = chrome
                    .button(ui, "Merge", merge_style)
                    .on_hover_text("Merge the active layer down");
                if merge.clicked() {
                    self.host
                        .events
                        .borrow_mut()
                        .push(LayerPanelEvent::MergeDown);
                }
            });
        });
    }
}

/// Scrollable layer list: one indented row per layer, in view order.
///
/// Each row is a drop zone for reordering; the name label is the drag source
/// (click selects, drag reorders). The active row is tinted with the egui
/// selection background.
struct LayersListComponent {
    host: LayerPanelHost,
}

impl Component for LayersListComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow();
        let active = view.active;
        // The list follows the nest width: a horizontal clip viewport keeps the
        // rows' fixed-size controls from widening the nest's scroll content
        // when the panel is narrow. It is wheel/drag-neutral and bar-free, so
        // it never steals input from the rows or the nest's vertical scroll.
        egui::ScrollArea::horizontal()
            .id_salt("layers-list")
            .auto_shrink([false, true])
            .min_scrolled_width(0.0)
            .scroll_source(egui::scroll_area::ScrollSource::NONE)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                // Tall enough to overflow a short dock viewport so the rows
                // scroll under the pinned header; the +80 reserves the header
                // band. The list spans the full nest width, shrinking with the
                // panel.
                ui.set_min_size(vec2(
                    ui.available_width(),
                    view.rows.len() as f32 * ROW_HEIGHT + 80.0,
                ));
                ui.add_space(HEADER_HEIGHT);
                for row in &view.rows {
                    row_ui(&self.host, ui, row, active, chrome);
                }
            });
    }
}

/// One layer row: indent, expand arrow, eye toggle, name (drag source) and a
/// settings button. The whole row is the drop target for reordering.
fn row_ui(
    host: &LayerPanelHost,
    ui: &mut egui::Ui,
    row: &LayerRow,
    active: Option<LayerId>,
    chrome: &PanelChrome,
) {
    let id = row.id;
    let is_active = active == Some(id);
    let row_bg = if is_active {
        ui.visuals().selection.bg_fill
    } else {
        egui::Color32::TRANSPARENT
    };
    let (_, dropped) = ui.dnd_drop_zone::<LayerId, _>(egui::Frame::NONE, |ui| {
        egui::Frame::NONE
            .fill(row_bg)
            .inner_margin(egui::Margin::symmetric(4, 2))
            .show(ui, |ui| {
                // The row background must cover the full nest width.
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add_space(row.depth as f32 * DEPTH_INDENT);
                    if row.is_group && row.has_children {
                        let arrow_glyph = if row.expanded { "▼" } else { "▶" };
                        let arrow = chrome
                            .button(ui, arrow_glyph, ButtonStyle::plain())
                            .on_hover_text("Expand or collapse the group");
                        // The arrow occupies a fixed slot so names align by
                        // depth; pad out whatever the button did not use.
                        ui.add_space((ARROW_SLOT - arrow.rect.width()).max(0.0));
                        if arrow.clicked() {
                            host.events
                                .borrow_mut()
                                .push(LayerPanelEvent::ToggleExpanded(id));
                        }
                    } else {
                        ui.add_space(ARROW_SLOT);
                    }
                    if chrome
                        .button(ui, "👁", ButtonStyle::toggled(row.visible))
                        .on_hover_text("Toggle visibility")
                        .clicked()
                    {
                        host.events
                            .borrow_mut()
                            .push(LayerPanelEvent::ToggleVisible(id));
                    }
                    // The name senses both click and drag: a plain click
                    // selects the layer, a drag reorders it.
                    let name = ui.add(
                        egui::Button::new(&row.name)
                            .sense(egui::Sense::click_and_drag())
                            .selected(is_active),
                    );
                    if name.clicked() {
                        host.events.borrow_mut().push(LayerPanelEvent::Select(id));
                    }
                    if name.drag_started() {
                        name.dnd_set_drag_payload(id);
                    }
                    // The settings button is pinned to the far right of the
                    // row while the eye/name stay on the left.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if chrome
                            .button(ui, "⚙", ButtonStyle::plain())
                            .on_hover_text("Layer settings")
                            .clicked()
                        {
                            host.events
                                .borrow_mut()
                                .push(LayerPanelEvent::SettingsToggle(id));
                        }
                    });
                });
            });
    });
    if let Some(dragged) = dropped {
        host.events
            .borrow_mut()
            .push(LayerPanelEvent::ReorderDropped {
                dragged: *dragged,
                target: id,
            });
    }
}

/// Panel content: the nest holding the layer rows and pinned header.
struct LayersPanelContent {
    nest: NestContent,
}

impl PanelContent for LayersPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the layer rows (content) and the pinned header (overlay).
pub(crate) fn build_layers_nest(host: &LayerPanelHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(LayersListComponent { host: host.clone() })
        .push(LayersHeaderComponent { host: host.clone() });
    nest
}

/// Dock spec for the Layers panel (dock id `PanelId::new(101)`).
pub fn layers_panel_spec(host: &LayerPanelHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(LAYERS_PANEL_ID),
        metadata: PanelMetadata::new("Layers", false, vec2(240.0, 120.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(40.0, 80.0), vec2(260.0, 360.0)),
        content: Box::new(LayersPanelContent {
            nest: build_layers_nest(host),
        }),
    }
}

/// Dock spec for the per-layer settings floating panel (dock id 102).
///
/// `can_pop_out == false` (and `can_native_pop_out == false`), so the header
/// shows no Float/Dock button: the panel floats but is not dockable.
pub fn layer_settings_panel_spec(host: &LayerPanelHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(LAYER_SETTINGS_PANEL_ID),
        metadata: PanelMetadata::new("Layer settings", false, vec2(260.0, 170.0)),
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(LayerSettingsContent::new(host.clone())),
    }
}
