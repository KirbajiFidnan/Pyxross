use std::collections::BTreeMap;

use egui::{Pos2, Rect, Vec2};

use super::geometry::{DockAreaState, LEFT_DOCK_DEFAULT_EXTENT};
use super::preview::PreviewFeed;
use super::skin::PanelChrome;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockAction {
    PopOut(PanelId),
    ClosePanel(PanelId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelHeaderAction {
    Close,
    Float,
    PopOut,
    Dock,
}

impl PanelHeaderAction {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Close => "Close",
            Self::Float => "Float",
            Self::PopOut => "Pop out",
            Self::Dock => "Dock",
        }
    }
}

impl DockAction {
    pub const fn canonical_panel(self) -> Option<crate::ui::panel_registry::PanelId> {
        match self {
            Self::PopOut(panel) if panel.raw() == 1 => {
                Some(crate::ui::panel_registry::PanelId::PanelA)
            }
            Self::PopOut(_) | Self::ClosePanel(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PanelId(u64);

impl PanelId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelPlacement {
    Floating,
    DockedLeft,
    DockedRight,
    DockedBottom,
    Native,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockSide {
    Left,
    Right,
    Bottom,
}

#[derive(Debug)]
pub enum DockError {
    DuplicatePanel(PanelId),
    UnknownPanel(PanelId),
    PopOutNotAllowed(PanelId),
    /// A floating-only panel (`can_pop_out == false` while floating) was asked
    /// to dock; it must stay floating forever.
    DockNotAllowed(PanelId),
    WrongDock(PanelId),
}

pub trait PanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome);
}

pub struct PanelMetadata {
    pub title: String,
    pub can_pop_out: bool,
    pub can_native_pop_out: bool,
    /// Whether the floating window can be resized by dragging its edges.
    pub resizable: bool,
    pub min_size: Vec2,
}

impl PanelMetadata {
    pub fn new(title: impl Into<String>, can_pop_out: bool, min_size: Vec2) -> Self {
        Self {
            title: title.into(),
            can_pop_out,
            can_native_pop_out: false,
            resizable: true,
            min_size,
        }
    }

    pub fn native_pop_out(title: impl Into<String>, min_size: Vec2) -> Self {
        Self {
            title: title.into(),
            can_pop_out: true,
            can_native_pop_out: true,
            resizable: true,
            min_size,
        }
    }

    pub const fn header_action(&self, placement: PanelPlacement) -> Option<PanelHeaderAction> {
        match placement {
            PanelPlacement::Floating => {
                if self.can_pop_out {
                    Some(PanelHeaderAction::Dock)
                } else {
                    // Floating-only (not dockable): the header offers Close.
                    Some(PanelHeaderAction::Close)
                }
            }
            PanelPlacement::DockedLeft
            | PanelPlacement::DockedRight
            | PanelPlacement::DockedBottom
            | PanelPlacement::Native => {
                if self.can_native_pop_out {
                    Some(PanelHeaderAction::PopOut)
                } else if self.can_pop_out {
                    Some(PanelHeaderAction::Float)
                } else {
                    None
                }
            }
        }
    }

    /// Whether the panel is floating-only at `placement`: it floats and must
    /// never dock.
    ///
    /// `can_pop_out == false` alone is not float-only: docked panels that may
    /// not pop out (Toolbox, Layers) stay docked. The rule is exactly
    /// `can_pop_out == false && placement == Floating`.
    pub const fn is_float_only(&self, placement: PanelPlacement) -> bool {
        !self.can_pop_out && matches!(placement, PanelPlacement::Floating)
    }
}

pub struct PanelSpec {
    pub id: PanelId,
    pub metadata: PanelMetadata,
    pub placement: PanelPlacement,
    pub floating_rect: Rect,
    pub content: Box<dyn PanelContent>,
}

pub(super) struct PanelEntry {
    pub(super) metadata: PanelMetadata,
    pub(super) placement: PanelPlacement,
    pub(super) floating_rect: Rect,
    pub(super) content: Box<dyn PanelContent>,
}

pub(super) struct DragSession {
    pub panel: PanelId,
    pub start: Pos2,
    pub current: Pos2,
    pub offset: Vec2,
}

pub struct DockManager {
    pub(super) panels: BTreeMap<PanelId, PanelEntry>,
    pub(super) left: DockAreaState,
    pub(super) right: DockAreaState,
    pub(super) bottom: DockAreaState,
    pub(super) drag: Option<DragSession>,
    pub(super) actions: Vec<DockAction>,
    last_dock_positions: BTreeMap<PanelId, DockPosition>,
    /// The rect each floating panel occupied when it was last removed, so a
    /// re-added panel can reopen exactly where it was closed (session-only).
    last_floating_rects: BTreeMap<PanelId, Rect>,
    /// Floating panels ordered by use: oldest first, most recently clicked
    /// last. Session-only stacking order (see the `UiLayer` contract).
    floating_mru: Vec<PanelId>,
    /// Canvas image published by the app for the preview panel.
    pub(super) preview_feed: PreviewFeed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DockPosition {
    side: DockSide,
    order: usize,
}

impl DockManager {
    pub(super) fn empty() -> Self {
        Self {
            panels: BTreeMap::new(),
            left: DockAreaState::new(DockSide::Left),
            right: DockAreaState::new(DockSide::Right),
            bottom: DockAreaState::new(DockSide::Bottom),
            drag: None,
            actions: Vec::new(),
            last_dock_positions: BTreeMap::new(),
            last_floating_rects: BTreeMap::new(),
            floating_mru: Vec::new(),
            preview_feed: PreviewFeed::default(),
        }
    }

    pub fn try_new(specs: impl IntoIterator<Item = PanelSpec>) -> Result<Self, DockError> {
        let mut manager = Self::empty();
        for spec in specs {
            if manager.panels.contains_key(&spec.id) {
                return Err(DockError::DuplicatePanel(spec.id));
            }
            let id = spec.id;
            let placement = spec.placement;
            manager.panels.insert(
                id,
                PanelEntry {
                    metadata: spec.metadata,
                    placement,
                    floating_rect: spec.floating_rect,
                    content: spec.content,
                },
            );
            manager.add_to_dock(id, placement);
        }
        Ok(manager)
    }

    /// Adds one panel spec to an existing manager, erroring on a duplicate id.
    ///
    /// Mirrors the per-spec logic of [`DockManager::try_new`] so the App can
    /// extend the demo dock with real panels without rebuilding it.
    pub fn add_spec(&mut self, spec: PanelSpec) -> Result<(), DockError> {
        if self.panels.contains_key(&spec.id) {
            return Err(DockError::DuplicatePanel(spec.id));
        }
        let id = spec.id;
        let placement = spec.placement;
        self.panels.insert(
            id,
            PanelEntry {
                metadata: spec.metadata,
                placement,
                floating_rect: spec.floating_rect,
                content: spec.content,
            },
        );
        self.add_to_dock(id, placement);
        Ok(())
    }

    /// Removes a panel entirely (from the panel map, any dock area, the floating
    /// MRU and the remembered dock positions). Returns false when unknown.
    ///
    /// A floating panel's rect is remembered so the same spec re-added later
    /// reopens at the same spot.
    pub fn remove_spec(&mut self, id: PanelId) -> bool {
        let Some(entry) = self.panels.remove(&id) else {
            return false;
        };
        if entry.placement == PanelPlacement::Floating {
            self.last_floating_rects.insert(id, entry.floating_rect);
        }
        self.remove_from_docks(id);
        self.floating_mru.retain(|candidate| *candidate != id);
        self.last_dock_positions.remove(&id);
        true
    }

    /// Replaces a panel's title (used by panels whose title reflects state).
    pub fn set_title(&mut self, id: PanelId, title: impl Into<String>) {
        if let Some(entry) = self.panels.get_mut(&id) {
            entry.metadata.title = title.into();
        }
    }

    pub fn panel_count(&self) -> usize {
        self.panels.len()
    }

    /// Floating panels by use order: oldest first, most recently clicked last.
    pub fn floating_order(&self) -> &[PanelId] {
        &self.floating_mru
    }

    /// Raises `id` to the top of the floating stack.
    ///
    /// Non-floating panels are ignored; the order is session-only.
    pub fn raise_floating(&mut self, id: PanelId) {
        if self.placement(id) != Some(PanelPlacement::Floating) {
            return;
        }
        self.floating_mru.retain(|candidate| *candidate != id);
        self.floating_mru.push(id);
    }

    /// The canvas-image feed the preview panel reads.
    pub fn preview_feed(&self) -> PreviewFeed {
        self.preview_feed.clone()
    }

    pub fn drain_actions(&mut self) -> Vec<DockAction> {
        std::mem::take(&mut self.actions)
    }

    pub fn transition_panel_a_to_native(&mut self) -> Result<(), DockError> {
        self.transition_panel(PanelId::new(1), PanelPlacement::Native)
    }

    pub fn restore_panel_a(&mut self) -> Result<(), DockError> {
        self.restore_panel(PanelId::new(1))
    }

    pub fn instance_identity(&self, id: PanelId) -> Option<PanelId> {
        self.panels.get(&id).map(|_| id)
    }

    pub fn placement(&self, id: PanelId) -> Option<PanelPlacement> {
        self.panels.get(&id).map(|entry| entry.placement)
    }

    pub fn metadata(&self, id: PanelId) -> Option<&PanelMetadata> {
        self.panels.get(&id).map(|entry| &entry.metadata)
    }

    /// Whether `id` is floating-only right now: floating and unable to pop
    /// out, so it can never be docked.
    pub fn is_float_only(&self, id: PanelId) -> bool {
        self.panels
            .get(&id)
            .is_some_and(|entry| entry.metadata.is_float_only(entry.placement))
    }

    pub fn header_action(&self, id: PanelId) -> Option<PanelHeaderAction> {
        let entry = self.panels.get(&id)?;
        entry.metadata.header_action(entry.placement)
    }

    pub fn transition_panel(
        &mut self,
        id: PanelId,
        placement: PanelPlacement,
    ) -> Result<(), DockError> {
        let Some(entry) = self.panels.get(&id) else {
            return Err(DockError::UnknownPanel(id));
        };
        if placement == PanelPlacement::Floating && !entry.metadata.can_pop_out {
            return Err(DockError::PopOutNotAllowed(id));
        }
        if placement != PanelPlacement::Floating && entry.metadata.is_float_only(entry.placement) {
            return Err(DockError::DockNotAllowed(id));
        }
        if placement == PanelPlacement::Native && id.raw() != 1 {
            return Err(DockError::PopOutNotAllowed(id));
        }
        self.remove_from_docks(id);
        if let Some(entry) = self.panels.get_mut(&id) {
            entry.placement = placement;
        }
        self.add_to_dock(id, placement);
        if placement == PanelPlacement::Floating {
            self.raise_floating(id);
        } else {
            self.floating_mru.retain(|candidate| *candidate != id);
        }
        Ok(())
    }

    pub fn restore_panel(&mut self, id: PanelId) -> Result<(), DockError> {
        let target = self
            .last_dock_positions
            .get(&id)
            .copied()
            .unwrap_or(DockPosition {
                side: DockSide::Right,
                order: self.right.panel_ids.len(),
            });
        let placement = match target.side {
            DockSide::Left => PanelPlacement::DockedLeft,
            DockSide::Right => PanelPlacement::DockedRight,
            DockSide::Bottom => PanelPlacement::DockedBottom,
        };
        self.transition_panel(id, placement)?;
        self.reorder(target.side, id, target.order)
    }

    pub fn set_floating_rect(&mut self, id: PanelId, rect: Rect) {
        if let Some(entry) = self.panels.get_mut(&id) {
            entry.floating_rect = rect;
        }
    }

    pub fn floating_rect(&self, id: PanelId) -> Option<Rect> {
        self.panels.get(&id).map(|entry| entry.floating_rect)
    }

    /// The rect a floating panel occupied when it was last removed, if any.
    pub fn last_floating_rect(&self, id: PanelId) -> Option<Rect> {
        self.last_floating_rects.get(&id).copied()
    }

    pub fn reorder(
        &mut self,
        side: DockSide,
        id: PanelId,
        destination: usize,
    ) -> Result<(), DockError> {
        if self
            .placement(id)
            .and_then(|placement| placement.dock_side())
            != Some(side)
        {
            return Err(DockError::WrongDock(id));
        }
        let ids = &mut self.area_mut(side).panel_ids;
        let Some(source) = ids.iter().position(|candidate| *candidate == id) else {
            return Err(DockError::WrongDock(id));
        };
        let panel = ids.remove(source);
        ids.insert(destination.min(ids.len()), panel);
        self.remember_dock_position(panel, side);
        Ok(())
    }

    pub(super) fn panel_content_mut(
        &mut self,
        id: PanelId,
    ) -> Option<(&PanelMetadata, &mut dyn PanelContent)> {
        let entry = self.panels.get_mut(&id)?;
        Some((&entry.metadata, entry.content.as_mut()))
    }

    pub(super) fn area(&self, side: DockSide) -> &DockAreaState {
        match side {
            DockSide::Left => &self.left,
            DockSide::Right => &self.right,
            DockSide::Bottom => &self.bottom,
        }
    }

    pub(super) fn area_mut(&mut self, side: DockSide) -> &mut DockAreaState {
        match side {
            DockSide::Left => &mut self.left,
            DockSide::Right => &mut self.right,
            DockSide::Bottom => &mut self.bottom,
        }
    }

    fn add_to_dock(&mut self, id: PanelId, placement: PanelPlacement) {
        match placement {
            PanelPlacement::Floating => {}
            PanelPlacement::DockedLeft => {
                if self.left.panel_ids.is_empty() {
                    self.left.extent = LEFT_DOCK_DEFAULT_EXTENT;
                }
                self.left.add(id);
                self.remember_dock_position(id, DockSide::Left);
            }
            PanelPlacement::DockedRight => {
                self.right.add(id);
                self.remember_dock_position(id, DockSide::Right);
            }
            PanelPlacement::DockedBottom => {
                self.bottom.add(id);
                self.remember_dock_position(id, DockSide::Bottom);
            }
            PanelPlacement::Native => {}
        }
    }

    fn remember_dock_position(&mut self, id: PanelId, side: DockSide) {
        let order = self
            .area(side)
            .panel_ids
            .iter()
            .position(|candidate| *candidate == id);
        if let Some(order) = order {
            self.last_dock_positions
                .insert(id, DockPosition { side, order });
        }
    }

    fn remove_from_docks(&mut self, id: PanelId) {
        self.left.remove(id);
        self.right.remove(id);
        self.bottom.remove(id);
    }
}

impl PanelPlacement {
    pub const fn dock_side(self) -> Option<DockSide> {
        match self {
            Self::Floating => None,
            Self::DockedLeft => Some(DockSide::Left),
            Self::DockedRight => Some(DockSide::Right),
            Self::DockedBottom => Some(DockSide::Bottom),
            Self::Native => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_dock_placement_maps_to_left_side() {
        let mut manager = DockManager::demo();
        manager
            .transition_panel(PanelId::new(3), PanelPlacement::DockedLeft)
            .unwrap();

        assert_eq!(
            manager.placement(PanelId::new(3)),
            Some(PanelPlacement::DockedLeft)
        );
        assert_eq!(manager.dock_order(DockSide::Left), &[PanelId::new(3)]);
        assert_eq!(PanelPlacement::DockedLeft.dock_side(), Some(DockSide::Left));
    }
}
