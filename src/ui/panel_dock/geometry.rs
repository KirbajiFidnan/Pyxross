use std::collections::BTreeMap;

use egui::{pos2, vec2, Pos2, Rect, Vec2};

use super::state::{DockManager, DockSide, PanelId};

const DEFAULT_PANEL_EXTENT: f32 = 120.0;
const DRAG_OUT_THRESHOLD: f32 = 12.0;
/// Extent a non-empty dock keeps when collapsed: only its frame strip stays
/// visible. An empty dock collapses fully to zero.
pub(super) const DOCK_FRAME_EXTENT: f32 = 8.0;
/// Extent the left dock takes once it holds its first panel; an empty left
/// dock collapses to zero so the bottom dock spans the full width between the
/// left and right docks.
pub(super) const LEFT_DOCK_DEFAULT_EXTENT: f32 = 200.0;

pub(super) struct DockAreaState {
    pub(super) extent: f32,
    pub(super) panel_ids: Vec<PanelId>,
    pub(super) panel_extents: BTreeMap<PanelId, f32>,
}

impl DockAreaState {
    pub(super) fn new(side: DockSide) -> Self {
        let extent = match side {
            DockSide::Left => 0.0,
            DockSide::Right => 240.0,
            DockSide::Bottom => 180.0,
        };
        Self {
            extent,
            panel_ids: Vec::new(),
            panel_extents: BTreeMap::new(),
        }
    }

    pub(super) fn add(&mut self, id: PanelId) {
        if !self.panel_ids.contains(&id) {
            self.panel_ids.push(id);
            self.panel_extents.insert(id, DEFAULT_PANEL_EXTENT);
        }
    }

    pub(super) fn remove(&mut self, id: PanelId) {
        self.panel_ids.retain(|candidate| *candidate != id);
        self.panel_extents.remove(&id);
    }
}

impl DockManager {
    pub fn clamp_floating(&mut self, id: PanelId, bounds: Rect) {
        let Some(entry) = self.panels.get_mut(&id) else {
            return;
        };
        let minimum = entry.metadata.min_size;
        let size = vec2(
            entry
                .floating_rect
                .width()
                .max(minimum.x)
                .min(bounds.width()),
            entry
                .floating_rect
                .height()
                .max(minimum.y)
                .min(bounds.height()),
        );
        let max_pos = bounds.max - size;
        let min_pos = pos2(
            entry
                .floating_rect
                .min
                .x
                .clamp(bounds.min.x, max_pos.x.max(bounds.min.x)),
            entry
                .floating_rect
                .min
                .y
                .clamp(bounds.min.y, max_pos.y.max(bounds.min.y)),
        );
        entry.floating_rect = Rect::from_min_size(min_pos, size);
    }

    pub fn resize_dock_area(&mut self, side: DockSide, delta: f32, viewport: Vec2) {
        let minimum = if self.area(side).panel_ids.is_empty() {
            0.0
        } else {
            DOCK_FRAME_EXTENT
        };
        let maximum = match side {
            DockSide::Left => viewport.x - self.area(DockSide::Right).extent,
            DockSide::Right => viewport.x - self.area(DockSide::Left).extent,
            DockSide::Bottom => viewport.y,
        };
        let area = self.area_mut(side);
        area.extent = (area.extent + delta).clamp(minimum, maximum.max(minimum));
    }

