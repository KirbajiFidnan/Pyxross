//! Floating "Recover Unsaved Work?" dock panel (id 212).
//!
//! Registered as a `PanelPlacement::Floating`, `can_pop_out == false`,
//! `resizable == false` dock panel holding a [`NestMode::Static`] nest,
//! matching the app's modal panels (`dock_create_document_panel`,
//! `dock_save_as_panel`, `dock_import_image_panel`, `dock_unsaved_guard_panel`).
//!
//! Prompts the user to **Restore** or **Discard** a crash-recovery journal
//! found on startup. Emits [`RecoveryEvent`]s for the App shell to execute.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Recover Unsaved Work floating panel.
pub const RECOVERY_PANEL_ID: u64 = 212;

/// Panel/window title.
const RECOVERY_TITLE: &str = "Recover Unsaved Work?";

/// Fixed minimum size of the (non-resizable) floating window.
const RECOVERY_MIN_SIZE: egui::Vec2 = vec2(340.0, 120.0);

/// Draft view state for the Recovery panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryView {
    pub project_name: String,
}

/// Actions emitted by the Recovery panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryEvent {
    Restore,
    Discard,
}

/// Shared cells for the Recovery panel.
#[derive(Clone, Default)]
pub struct RecoveryHost {
    pub view: Rc<RefCell<RecoveryView>>,
    pub events: Rc<RefCell<Vec<RecoveryEvent>>>,
}

impl RecoveryHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_project_name(&self, name: impl Into<String>) {
        self.view.borrow_mut().project_name = name.into();
    }

    pub fn take_events(&self) -> Vec<RecoveryEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

/// Static form component inside the Recovery panel.
struct RecoveryComponent {
    host: RecoveryHost,
}

impl Component for RecoveryComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let project_name = self.host.view.borrow().project_name.clone();
        let display_name = if project_name.is_empty() {
            "Untitled"
        } else {
            project_name.as_str()
        };

        ui.label(format!("Pyxross found unsaved work from \"{display_name}\"."));
        ui.add_space(12.0);

        let mut emitted = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if chrome.button(ui, "Restore", ButtonStyle::plain()).clicked() {
                emitted = Some(RecoveryEvent::Restore);
            }
            if chrome.button(ui, "Discard", ButtonStyle::plain()).clicked() {
                emitted = Some(RecoveryEvent::Discard);
            }
        });

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(RecoveryEvent::Discard);
        } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            emitted = Some(RecoveryEvent::Restore);
        }

        if let Some(ev) = emitted {
            self.host.events.borrow_mut().push(ev);
        }
    }
}

struct RecoveryPanelContent {
    nest: NestContent,
}

impl PanelContent for RecoveryPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the Recovery prompt.
pub(crate) fn build_recovery_nest(host: &RecoveryHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(RecoveryComponent { host: host.clone() });
    nest
}

/// Dock spec for the Recover Unsaved Work floating panel (dock id 212).
pub fn recovery_panel_spec(host: &RecoveryHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(RECOVERY_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(RECOVERY_TITLE, false, RECOVERY_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(RecoveryPanelContent {
            nest: build_recovery_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = RecoveryHost::new();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), RECOVERY_MIN_SIZE);
        let spec = recovery_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), RECOVERY_PANEL_ID);
        assert_eq!(spec.metadata.title, "Recover Unsaved Work?");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        assert_eq!(build_recovery_nest(&host).mode(), NestMode::Static);
    }

    #[test]
    fn enter_emits_restore_and_escape_emits_discard() {
        let host = RecoveryHost::new();
        host.set_project_name("Sprite");
        let mut nest = build_recovery_nest(&host);
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(RECOVERY_PANEL_ID)),
        );

        let raw_enter = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, RECOVERY_MIN_SIZE)),
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        ctx.run_ui(raw_enter, |ui| nest.ui(ui, &chrome))
            .textures_delta
            .clear();
        assert_eq!(host.take_events(), vec![RecoveryEvent::Restore]);

        let raw_esc = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, RECOVERY_MIN_SIZE)),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        ctx.run_ui(raw_esc, |ui| nest.ui(ui, &chrome))
            .textures_delta
            .clear();
        assert_eq!(host.take_events(), vec![RecoveryEvent::Discard]);
    }
}
