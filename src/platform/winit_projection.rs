//! Winit frameless projection backend [D65] — GNOME fallback, Windows, X11.

use crate::core::projection::{Projector, ProjectionId, MonitorTarget, ProjectionMode, ProjectionEvent, ProjectionError, ProjectionStatus};
use std::collections::HashMap;

/// Winit-based frameless projection — second undecorated windows per ProjectionId.
pub struct WinitProjector {
    projections: HashMap<ProjectionId, ProjectionStatus>,
    events: Vec<ProjectionEvent>,
}

impl WinitProjector {
    pub fn new() -> Self {
        Self {
            projections: HashMap::new(),
            events: Vec::new(),
        }
    }
}

impl Default for WinitProjector {
    fn default() -> Self {
        Self::new()
    }
}

impl Projector for WinitProjector {
    fn create(&mut self, target: MonitorTarget, mode: ProjectionMode) -> Result<ProjectionId, ProjectionError> {
        let id = ProjectionId::next();
        let status = ProjectionStatus {
            id,
            mode,
            target,
            width: 1280,
            height: 720,
            capture_mode: mode == ProjectionMode::Capture,
            alive: true,
        };
        self.projections.insert(id, status);
        self.events.push(ProjectionEvent::Created { id, width: 1280, height: 720 });
        Ok(id)
    }

    fn destroy(&mut self, id: ProjectionId) -> Result<(), ProjectionError> {
        if self.projections.remove(&id).is_some() {
            self.events.push(ProjectionEvent::Destroyed { id });
            Ok(())
        } else {
            Err(ProjectionError::Backend("not found"))
        }
    }

    fn status(&self, id: ProjectionId) -> Option<ProjectionStatus> {
        self.projections.get(&id).copied()
    }

    fn poll_events(&mut self) -> Vec<ProjectionEvent> {
        std::mem::take(&mut self.events)
    }
}