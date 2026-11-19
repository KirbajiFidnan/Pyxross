//! Floating "Key Bindings" dock panel (id 214).
//!
//! Registered as a `PanelPlacement::Floating`, `can_pop_out == false`,
//! `resizable == false` dock panel holding a [`NestMode::Static`] nest,
//! opened from **Options -> key bindings**. Wraps [`KeybindingsPanel`] so the
//! user can rebind any tool or UI shortcut and apply/reset it.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::input::Keymap;
use crate::ui::keybindings::{KeybindingsEvent, KeybindingsPanel};
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Key Bindings floating panel.
pub const KEYBINDINGS_PANEL_ID: u64 = 214;

/// Dock id for the Key Bindings unsaved-changes confirmation popup.
pub const KEYBINDINGS_GUARD_PANEL_ID: u64 = 216;

/// Panel/window title.
const KEYBINDINGS_TITLE: &str = "Key Bindings";

/// Minimum size of the resizable floating Key Bindings window.
const KEYBINDINGS_MIN_SIZE: egui::Vec2 = vec2(380.0, 260.0);

/// Actions emitted by the Key Bindings unsaved-changes confirmation popup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeybindingsGuardEvent {
    ApplyAndClose,
    CloseWithoutSaving,
    Cancel,
}

/// Shared view & event state for the Key Bindings panel.
#[derive(Clone)]
pub struct KeybindingsHost {
    pub panel: Rc<RefCell<KeybindingsPanel>>,
    pub events: Rc<RefCell<Vec<KeybindingsEvent>>>,
    pub guard_events: Rc<RefCell<Vec<KeybindingsGuardEvent>>>,
}

impl Default for KeybindingsHost {
    fn default() -> Self {
        Self::new(&Keymap::defaults())
    }
}

impl KeybindingsHost {
    pub fn new(keymap: &Keymap) -> Self {
        Self {
            panel: Rc::new(RefCell::new(KeybindingsPanel::new(keymap))),
            events: Rc::new(RefCell::new(Vec::new())),
            guard_events: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn sync_keymap(&self, keymap: &Keymap) {
        *self.panel.borrow_mut() = KeybindingsPanel::new(keymap);
    }

    pub fn take_events(&self) -> Vec<KeybindingsEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    pub fn take_guard_events(&self) -> Vec<KeybindingsGuardEvent> {
        self.guard_events.borrow_mut().drain(..).collect()
    }
}

struct KeybindingsComponent {
    host: KeybindingsHost,
}

impl Component for KeybindingsComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let mut emitted = Vec::new();
        let footer_h = 38.0;
        let list_h = (ui.available_height() - footer_h).max(80.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(list_h)
            .show(ui, |ui| {
                self.host.panel.borrow_mut().ui_rows(ui, chrome.colors);
            });
        self.host
            .panel
            .borrow_mut()
            .ui_footer(ui, chrome.colors, &mut emitted);
        if !emitted.is_empty() {
            self.host.events.borrow_mut().extend(emitted);
        }
    }
}

struct KeybindingsPanelContent {
    nest: NestContent,
}

impl PanelContent for KeybindingsPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

pub(crate) fn build_keybindings_nest(host: &KeybindingsHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(KeybindingsComponent { host: host.clone() });
    nest
}

/// Dock spec for the Key Bindings floating panel (dock id 214).
pub fn keybindings_panel_spec(host: &KeybindingsHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(KEYBINDINGS_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(KEYBINDINGS_TITLE, false, KEYBINDINGS_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(KeybindingsPanelContent {
            nest: build_keybindings_nest(host),
        }),
    }
}

struct KeybindingsGuardComponent {
    host: KeybindingsHost,
}

impl Component for KeybindingsGuardComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        ui.label("Are you sure you want to close without saving settings?");
        ui.add_space(12.0);
        let mut emitted = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if chrome
                .button(ui, "Apply & Close", ButtonStyle::plain())
                .clicked()
            {
                emitted = Some(KeybindingsGuardEvent::ApplyAndClose);
            }
            if chrome
                .button(ui, "Close Without Saving", ButtonStyle::plain())
                .clicked()
            {
                emitted = Some(KeybindingsGuardEvent::CloseWithoutSaving);
            }
            if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                emitted = Some(KeybindingsGuardEvent::Cancel);
            }
        });
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(KeybindingsGuardEvent::Cancel);
        }
        if let Some(ev) = emitted {
            self.host.guard_events.borrow_mut().push(ev);
        }
    }
}

/// Dock spec for the Key Bindings unsaved-changes confirmation popup (id 216).
pub fn keybindings_guard_panel_spec(
    host: &KeybindingsHost,
    floating_rect: egui::Rect,
) -> PanelSpec {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(KeybindingsGuardComponent { host: host.clone() });
    PanelSpec {
        id: PanelId::new(KEYBINDINGS_GUARD_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new("Unsaved Key Bindings", false, vec2(360.0, 110.0))
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(KeybindingsPanelContent { nest }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_float_only_resizable_and_uses_a_static_nest() {
        let host = KeybindingsHost::default();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), KEYBINDINGS_MIN_SIZE);
        let spec = keybindings_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), KEYBINDINGS_PANEL_ID);
        assert_eq!(spec.metadata.title, "Key Bindings");
        assert!(!spec.metadata.can_pop_out);
        assert!(spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        assert_eq!(build_keybindings_nest(&host).mode(), NestMode::Static);
    }
}
