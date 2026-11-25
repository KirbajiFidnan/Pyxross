//! Floating per-layer settings dock panel: name, opacity, blend mode, ungroup.
//!
//! Registered as a `PanelPlacement::Floating` dock panel (id 102) with
//! `can_pop_out == false`: it floats, cannot be docked, and closes from its
//! header's Close action (the nest carries no close button of its own).
//! The in-progress rename text lives in the host's `rename_buffer` so it
//! survives across frames; the model is only ever touched through
//! [`LayerPanelEvent`]s pushed to the host.

use crate::core::model::BlendMode;
use crate::ui::dock_hosts::LayerPanelHost;
use crate::ui::dock_tool_property_panel::pro_slider;
use crate::ui::layers::LayerPanelEvent;
use crate::ui::panel_dock::{ButtonStyle, PanelChrome, PanelContent};

/// Display label for a blend mode.
fn blend_label(mode: BlendMode) -> &'static str {
    mode.label()
}

/// All blend modes in display order.
const BLEND_MODES: &[BlendMode] = &BlendMode::ALL;

/// Panel content for the per-layer settings floating dock panel.
///
/// Renders the open layer's settings; no-op (empty body) when no layer is
/// open or the open id is not in the current view.
pub(crate) struct LayerSettingsContent {
    host: LayerPanelHost,
}

impl LayerSettingsContent {
    pub(crate) fn new(host: LayerPanelHost) -> Self {
        Self { host }
    }
}

impl PanelContent for LayerSettingsContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let Some(id) = *self.host.open_settings.borrow() else {
            return;
        };
        let view = self.host.view.borrow();
        let Some(row) = view.rows.iter().find(|row| row.id == id) else {
            return;
        };

        // The rename text is the host's persistent buffer (seeded when the
        // panel opens), so typed characters survive across frames.
        let mut rename = self.host.rename_buffer.borrow_mut();
        let mut opacity = row.opacity;
        let blend = row.blend;
        let is_group = row.is_group;

        // Name: commit on Enter, or on losing focus with a changed value.
        ui.weak("Name");
        ui.add_space(2.0);
        let name_response = ui.add(
            egui::TextEdit::singleline(&mut *rename)
                .desired_width(ui.available_width())
                .hint_text("Layer name"),
        );
        let enter =
            name_response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if enter || (name_response.lost_focus() && *rename != row.name) {
            let committed = rename.clone();
            self.host
                .events
                .borrow_mut()
                .push(LayerPanelEvent::Rename(id, committed.clone()));
            *rename = committed;
        }

        ui.add_space(6.0);
        ui.weak("Opacity");
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let drag = ui.add_sized(
                egui::vec2(46.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut opacity)
                    .range(0.0..=1.0)
                    .speed(0.01)
                    .fixed_decimals(2),
            );
            let slider_w = ui.available_width().max(48.0);
            let slider = ui
                .allocate_ui(egui::vec2(slider_w, 18.0), |ui| {
                    pro_slider(ui, "layer-opacity", &mut opacity, 0.0..=1.0, false)
                })
                .inner;
            if drag.changed() || slider.changed() {
                self.host
                    .events
                    .borrow_mut()
                    .push(LayerPanelEvent::SetOpacity(id, opacity));
            }
        });

        ui.add_space(6.0);
        ui.weak("Blend Mode");
        ui.add_space(2.0);
        egui::ComboBox::from_id_salt(("layer-blend", id.as_u64()))
            .width(ui.available_width().max(100.0))
            .selected_text(blend_label(blend))
            .show_ui(ui, |ui| {
                for &mode in BLEND_MODES {
                    let selected = blend == mode;
                    if chrome.combo_item(ui, selected, blend_label(mode)).clicked() && !selected {
                        self.host
                            .events
                            .borrow_mut()
                            .push(LayerPanelEvent::SetBlend(id, mode));
                    }
                }
            });

        if is_group {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            if chrome
                .button(ui, "Ungroup", ButtonStyle::plain())
                .on_hover_text("Ungroup this group")
                .clicked()
            {
                self.host
                    .events
                    .borrow_mut()
                    .push(LayerPanelEvent::Ungroup(id));
            }
        }
    }
}
