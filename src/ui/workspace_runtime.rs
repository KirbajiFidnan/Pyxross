//! Shell coordinator for workspace-panel placement and active-project context.
//!
//! This module owns logical panel placement and maps native host events back to
//! the same panel instance without owning project or renderer state.

use egui::ViewportId;
use winit::window::WindowId;

use super::host_registry::{
    ContextGeneration, ContextSnapshot, HostId, HostRegistryError, WindowHostRegistry,
};
use super::panel_layout::{PanelLayout, PanelLayoutError};
use super::panel_registry::{PanelCategory, PanelId, PanelPlacement};
use super::project::{ProjectId, ProjectStore};

pub const WORKSPACE_PANEL_IDS: [PanelId; 4] = [
    PanelId::PanelA,
    PanelId::Preview,
    PanelId::Playback,
    PanelId::FrameEditor,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspacePanelAction {
    PopOut(PanelId),
    DockBack(PanelId),
    CloseNativeCopy { panel: PanelId, host: HostId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspacePanelState {
    Main {
        placement: PanelPlacement,
        native_host: Option<HostId>,
    },
}

impl WorkspacePanelState {
    pub const fn host(self) -> Option<HostId> {
        match self {
            Self::Main { native_host, .. } => native_host,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspacePanelContext {
    panel: PanelId,
    snapshot: ContextSnapshot,
}

impl WorkspacePanelContext {
    pub const fn panel(self) -> PanelId {
        self.panel
    }
    pub const fn project(self) -> ProjectId {
        self.snapshot.project()
    }
    pub const fn snapshot(self) -> ContextSnapshot {
        self.snapshot
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum WorkspaceRuntimeError {
    UnsupportedPanel(PanelId),
    Layout(PanelLayoutError),
    Host(HostRegistryError),
    HostMissing(PanelId),
    HostMismatch { panel: PanelId, host: HostId },
}

impl From<PanelLayoutError> for WorkspaceRuntimeError {
    fn from(error: PanelLayoutError) -> Self {
        Self::Layout(error)
    }
}

impl From<HostRegistryError> for WorkspaceRuntimeError {
    fn from(error: HostRegistryError) -> Self {
        Self::Host(error)
    }
}

/// Coordinates existing shell registries without owning a duplicate panel map.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkspacePanelCoordinator;

impl WorkspacePanelCoordinator {
    pub const fn new() -> Self {
        Self
    }

    pub const fn label(panel: PanelId) -> &'static str {
        match panel {
            PanelId::PanelA => "Panel A",
            PanelId::Preview => "Preview",
            PanelId::Playback => "Playback",
            PanelId::FrameEditor => "Frame Editor",
            PanelId::Layers => "Layers",
            PanelId::ColorPalette => "Color Palette",
            PanelId::TilePalette => "Tile Palette",
            PanelId::Animations => "Animations",
            PanelId::Toolbox => "Toolbox",
        }
    }

    pub fn viewport_id(&self, panel: PanelId) -> Option<ViewportId> {
        (panel.category() == PanelCategory::Workspace)
            .then(|| ViewportId::from_hash_of(("pyxross.workspace", panel)))
    }

    pub fn apply(
        &self,
        action: WorkspacePanelAction,
        layout: &mut PanelLayout,
        hosts: &mut WindowHostRegistry,
    ) -> Result<WorkspacePanelState, WorkspaceRuntimeError> {
        let panel = match action {
            WorkspacePanelAction::PopOut(panel) | WorkspacePanelAction::DockBack(panel) => panel,
            WorkspacePanelAction::CloseNativeCopy { panel, .. } => panel,
        };
        self.require_workspace(panel)?;
        match action {
            WorkspacePanelAction::PopOut(panel) => self.pop_out(panel, layout, hosts),
            WorkspacePanelAction::DockBack(panel) => self.dock_back(panel, layout, hosts),
            WorkspacePanelAction::CloseNativeCopy { panel, host } => {
                self.close_native_copy(panel, host, layout, hosts)
            }
        }
    }

    pub fn state(
        &self,
        panel: PanelId,
        layout: &PanelLayout,
        hosts: &WindowHostRegistry,
    ) -> Result<WorkspacePanelState, WorkspaceRuntimeError> {
        self.require_workspace(panel)?;
        let placement = layout
            .registry()
            .get(panel)
            .map_err(PanelLayoutError::from)?
            .placement();
        match placement {
            PanelPlacement::Docked
            | PanelPlacement::FloatingInMainWindow
            | PanelPlacement::PopOutWindow => {
                let binding = hosts.resolve_panel(panel);
                Ok(WorkspacePanelState::Main {
                    placement,
                    native_host: binding.map(|binding| binding.host()),
                })
            }
        }
    }

    pub fn context_for(
        &self,
        panel: PanelId,
        projects: &ProjectStore,
        generation: ContextGeneration,
    ) -> Option<WorkspacePanelContext> {
        if panel.category() != PanelCategory::Workspace {
            return None;
        }
        projects.active_id().map(|project| WorkspacePanelContext {
            panel,
            snapshot: ContextSnapshot::new(project, generation),
        })
    }

    pub fn os_close_action_for_window(
        &self,
        window: WindowId,
        hosts: &WindowHostRegistry,
    ) -> Option<WorkspacePanelAction> {
        hosts
            .resolve_window(window)
            .map(|binding| WorkspacePanelAction::CloseNativeCopy {
                panel: binding.panel(),
                host: binding.host(),
            })
    }

    fn require_workspace(&self, panel: PanelId) -> Result<(), WorkspaceRuntimeError> {
        (panel.category() == PanelCategory::Workspace)
            .then_some(())
            .ok_or(WorkspaceRuntimeError::UnsupportedPanel(panel))
    }

    fn pop_out(
        &self,
        panel: PanelId,
        layout: &mut PanelLayout,
        hosts: &mut WindowHostRegistry,
    ) -> Result<WorkspacePanelState, WorkspaceRuntimeError> {
        if !layout.open_panels().contains(&panel) {
            return Err(WorkspaceRuntimeError::Layout(
                PanelLayoutError::ClosedPanel(panel),
            ));
        }
        match hosts.resolve_panel(panel) {
            Some(_) => {}
            None => {
                let viewport = self
                    .viewport_id(panel)
                    .ok_or(WorkspaceRuntimeError::UnsupportedPanel(panel))?;
                hosts.create(panel, viewport)?;
            }
        }
        self.state(panel, layout, hosts)
    }

    fn dock_back(
        &self,
        panel: PanelId,
        layout: &mut PanelLayout,
        hosts: &mut WindowHostRegistry,
    ) -> Result<WorkspacePanelState, WorkspaceRuntimeError> {
        let placement = layout
            .registry()
            .get(panel)
            .map_err(PanelLayoutError::from)?
            .last_main_placement();
        layout.transition(panel, placement)?;
        if let Some(binding) = hosts.resolve_panel(panel) {
            hosts.retire(binding.host())?;
        }
        self.state(panel, layout, hosts)
    }

    fn close_native_copy(
        &self,
        panel: PanelId,
        host: HostId,
        layout: &mut PanelLayout,
        hosts: &mut WindowHostRegistry,
    ) -> Result<WorkspacePanelState, WorkspaceRuntimeError> {
        let Some(binding) = hosts.resolve_panel(panel) else {
            return Err(WorkspaceRuntimeError::HostMissing(panel));
        };
        if binding.host() != host {
            return Err(WorkspaceRuntimeError::HostMismatch { panel, host });
        }
        hosts.retire(host)?;
        self.state(panel, layout, hosts)
    }
}
