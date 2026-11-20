//! Floating animation settings dock panel: name, delay multiplier and loop.
//!
//! Registered as a `PanelPlacement::Floating` dock panel (id 202) with
//! `can_pop_out == false`: it floats, cannot be docked, and closes from its
//! header's Close action (the dock renders Close-only for a floating-only
//! panel). A static nest holds the three controls. The in-progress text lives in
//! the host's buffers so it survives across frames; the model is only ever
//! touched through [`AnimationPanelEvent`]s pushed to the host.

use egui::vec2;

use crate::ui::animation_hosts::{AnimationPanelEvent, AnimationPanelHost};
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the animation settings floating panel.
pub const ANIMATION_SETTINGS_PANEL_ID: u64 = 202;

/// The settings controls: name, delay multiplier and loop.
struct AnimationSettingsComponent {
    host: AnimationPanelHost,
}

impl Component for AnimationSettingsComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, _chrome: &PanelChrome) {
        // Snapshot the row being edited; an empty panel is shown when none is
        // open. The clone releases the `settings_row` borrow immediately.
        let Some(row) = self.host.settings_row.borrow().clone() else {
            return;
        };
        let index = row.index;

        // Name: commit on Enter, or on losing focus with a changed value. The
        // buffer is the host's persistent text so typed characters survive
        // across frames.
        ui.weak("Name");
        ui.add_space(2.0);
        let mut rename = self.host.rename_buffer.borrow_mut();
        let name_response = ui.add(
            egui::TextEdit::singleline(&mut *rename)
                .desired_width(ui.available_width())
                .hint_text("Name"),
        );
        let enter =
            name_response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if enter || (name_response.lost_focus() && *rename != row.name) {
            let committed = rename.clone();
            self.host
                .events
                .borrow_mut()
                .push(AnimationPanelEvent::SetName {
                    index,
                    name: committed,
                });
        }
        drop(rename);

        ui.add_space(8.0);

        // Delay multiplier & Loop on one aligned row.
        let mut multiplier = self.host.multiplier_buffer.borrow_mut();
        let mut loop_on = self.host.settings_loop.borrow_mut();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.weak("Delay ×");
            let response =
                ui.add(egui::TextEdit::singleline(&mut *multiplier).desired_width(64.0));
            if response.changed() {
                if let Ok(parsed) = multiplier.trim().parse::<f64>() {
                    self.host
                        .events
                        .borrow_mut()
                        .push(AnimationPanelEvent::SetDelayMultiplier {
                            index,
                            mult: parsed,
                        });
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.checkbox(&mut *loop_on, "Loop").changed() {
                    self.host
                        .events
                        .borrow_mut()
                        .push(AnimationPanelEvent::SetLoop {
                            index,
                            on: *loop_on,
                        });
                }
            });
        });
    }
}

/// Panel content: the static nest holding the settings controls.
struct AnimationSettingsPanelContent {
    nest: NestContent,
}

impl PanelContent for AnimationSettingsPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the settings controls.
pub(crate) fn build_animation_settings_nest(host: &AnimationPanelHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(AnimationSettingsComponent { host: host.clone() });
    nest
}

/// Dock spec for the animation settings floating panel (dock id 202).
///
/// `can_pop_out == false`, so the header shows no Float/Dock button: the panel
/// floats but is not dockable.
pub fn animation_settings_panel_spec(
    host: &AnimationPanelHost,
    floating_rect: egui::Rect,
) -> PanelSpec {
    let floating_rect = egui::Rect::from_min_size(
        floating_rect.min,
        vec2(
            floating_rect.width().max(260.0),
            floating_rect.height().min(132.0).max(124.0),
        ),
    );
    PanelSpec {
        id: PanelId::new(ANIMATION_SETTINGS_PANEL_ID),
        metadata: PanelMetadata::new("Animation settings", false, vec2(260.0, 124.0)),
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(AnimationSettingsPanelContent {
            nest: build_animation_settings_nest(host),
        }),
    }
}
