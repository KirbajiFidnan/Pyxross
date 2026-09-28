//! Runtime host identity and context-generation routing.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use egui::ViewportId;
use winit::window::WindowId;

use super::panel_registry::{PanelCategory, PanelId, PanelRegistry};
use super::project::ProjectId;

/// Monotonic identity for a live panel host.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct HostId(u64);

impl HostId {
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Monotonic generation used to reject queued events from an old context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ContextGeneration(u64);

impl ContextGeneration {
    pub const fn initial() -> Self {
        Self(0)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextGenerationService {
    current: ContextGeneration,
}

impl Default for ContextGenerationService {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextGenerationService {
    pub const fn new() -> Self {
        Self {
            current: ContextGeneration::initial(),
        }
    }
    pub const fn current(self) -> ContextGeneration {
        self.current
    }

    pub fn advance(&mut self) -> ContextGeneration {
        self.current = ContextGeneration(
            self.current
                .0
                .checked_add(1)
                .expect("context generation exhausted"),
        );
        self.current
    }

    pub fn accepts(self, generation: ContextGeneration) -> bool {
        self.current == generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextSnapshot {
    project: ProjectId,
    generation: ContextGeneration,
}

impl ContextSnapshot {
    pub const fn new(project: ProjectId, generation: ContextGeneration) -> Self {
        Self {
            project,
            generation,
        }
    }
    pub const fn project(self) -> ProjectId {
        self.project
    }
    pub const fn generation(self) -> ContextGeneration {
        self.generation
    }
    pub fn accepts(self, project: ProjectId, generation: ContextGeneration) -> bool {
        self.project == project && self.generation == generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostBinding {
    host: HostId,
    panel: PanelId,
    viewport: ViewportId,
    window: Option<WindowId>,
}

impl HostBinding {
    pub const fn host(self) -> HostId {
        self.host
    }
    pub const fn panel(self) -> PanelId {
        self.panel
    }
    pub const fn viewport(self) -> ViewportId {
        self.viewport
    }
    pub const fn window(self) -> Option<WindowId> {
        self.window
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostRegistryError {
    UnknownHost(HostId),
    UnknownPanel(PanelId),
    DuplicatePanel(PanelId),
    DuplicateViewport(ViewportId),
    DuplicateWindow(WindowId),
    WindowAlreadyBound(HostId),
    UnsupportedPanel(PanelId),
}

/// Maps stable panel identity to runtime host, viewport, and window identity.
#[derive(Debug, Default)]
pub struct WindowHostRegistry {
    panels: PanelRegistry,
    hosts: BTreeMap<HostId, HostBinding>,
    panel_hosts: BTreeMap<PanelId, HostId>,
    viewport_hosts: HashMap<ViewportId, HostId>,
    window_hosts: HashMap<WindowId, HostId>,
    retired: BTreeSet<HostId>,
    next_host: u64,
}

impl WindowHostRegistry {
    pub fn new() -> Self {
        Self {
            panels: PanelRegistry::new(),
            ..Self::default()
        }
    }

    pub fn create(
        &mut self,
        panel: PanelId,
        viewport: ViewportId,
    ) -> Result<HostId, HostRegistryError> {
        let instance = self
            .panels
            .get(panel)
            .map_err(|_| HostRegistryError::UnknownPanel(panel))?;
        if instance.category() != PanelCategory::Workspace {
            return Err(HostRegistryError::UnsupportedPanel(panel));
        }
        if self.panel_hosts.contains_key(&panel) {
            return Err(HostRegistryError::DuplicatePanel(panel));
        }
        if self.viewport_hosts.contains_key(&viewport) {
            return Err(HostRegistryError::DuplicateViewport(viewport));
        }
        self.next_host = self.next_host.checked_add(1).expect("host ID exhausted");
        let host = HostId(self.next_host);
        self.hosts.insert(
            host,
            HostBinding {
                host,
                panel,
                viewport,
                window: None,
            },
        );
        self.panel_hosts.insert(panel, host);
        self.viewport_hosts.insert(viewport, host);
        Ok(host)
    }

    pub fn bind_window(&mut self, host: HostId, window: WindowId) -> Result<(), HostRegistryError> {
        if self.window_hosts.contains_key(&window) {
            return Err(HostRegistryError::DuplicateWindow(window));
        }
        let binding = self
            .hosts
            .get_mut(&host)
            .ok_or(HostRegistryError::UnknownHost(host))?;
        if binding.window.is_some() {
            return Err(HostRegistryError::WindowAlreadyBound(host));
        }
        binding.window = Some(window);
        self.window_hosts.insert(window, host);
        Ok(())
    }

    pub fn resolve_host(&self, host: HostId) -> Option<HostBinding> {
        self.hosts.get(&host).copied()
    }
    pub fn resolve_panel(&self, panel: PanelId) -> Option<HostBinding> {
        self.panel_hosts
            .get(&panel)
            .and_then(|host| self.resolve_host(*host))
    }
    pub fn resolve_viewport(&self, viewport: ViewportId) -> Option<HostBinding> {
        self.viewport_hosts
            .get(&viewport)
            .and_then(|host| self.resolve_host(*host))
    }
    pub fn resolve_window(&self, window: WindowId) -> Option<HostBinding> {
        self.window_hosts
            .get(&window)
            .and_then(|host| self.resolve_host(*host))
    }
    pub fn len(&self) -> usize {
        self.hosts.len()
    }
    pub fn is_retired(&self, host: HostId) -> bool {
        self.retired.contains(&host)
    }

    pub fn is_consistent(&self) -> bool {
        self.hosts.len() == self.panel_hosts.len()
            && self.hosts.len() == self.viewport_hosts.len()
            && self.hosts.values().all(|binding| {
                self.panel_hosts.get(&binding.panel) == Some(&binding.host)
                    && self.viewport_hosts.get(&binding.viewport) == Some(&binding.host)
                    && binding.window.map_or(true, |window| {
                        self.window_hosts.get(&window) == Some(&binding.host)
                    })
            })
            && self.window_hosts.iter().all(|(window, host)| {
                self.hosts.get(host).and_then(|binding| binding.window) == Some(*window)
            })
    }

    pub fn retire(&mut self, host: HostId) -> Result<HostBinding, HostRegistryError> {
        let binding = self
            .hosts
            .remove(&host)
            .ok_or(HostRegistryError::UnknownHost(host))?;
        self.panel_hosts.remove(&binding.panel);
        self.viewport_hosts.remove(&binding.viewport);
        if let Some(window) = binding.window {
            self.window_hosts.remove(&window);
        }
        self.retired.insert(host);
        Ok(binding)
    }
}
