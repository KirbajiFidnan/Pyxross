//! Global, versioned persistence for panel layout and geometry.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use egui_dock::DockState;
use serde::{Deserialize, Serialize};

use super::panel_registry::{PanelId, PanelPlacement};

pub const WORKSPACE_LAYOUT_VERSION: u32 = 1;
const MAX_GEOMETRY: f32 = 16_384.0;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct PanelGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl PanelGeometry {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(|value| value.is_finite())
            && self.width > 0.0
            && self.height > 0.0
            && self.width <= MAX_GEOMETRY
            && self.height <= MAX_GEOMETRY
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PanelLayoutRecord {
    pub panel: PanelId,
    pub placement: PanelPlacement,
    pub last_main_placement: PanelPlacement,
    pub desired_popout: bool,
    pub geometry: Option<PanelGeometry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkspaceLayoutV1 {
    pub version: u32,
    pub dock_state: DockState<PanelId>,
    pub panels: Vec<PanelLayoutRecord>,
}

#[derive(Debug)]
pub enum WorkspaceLayoutError {
    Io(std::io::Error),
    Json(String),
    UnsupportedVersion(u32),
    Invalid(String),
}

impl fmt::Display for WorkspaceLayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "workspace layout I/O error: {error}"),
            Self::Json(error) => write!(formatter, "workspace layout JSON error: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported workspace layout version {version}")
            }
            Self::Invalid(error) => write!(formatter, "invalid workspace layout: {error}"),
        }
    }
}

impl std::error::Error for WorkspaceLayoutError {}

impl From<std::io::Error> for WorkspaceLayoutError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl Default for WorkspaceLayoutV1 {
    fn default() -> Self {
        let panels = PanelId::ALL
            .into_iter()
            .map(|panel| PanelLayoutRecord {
                panel,
                placement: PanelPlacement::Docked,
                last_main_placement: PanelPlacement::Docked,
                desired_popout: false,
                geometry: None,
            })
            .collect();
        let mut dock_state = DockState::new(PanelId::ALL.to_vec());
        if let Some(root) = dock_state.main_surface_mut().root_node_mut() {
            if let Some(leaf) = root.get_leaf_mut() {
                let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1024.0, 768.0));
                leaf.rect = bounds;
                leaf.viewport = bounds;
            }
        }
        Self {
            version: WORKSPACE_LAYOUT_VERSION,
            dock_state,
            panels,
        }
    }
}

impl WorkspaceLayoutV1 {
    pub fn normalized_for_startup(mut self) -> Self {
        for record in &mut self.panels {
            record.placement = match record.last_main_placement {
                PanelPlacement::Docked | PanelPlacement::FloatingInMainWindow => {
                    record.last_main_placement
                }
                PanelPlacement::PopOutWindow => PanelPlacement::Docked,
            };
            record.desired_popout = false;
        }
        self
    }

    pub fn validate(&self) -> Result<(), WorkspaceLayoutError> {
        if self.version != WORKSPACE_LAYOUT_VERSION {
            return Err(WorkspaceLayoutError::UnsupportedVersion(self.version));
        }
        let mut records = BTreeSet::new();
        for record in &self.panels {
            if !records.insert(record.panel) {
                return Err(WorkspaceLayoutError::Invalid(format!(
                    "duplicate panel {:?}",
                    record.panel
                )));
            }
            if record.panel.category() == super::panel_registry::PanelCategory::Tool
                && (record.placement == PanelPlacement::PopOutWindow || record.desired_popout)
            {
                return Err(WorkspaceLayoutError::Invalid(format!(
                    "tool panel {:?} cannot pop out",
                    record.panel
                )));
            }
            if !record
                .placement
                .is_main_window_for_persistence(record.panel)
                && record.last_main_placement == PanelPlacement::PopOutWindow
            {
                return Err(WorkspaceLayoutError::Invalid(format!(
                    "panel {:?} has no valid main placement",
                    record.panel
                )));
            }
            if let Some(geometry) = record.geometry {
                if !geometry.is_valid() {
                    return Err(WorkspaceLayoutError::Invalid(format!(
                        "invalid geometry for {:?}",
                        record.panel
                    )));
                }
            }
        }
        let mut tabs = BTreeSet::new();
        for (_, panel) in self.dock_state.iter_all_tabs() {
            if !tabs.insert(*panel) {
                return Err(WorkspaceLayoutError::Invalid(format!(
                    "duplicate dock tab {:?}",
                    panel
                )));
            }
            if !records.contains(panel) {
                return Err(WorkspaceLayoutError::Invalid(format!(
                    "dock tab {:?} has no record",
                    panel
                )));
            }
        }
        if tabs != records {
            return Err(WorkspaceLayoutError::Invalid(
                "dock tabs and panel records differ".to_string(),
            ));
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, WorkspaceLayoutError> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map_err(|error| WorkspaceLayoutError::Json(error.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self, WorkspaceLayoutError> {
        let layout: Self = serde_json::from_str(json)
            .map_err(|error| WorkspaceLayoutError::Json(error.to_string()))?;
        layout.validate()?;
        Ok(layout)
    }

    pub fn load_or_default(path: &Path) -> Result<Self, WorkspaceLayoutError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        match fs::read_to_string(path) {
            Ok(json) => match Self::from_json(&json) {
                Ok(layout) => Ok(layout),
                Err(_) => Ok(Self::default()),
            },
            Err(error) => Err(WorkspaceLayoutError::Io(error)),
        }
    }

    pub fn save_atomic(&self, path: &Path) -> Result<(), WorkspaceLayoutError> {
        let json = self.to_json()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp = path.with_extension(format!("tmp-{}-{stamp}", std::process::id()));
        fs::write(&temp, json)?;
        fs::rename(temp, path)?;
        Ok(())
    }
}

trait PlacementPersistenceExt {
    fn is_main_window_for_persistence(self, panel: PanelId) -> bool;
}

impl PlacementPersistenceExt for PanelPlacement {
    fn is_main_window_for_persistence(self, panel: PanelId) -> bool {
        self != PanelPlacement::PopOutWindow
            || panel.category() == super::panel_registry::PanelCategory::Workspace
    }
}
