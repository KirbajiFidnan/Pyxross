//! Canvas project-tab view and shell events.

use crate::ui::project::{ProjectId, ProjectTab};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectTabEvent {
    Activate(ProjectId),
    Close(ProjectId),
}

#[derive(Default)]
pub struct ProjectTabBar;

impl ProjectTabBar {
    pub fn new() -> Self {
        Self
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        tabs: &[ProjectTab],
        events: &mut Vec<ProjectTabEvent>,
    ) {
        ui.horizontal_wrapped(|ui| {
            for tab in tabs {
                let response = ui.selectable_label(tab.active, &tab.name);
                if response.clicked() && !tab.active {
                    events.push(ProjectTabEvent::Activate(tab.id));
                }
                if ui.small_button("x").clicked() {
                    events.push(ProjectTabEvent::Close(tab.id));
                }
            }
        });
    }
}
