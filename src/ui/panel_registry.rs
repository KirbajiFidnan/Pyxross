//! Shell-owned panel identity and placement state.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable logical identity for every panel instance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum PanelId {
    PanelA,
    Preview,
    Playback,
    FrameEditor,
    Layers,
    ColorPalette,
    TilePalette,
    Animations,
    Toolbox,
}

impl PanelId {
    pub const ALL: [Self; 9] = [
        Self::PanelA,
        Self::Preview,
        Self::Playback,
        Self::FrameEditor,
        Self::Layers,
        Self::ColorPalette,
        Self::TilePalette,
        Self::Animations,
        Self::Toolbox,
    ];

    pub const fn category(self) -> PanelCategory {
        match self {
            Self::PanelA | Self::Preview | Self::Playback | Self::FrameEditor => {
                PanelCategory::Workspace
            }
            Self::Layers
            | Self::ColorPalette
            | Self::TilePalette
            | Self::Animations
            | Self::Toolbox => PanelCategory::Tool,
        }
    }
}

/// Whether a panel may become an OS-hosted workspace surface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PanelCategory {
    Workspace,
    Tool,
}

/// The one visible host mode owned by a panel instance.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum PanelPlacement {
    #[default]
    Docked,
    FloatingInMainWindow,
    PopOutWindow,
}

impl PanelPlacement {
    const fn is_main_window(self) -> bool {
        matches!(self, Self::Docked | Self::FloatingInMainWindow)
    }
}

/// A stable logical panel and its last valid main-window placement.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PanelInstance {
    id: PanelId,
    placement: PanelPlacement,
    last_main_placement: PanelPlacement,
}

impl PanelInstance {
    const fn new(id: PanelId) -> Self {
        Self {
            id,
            placement: PanelPlacement::Docked,
            last_main_placement: PanelPlacement::Docked,
        }
    }

    pub const fn id(self) -> PanelId {
        self.id
    }
    pub const fn category(self) -> PanelCategory {
        self.id.category()
    }
    pub const fn placement(self) -> PanelPlacement {
        self.placement
    }
    pub const fn last_main_placement(self) -> PanelPlacement {
        self.last_main_placement
    }
}

/// Typed failures for invalid registry transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelRegistryError {
    DuplicatePanel(PanelId),
    UnknownPanel(PanelId),
    UnsupportedPlacement {
        panel: PanelId,
        placement: PanelPlacement,
    },
}

impl fmt::Display for PanelRegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicatePanel(panel) => {
                write!(formatter, "panel {panel:?} is already registered")
            }
            Self::UnknownPanel(panel) => write!(formatter, "panel {panel:?} is not registered"),
            Self::UnsupportedPlacement { panel, placement } => {
                write!(
                    formatter,
                    "panel {panel:?} cannot use placement {placement:?}"
                )
            }
        }
    }
}

impl std::error::Error for PanelRegistryError {}

/// Owns exactly one logical instance for each registered panel.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PanelRegistry {
    panels: BTreeMap<PanelId, PanelInstance>,
}

impl PanelRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        for id in PanelId::ALL {
            registry.register(id).expect("default panel IDs are unique");
        }
        registry
    }

    pub fn register(&mut self, id: PanelId) -> Result<(), PanelRegistryError> {
        if self.panels.contains_key(&id) {
            return Err(PanelRegistryError::DuplicatePanel(id));
        }
        self.panels.insert(id, PanelInstance::new(id));
        Ok(())
    }

    pub fn get(&self, id: PanelId) -> Result<PanelInstance, PanelRegistryError> {
        self.panels
            .get(&id)
            .copied()
            .ok_or(PanelRegistryError::UnknownPanel(id))
    }

    pub fn len(&self) -> usize {
        self.panels.len()
    }
    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = PanelInstance> + '_ {
        self.panels.values().copied()
    }

    pub fn transition(
        &mut self,
        id: PanelId,
        placement: PanelPlacement,
    ) -> Result<(), PanelRegistryError> {
        let panel = self
            .panels
            .get_mut(&id)
            .ok_or(PanelRegistryError::UnknownPanel(id))?;
        if placement == PanelPlacement::PopOutWindow && panel.category() == PanelCategory::Tool {
            return Err(PanelRegistryError::UnsupportedPlacement {
                panel: id,
                placement,
            });
        }
        if placement.is_main_window() {
            panel.last_main_placement = placement;
        }
        panel.placement = placement;
        Ok(())
    }

    pub fn dock_back(&mut self, id: PanelId) -> Result<(), PanelRegistryError> {
        let placement = self.get(id)?.last_main_placement();
        self.transition(id, placement)
    }
}
