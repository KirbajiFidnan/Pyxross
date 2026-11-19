//! Floating "Editor Options" dock panel (id 219).
//!
//! Hosts user-configurable editor behavior toggles such as whether `W/A/S/D`
//! nudge an active selection transform (alongside the arrow keys).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui::vec2;
use serde::{Deserialize, Serialize};

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Editor Options floating panel.
pub const EDITOR_OPTIONS_PANEL_ID: u64 = 219;

const EDITOR_OPTIONS_TITLE: &str = "Editor Options";
const EDITOR_OPTIONS_MIN_SIZE: egui::Vec2 = vec2(340.0, 140.0);

/// Persisted editor behavior options.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorOptions {
    /// When true, W/A/S/D nudge an active selection transform (like arrow keys).
    pub wasd_transform_move: bool,
}

impl Default for EditorOptions {
    fn default() -> Self {
        Self {
            wasd_transform_move: true,
        }
    }
}

pub fn user_editor_options_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("pyxross").join("editor_options.json"))
}

impl EditorOptions {
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
    }

    pub fn load_user_or_default() -> Self {
        user_editor_options_path()
            .as_deref()
            .map(Self::load_from)
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn save_user(&self) {
        if let Some(path) = user_editor_options_path() {
            self.save_to(&path);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorOptionsEvent {
    Changed(EditorOptions),
    Close,
}

#[derive(Clone, Default)]
pub struct EditorOptionsHost {
    pub options: Rc<RefCell<EditorOptions>>,
    pub events: Rc<RefCell<Vec<EditorOptionsEvent>>>,
}

impl EditorOptionsHost {
    pub fn new(initial: EditorOptions) -> Self {
        Self {
            options: Rc::new(RefCell::new(initial)),
            events: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn take_events(&self) -> Vec<EditorOptionsEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

struct EditorOptionsComponent {
    host: EditorOptionsHost,
}

impl Component for EditorOptionsComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let mut opts = self.host.options.borrow().clone();
        let before = opts.clone();

        ui.strong("Transform Controls");
        ui.add_space(4.0);
        ui.checkbox(
            &mut opts.wasd_transform_move,
            "Use W / A / S / D to move active transform",
        );
        ui.weak("When disabled, only Arrow keys nudge an active transform.");

        if opts != before {
            *self.host.options.borrow_mut() = opts.clone();
            self.host
                .events
                .borrow_mut()
                .push(EditorOptionsEvent::Changed(opts));
        }

        ui.add_space(12.0);
        if chrome.button(ui, "Close", ButtonStyle::plain()).clicked() {
            self.host.events.borrow_mut().push(EditorOptionsEvent::Close);
        }
    }
}

struct EditorOptionsPanelContent {
    nest: NestContent,
}

impl PanelContent for EditorOptionsPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

pub fn editor_options_panel_spec(
    host: &EditorOptionsHost,
    floating_rect: egui::Rect,
) -> PanelSpec {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(EditorOptionsComponent { host: host.clone() });
    PanelSpec {
        id: PanelId::new(EDITOR_OPTIONS_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(EDITOR_OPTIONS_TITLE, false, EDITOR_OPTIONS_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(EditorOptionsPanelContent { nest }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_options_enable_wasd_transform_move() {
        assert!(EditorOptions::default().wasd_transform_move);
    }
}
