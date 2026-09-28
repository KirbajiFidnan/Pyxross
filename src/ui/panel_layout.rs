//! Same-window docking state for the shell-owned panel registry.

use std::collections::BTreeSet;

use egui_dock::DockState;

use super::panel_registry::{PanelId, PanelPlacement, PanelRegistry, PanelRegistryError};

#[derive(Debug, PartialEq, Eq)]
pub enum PanelLayoutError {
    Registry(PanelRegistryError),
    ClosedPanel(PanelId),
    UnknownPanel(PanelId),
    DuplicatePanel(PanelId),
    InvalidTopology,
}

impl From<PanelRegistryError> for PanelLayoutError {
    fn from(error: PanelRegistryError) -> Self {
        Self::Registry(error)
    }
}

/// Owns the serializable in-main-window topology while the registry owns identity.
#[derive(Debug)]
pub struct PanelLayout {
    registry: PanelRegistry,
    open: Vec<PanelId>,
    dock_state: DockState<PanelId>,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl PanelLayout {
    pub fn new() -> Self {
        let open = PanelId::ALL.to_vec();
        Self {
            registry: PanelRegistry::new(),
            dock_state: DockState::new(open.clone()),
            open,
        }
    }

    pub fn registry(&self) -> &PanelRegistry {
        &self.registry
    }
    pub fn dock_state(&self) -> &DockState<PanelId> {
        &self.dock_state
    }
    pub fn open_panels(&self) -> &[PanelId] {
        &self.open
    }

    pub fn transition(
        &mut self,
        panel: PanelId,
        placement: PanelPlacement,
    ) -> Result<(), PanelLayoutError> {
        if !self.open.contains(&panel) {
            return Err(PanelLayoutError::ClosedPanel(panel));
        }
        self.registry.transition(panel, placement)?;
        Ok(())
    }

    pub fn close(&mut self, panel: PanelId) -> Result<(), PanelLayoutError> {
        let Some(position) = self.open.iter().position(|candidate| *candidate == panel) else {
            return Err(PanelLayoutError::UnknownPanel(panel));
        };
        self.open.remove(position);
        self.rebuild_dock_state();
        Ok(())
    }

    pub fn reopen(&mut self, panel: PanelId) -> Result<(), PanelLayoutError> {
        self.registry.get(panel)?;
        if self.open.contains(&panel) {
            return Err(PanelLayoutError::DuplicatePanel(panel));
        }
        self.open.push(panel);
        self.rebuild_dock_state();
        Ok(())
    }

    pub fn reorder(&mut self, panel: PanelId, target: usize) -> Result<(), PanelLayoutError> {
        let Some(position) = self.open.iter().position(|candidate| *candidate == panel) else {
            return Err(PanelLayoutError::ClosedPanel(panel));
        };
        if target >= self.open.len() {
            return Err(PanelLayoutError::ClosedPanel(panel));
        }
        let panel = self.open.remove(position);
        self.open.insert(target, panel);
        self.rebuild_dock_state();
        Ok(())
    }

    pub fn validate(&self) -> Result<(), PanelLayoutError> {
        let mut seen = BTreeSet::new();
        for (_, panel) in self.dock_state.iter_all_tabs() {
            if !seen.insert(*panel) {
                return Err(PanelLayoutError::DuplicatePanel(*panel));
            }
            self.registry.get(*panel)?;
        }
        if seen.len() != self.open.len() || self.open.iter().any(|panel| !seen.contains(panel)) {
            return Err(PanelLayoutError::InvalidTopology);
        }
        Ok(())
    }

    fn rebuild_dock_state(&mut self) {
        self.dock_state = DockState::new(self.open.clone());
    }
}