    pub fn resize_dock_panel(&mut self, id: PanelId, delta: f32, viewport: Vec2) {
        let Some(side) = self
            .placement(id)
            .and_then(|placement| placement.dock_side())
        else {
            return;
        };
        let available = match side {
            DockSide::Left => viewport.y,
            DockSide::Right => viewport.y,
            DockSide::Bottom => viewport.x,
        };
        let Some(index) = self
            .area(side)
            .panel_ids
            .iter()
            .position(|candidate| *candidate == id)
        else {
            return;
        };
        let Some(next_id) = self.area(side).panel_ids.get(index + 1).copied() else {
            return;
        };
        let extents = self.resolved_panel_extents(side, available);
        let Some((_, current)) = extents.iter().find(|(candidate, _)| *candidate == id) else {
            return;
        };
        let Some((_, next_current)) = extents.iter().find(|(candidate, _)| *candidate == next_id)
        else {
            return;
        };
        let minimum = panel_minimum(&id, &self.panels, side).unwrap_or(0.0);
        let next_minimum = panel_minimum(&next_id, &self.panels, side).unwrap_or(0.0);
        let used: f32 = extents.iter().map(|(_, extent)| *extent).sum();
        let free = (available - used).max(0.0);
        let maximum_delta = free + (*next_current - next_minimum);
        // On a thin dock resolved extents can sit below their minimums, so the
        // lower bound may exceed `maximum_delta`; clamp it into range.
        let lower = (minimum - *current).min(maximum_delta);
        let actual_delta = delta.clamp(lower, maximum_delta);
        let next_delta = if actual_delta > free {
            -(actual_delta - free)
        } else if actual_delta < 0.0 {
            -actual_delta
        } else {
            0.0
        };
        let area = self.area_mut(side);
        area.panel_extents.insert(id, *current + actual_delta);
        area.panel_extents
            .insert(next_id, *next_current + next_delta);
    }

    pub fn dock_extent(&self, side: DockSide) -> f32 {
        self.area(side).extent
    }

    pub fn dock_order(&self, side: DockSide) -> &[PanelId] {
        &self.area(side).panel_ids
    }

    pub fn dock_panel_size(&self, side: DockSide, id: PanelId) -> Option<Vec2> {
        let area = self.area(side);
        let panel_extent = area.panel_extents.get(&id).copied()?;
        Some(match side {
            DockSide::Left => vec2(area.extent, panel_extent),
            DockSide::Right => vec2(area.extent, panel_extent),
            DockSide::Bottom => vec2(panel_extent, area.extent),
        })
    }

    pub fn panel_rects(&self, side: DockSide, viewport: Rect) -> Vec<(PanelId, Rect)> {
        let area = self.area(side);
        let extent = area.extent.max(0.0);
        let mut cursor = match side {
            DockSide::Left => viewport.top(),
            DockSide::Right => viewport.top(),
            DockSide::Bottom => viewport.left(),
        };
        self.resolved_panel_extents(
            side,
            match side {
                DockSide::Left => viewport.height(),
                DockSide::Right => viewport.height(),
                DockSide::Bottom => viewport.width(),
            },
        )
        .into_iter()
        .map(|(id, panel_extent)| {
            let panel_extent = panel_extent.max(0.0);
            let rect = match side {
                DockSide::Left => Rect::from_min_max(
                    pos2(viewport.left(), cursor),
                    pos2(viewport.left() + extent, cursor + panel_extent),
                ),
                DockSide::Right => Rect::from_min_max(
                    pos2(viewport.right() - extent, cursor),
                    pos2(viewport.right(), cursor + panel_extent),
                ),
                DockSide::Bottom => Rect::from_min_max(
                    pos2(cursor, viewport.top()),
                    pos2(cursor + panel_extent, viewport.bottom()),
                ),
            };
            cursor = match side {
                DockSide::Left => rect.bottom(),
                DockSide::Right => rect.bottom(),
                DockSide::Bottom => rect.right(),
            };
            (id, rect)
        })
        .collect()
    }

