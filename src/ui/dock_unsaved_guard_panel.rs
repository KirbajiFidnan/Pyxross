//! Floating "Unsaved Changes" dock panel (id 211).
//!
//! Registered as a `PanelPlacement::Floating`, `can_pop_out == false`,
//! `resizable == false` dock panel holding a [`NestMode::Static`] nest,
//! matching the app's modal panels (`dock_create_document_panel`,
//! `dock_save_as_panel`, `dock_import_image_panel`).
//!
//! Prompts the user to **Save**, **Discard**, or **Cancel** before a pending
//! Close / Open / New action discards unsaved edits. Emits
//! [`UnsavedGuardEvent`]s for the App shell to execute.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Unsaved Changes floating panel.
pub const UNSAVED_GUARD_PANEL_ID: u64 = 211;

/// Panel/window title.
const UNSAVED_GUARD_TITLE: &str = "Unsaved Changes";

/// Fixed minimum size of the (non-resizable) floating window.
const UNSAVED_GUARD_MIN_SIZE: egui::Vec2 = vec2(320.0, 120.0);

/// Draft view state for the Unsaved Changes panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnsavedGuardView {
    pub document_name: String,
}

/// Actions emitted by the Unsaved Changes panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsavedGuardEvent {
    Save,
    Discard,
    Cancel,
}

/// Shared cells for the Unsaved Changes panel.
#[derive(Clone, Default)]
pub struct UnsavedGuardHost {
    pub view: Rc<RefCell<UnsavedGuardView>>,
    pub events: Rc<RefCell<Vec<UnsavedGuardEvent>>>,
}

impl UnsavedGuardHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_document_name(&self, name: impl Into<String>) {
        self.view.borrow_mut().document_name = name.into();
    }

    pub fn take_events(&self) -> Vec<UnsavedGuardEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

/// Static form component inside the Unsaved Changes panel.
struct UnsavedGuardComponent {
    host: UnsavedGuardHost,
}

impl Component for UnsavedGuardComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let doc_name = self.host.view.borrow().document_name.clone();
        let display_name = if doc_name.is_empty() {
            "Untitled"
        } else {
            doc_name.as_str()
        };

        ui.label(format!(
            "\"{display_name}\" has unsaved changes. Save before continuing?"
        ));
        ui.add_space(12.0);

        let mut emitted = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if chrome.button(ui, "Save", ButtonStyle::plain()).clicked() {
                emitted = Some(UnsavedGuardEvent::Save);
            }
            if chrome.button(ui, "Discard", ButtonStyle::plain()).clicked() {
                emitted = Some(UnsavedGuardEvent::Discard);
            }
            if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                emitted = Some(UnsavedGuardEvent::Cancel);
            }
        });

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(UnsavedGuardEvent::Cancel);
        } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            emitted = Some(UnsavedGuardEvent::Save);
        }

        if let Some(ev) = emitted {
            self.host.events.borrow_mut().push(ev);
        }
    }
}

struct UnsavedGuardPanelContent {
    nest: NestContent,
}

impl PanelContent for UnsavedGuardPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the Unsaved Changes prompt.
pub(crate) fn build_unsaved_guard_nest(host: &UnsavedGuardHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(UnsavedGuardComponent { host: host.clone() });
    nest
}

/// Dock spec for the Unsaved Changes floating panel (dock id 211).
pub fn unsaved_guard_panel_spec(host: &UnsavedGuardHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(UNSAVED_GUARD_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(UNSAVED_GUARD_TITLE, false, UNSAVED_GUARD_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(UnsavedGuardPanelContent {
            nest: build_unsaved_guard_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = UnsavedGuardHost::new();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), UNSAVED_GUARD_MIN_SIZE);
        let spec = unsaved_guard_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), UNSAVED_GUARD_PANEL_ID);
        assert_eq!(spec.metadata.title, "Unsaved Changes");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        assert_eq!(build_unsaved_guard_nest(&host).mode(), NestMode::Static);
    }

    #[test]
    fn enter_emits_save_and_escape_emits_cancel() {
        let host = UnsavedGuardHost::new();
        host.set_document_name("Sprite");
        let mut nest = build_unsaved_guard_nest(&host);
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(UNSAVED_GUARD_PANEL_ID)),
        );

        let raw_enter = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                UNSAVED_GUARD_MIN_SIZE,
            )),
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
        assert_eq!(host.take_events(), vec![UnsavedGuardEvent::Save]);

        let raw_esc = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                UNSAVED_GUARD_MIN_SIZE,
            )),
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
        assert_eq!(host.take_events(), vec![UnsavedGuardEvent::Cancel]);
    }
}
