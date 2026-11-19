//! Toolbox dock panel: a static nest listing the application drawing tools.

use egui::vec2;

use crate::input::Tool;
use crate::ui::dock_hosts::ToolboxHost;
use crate::ui::icons;
use crate::ui::panel_dock::nest::NestView;
use crate::ui::panel_dock::{
    ButtonStyle, Component, NestContent, NestMode, PanelChrome, PanelId, PanelMetadata,
    PanelPlacement, PanelSpec,
};
use crate::ui::toolbar::{tool_label, ToolbarEvent, TOOLS};

/// The dock id for the Toolbox panel.
pub const TOOLBOX_PANEL_ID: u64 = 100;

/// Content height tall enough to overflow a short dock viewport, so the tool
/// buttons scroll with the nest content instead of being pinned. The width
/// follows the panel so a narrow dock never overflows horizontally.
const TOOLBOX_CONTENT_HEIGHT: f32 = 360.0;

fn tool_icon(tool: Tool, shortcuts: &crate::ui::dock_hosts::ToolboxShortcuts) -> String {
    match tool {
        Tool::Pencil => icons::PENCIL.to_owned(),
        Tool::Eraser => icons::ERASER.to_owned(),
        Tool::Fieldier => icons::SELECTION.to_owned(),
        Tool::Fill => icons::FILL.to_owned(),
        Tool::Eyedropper => icons::EYEDROPPER.to_owned(),
        Tool::Tile => icons::TILE.to_owned(),
        Tool::AnimationPlacer => icons::ANIMATION.to_owned(),
        Tool::Text => {
            if shortcuts.text.is_empty() || shortcuts.text == "Unbound" {
                "T".to_owned()
            } else {
                shortcuts.text.clone()
            }
        }
        Tool::Draw => icons::PENCIL.to_owned(),
    }
}

fn tool_tooltip(tool: Tool, shortcuts: &crate::ui::dock_hosts::ToolboxShortcuts) -> String {
    let name = tool_label(tool);
    match tool {
        Tool::Pencil => format!("{name} ({})", shortcuts.pencil),
        Tool::Eraser => format!("{name} ({})", shortcuts.eraser),
        Tool::Fieldier => format!(
            "{name} ({} / {} / {})",
            shortcuts.rectangle, shortcuts.wand, shortcuts.lasso
        ),
        Tool::Fill => format!("{name} ({})", shortcuts.fill),
        Tool::Eyedropper => format!("{name} (Right-Click)"),
        Tool::Tile => format!("{name} ({})", shortcuts.tile),
        Tool::AnimationPlacer => format!("{name} ({})", shortcuts.animation_placer),
        Tool::Text => format!("{name} ({})", shortcuts.text),
        Tool::Draw => name.to_string(),
    }
}

/// One selectable tool row: highlights the active tool and emits a
/// [`ToolbarEvent::ToolSelected`] on click.
struct ToolboxComponent {
    host: ToolboxHost,
}

impl Component for ToolboxComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        ui.set_min_size(vec2(ui.available_width(), TOOLBOX_CONTENT_HEIGHT));
        let view = self.host.view.borrow().clone();
        let active_tool = view.tool;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let button_w = ui.available_width().clamp(34.0, 48.0);
            for tool in TOOLS {
                let active = active_tool == tool;
                let response = ui
                    .allocate_ui(vec2(button_w, 34.0), |ui| {
                        let label = tool_icon(tool, &view.shortcuts);
                        let font_size = if tool == Tool::Text && label.len() > 2 {
                            12.0
                        } else if tool == Tool::Text {
                            17.0
                        } else {
                            20.0
                        };
                        chrome
                            .button_with_font_size(
                                ui,
                                &label,
                                Some(font_size),
                                ButtonStyle::toggled(active),
                            )
                            .on_hover_text(tool_tooltip(tool, &view.shortcuts))
                    })
                    .inner;
                if response.clicked() {
                    self.host
                        .events
                        .borrow_mut()
                        .push(ToolbarEvent::ToolSelected(tool));
                }
            }
        });
    }
}

/// Static nest listing the five real tools; the buttons live in the scrollable content.
pub(crate) fn build_toolbox_nest(host: &ToolboxHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ToolboxComponent { host: host.clone() });
    nest
}

/// Dock spec for the Toolbox panel (dock id `PanelId::new(100)`).
pub fn toolbox_panel_spec(host: &ToolboxHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(TOOLBOX_PANEL_ID),
        metadata: PanelMetadata::new("Toolbox", false, vec2(160.0, 120.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(40.0, 80.0), vec2(200.0, 360.0)),
        content: Box::new(build_toolbox_nest(host)),
    }
}
