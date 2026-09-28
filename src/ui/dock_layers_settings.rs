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
use crate::ui::layers::LayerPanelEvent;
use crate::ui::panel_dock::{ButtonStyle, PanelChrome, PanelContent};

/// Display label for a blend mode (mirrors `layers::blend_label`).
fn blend_label(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiply",
        BlendMode::Screen => "Screen",
        BlendMode::Add => "Add",
    }
}

/// All blend modes in display order.
const BLEND_MODES: [BlendMode; 4] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Add,
];

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
        let mut blend = row.blend;
        let is_group = row.is_group;

        // Name: commit on Enter, or on losing focus with a changed value.
        let name_response = ui.add(egui::TextEdit::singleline(&mut *rename));
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

        // Opacity: numeric drag value and slider bound to one local; the
        // App coalesces the change stream into one undo step per gesture.
        ui.horizontal(|ui| {
            ui.label("Opacity");
            let drag = ui.add(
                egui::DragValue::new(&mut opacity)
                    .range(0.0..=1.0)
                    .speed(0.01),
            );
            let slider = ui.add(egui::Slider::new(&mut opacity, 0.0..=1.0).fixed_decimals(2));
            if drag.changed() || slider.changed() {
                self.host
                    .events
                    .borrow_mut()
                    .push(LayerPanelEvent::SetOpacity(id, opacity));
            }
        });

        // Blend mode combo.
        ui.horizontal(|ui| {
            ui.label("Blend");
            egui::ComboBox::from_id_salt(("layer-blend", id.as_u64()))
                .selected_text(blend_label(blend))
                .show_ui(ui, |ui| {
                    for mode in BLEND_MODES {
                        if ui
                            .selectable_value(&mut blend, mode, blend_label(mode))
                            .changed()
                        {
                            self.host
                                .events
                                .borrow_mut()
                                .push(LayerPanelEvent::SetBlend(id, blend));
                        }
                    }
                });
        });

        if is_group
            && chrome
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
