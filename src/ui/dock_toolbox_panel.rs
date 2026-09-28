//! Toolbox dock panel: a static nest listing the application drawing tools.

use egui::vec2;

use crate::ui::dock_hosts::ToolboxHost;
use crate::ui::panel_dock::nest::NestView;
use crate::ui::panel_dock::{
    ButtonStyle, Component, NestContent, NestMode, PanelChrome, PanelId, PanelMetadata,
    PanelPlacement, PanelSpec,
};
use crate::ui::toolbar::{tool_label, ToolbarEvent, TOOLS};

/// The dock id for the Toolbox panel.
const TOOLBOX_PANEL_ID: u64 = 100;

/// Content height tall enough to overflow a short dock viewport, so the tool
/// buttons scroll with the nest content instead of being pinned. The width
/// follows the panel so a narrow dock never overflows horizontally.
const TOOLBOX_CONTENT_HEIGHT: f32 = 360.0;

/// One selectable tool row: highlights the active tool and emits a
/// [`ToolbarEvent::ToolSelected`] on click.
struct ToolboxComponent {
    host: ToolboxHost,
}

impl Component for ToolboxComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        ui.set_min_size(vec2(ui.available_width(), TOOLBOX_CONTENT_HEIGHT));
        let active_tool = self.host.view.borrow().tool;
        for tool in TOOLS {
            let active = active_tool == tool;
            if chrome
                .button(ui, tool_label(tool), ButtonStyle::toggled(active))
                .clicked()
            {
                self.host
                    .events
                    .borrow_mut()
                    .push(ToolbarEvent::ToolSelected(tool));
            }
        }
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