    fn resolved_panel_extents(&self, side: DockSide, available: f32) -> Vec<(PanelId, f32)> {
        let area = self.area(side);
        let desired: Vec<(PanelId, f32, f32)> = area
            .panel_ids
            .iter()
            .map(|id| {
                let extent = area
                    .panel_extents
                    .get(id)
                    .copied()
                    .unwrap_or(DEFAULT_PANEL_EXTENT);
                let minimum = panel_minimum(id, &self.panels, side).unwrap_or(0.0);
                (*id, extent.max(minimum), minimum)
            })
            .collect();
        let desired_total: f32 = desired.iter().map(|(_, extent, _)| *extent).sum();
        if desired_total <= available || desired.is_empty() {
            let mut resolved: Vec<(PanelId, f32)> = desired
                .into_iter()
                .map(|(id, extent, _)| (id, extent))
                .collect();
            if let Some((_, last_extent)) = resolved.last_mut() {
                *last_extent += (available - desired_total).max(0.0);
            }
            return resolved;
        }

        let minimum_total: f32 = desired.iter().map(|(_, _, minimum)| *minimum).sum();
        if minimum_total >= available {
            let extent = available / desired.len() as f32;
            return desired.into_iter().map(|(id, _, _)| (id, extent)).collect();
        }

        let shrinkable = desired_total - minimum_total;
        let ratio = (available - minimum_total) / shrinkable;
        desired
            .into_iter()
            .map(|(id, extent, minimum)| (id, minimum + (extent - minimum) * ratio))
            .collect()
    }
}

fn panel_minimum(
    id: &PanelId,
    panels: &BTreeMap<PanelId, super::state::PanelEntry>,
    side: DockSide,
) -> Option<f32> {
    let minimum = panels.get(id)?.metadata.min_size;
    Some(match side {
        DockSide::Left => minimum.y,
        DockSide::Right => minimum.y,
        DockSide::Bottom => minimum.x,
    })
}

pub fn drag_outside_threshold(start: Pos2, current: Pos2) -> bool {
    start.distance(current) >= DRAG_OUT_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::super::state::PanelPlacement;
    use super::*;

    #[test]
    fn right_dock_panels_share_width_and_stack_downward() {
        let manager = DockManager::demo();
        let dock = Rect::from_min_max(pos2(400.0, 0.0), pos2(640.0, 480.0));
        let panels = manager.panel_rects(DockSide::Right, dock);

        assert_eq!(panels.len(), 3);
        for (_, rect) in &panels {
            assert_eq!(rect.left(), dock.left());
            assert_eq!(rect.right(), dock.right());
        }
        assert_eq!(panels[1].1.top(), panels[0].1.bottom());
        assert_eq!(panels[2].1.top(), panels[1].1.bottom());
        assert_eq!(panels[2].1.bottom(), dock.bottom());
    }

    #[test]
    fn last_bottom_dock_panel_fills_remaining_width() {
        let manager = DockManager::demo();
        let dock = Rect::from_min_max(pos2(0.0, 300.0), pos2(640.0, 480.0));
        let panels = manager.panel_rects(DockSide::Bottom, dock);

        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].1.left(), dock.left());
        assert_eq!(panels[0].1.right(), dock.right());
    }

    #[test]
    fn left_dock_panels_stack_downward_and_fill_width() {
        let mut manager = DockManager::demo();
        manager
            .transition_panel(PanelId::new(3), PanelPlacement::DockedLeft)
            .unwrap();
        manager
            .transition_panel(PanelId::new(4), PanelPlacement::DockedLeft)
            .unwrap();
        let dock = Rect::from_min_max(pos2(0.0, 0.0), pos2(200.0, 480.0));
        let panels = manager.panel_rects(DockSide::Left, dock);

        assert_eq!(panels.len(), 2);
        for (_, rect) in &panels {
            assert_eq!(rect.left(), dock.left());
            assert_eq!(rect.right(), dock.right());
        }
        assert_eq!(panels[1].1.top(), panels[0].1.bottom());
        assert_eq!(panels[1].1.bottom(), dock.bottom());
    }

    #[test]
    fn left_dock_resize_uses_the_horizontal_axis() {
        let mut manager = DockManager::demo();
        manager
            .transition_panel(PanelId::new(3), PanelPlacement::DockedLeft)
            .unwrap();
        let viewport = vec2(640.0, 480.0);
        let before = manager.dock_extent(DockSide::Left);
        manager.resize_dock_area(DockSide::Left, 40.0, viewport);
        assert_eq!(manager.dock_extent(DockSide::Left), before + 40.0);
    }
}
