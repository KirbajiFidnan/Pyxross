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
        let used: f32 = extents.iter().map(|(_, extent)| *extent).sum();
        let free = (available - used).max(0.0);

        let mut updates: Vec<(PanelId, f32)> = Vec::new();
        if delta >= 0.0 {
            // Domino resize downstream: sum the shrinkable slack across all panels
            // after `index` so that once `index + 1` hits its minimum, `index + 2`..
            // continue shrinking.
            let mut downstream_slack = 0.0f32;
            for (cid, cext) in extents.iter().skip(index + 1) {
                let cmin = panel_minimum(cid, &self.panels, side).unwrap_or(0.0);
                downstream_slack += (*cext - cmin).max(0.0);
            }
            let maximum_delta = free + downstream_slack;
            let actual_delta = delta.clamp(0.0, maximum_delta);
            updates.push((id, *current + actual_delta));
            if actual_delta > free {
                let mut remaining = actual_delta - free;
                for (cid, cext) in extents.iter().skip(index + 1) {
                    if remaining <= 0.0 {
                        break;
                    }
                    let cmin = panel_minimum(cid, &self.panels, side).unwrap_or(0.0);
                    let slack = (*cext - cmin).max(0.0);
                    let take = remaining.min(slack);
                    updates.push((*cid, *cext - take));
                    remaining -= take;
                }
            }
        } else {
            // Domino resize upstream: sum the shrinkable slack across `0..=index`
            // in reverse order so that once `index` hits its minimum, `index - 1`..
            // continue shrinking while `index + 1` grows.
            let mut upstream_slack = 0.0f32;
            for (cid, cext) in extents[..=index].iter().rev() {
                let cmin = panel_minimum(cid, &self.panels, side).unwrap_or(0.0);
                upstream_slack += (*cext - cmin).max(0.0);
            }
            let shrink_amount = (-delta).min(upstream_slack);
            if shrink_amount > 0.0 {
                let mut remaining = shrink_amount;
                for (cid, cext) in extents[..=index].iter().rev() {
                    if remaining <= 0.0 {
                        break;
                    }
                    let cmin = panel_minimum(cid, &self.panels, side).unwrap_or(0.0);
                    let slack = (*cext - cmin).max(0.0);
                    let take = remaining.min(slack);
                    updates.push((*cid, *cext - take));
                    remaining -= take;
                }
                let (_, next_current) = extents[index + 1];
                updates.push((next_id, next_current + shrink_amount));
            }
        }
        let area = self.area_mut(side);
        for (uid, uext) in updates {
            area.panel_extents.insert(uid, uext);
        }
    }

    pub fn dock_extent(&self, side: DockSide) -> f32 {
        self.area(side).extent
    }

    /// Sets the preferred extent (height for Left/Right, width for Bottom) of a
    /// docked panel when it is registered in `side`.
    pub fn set_panel_extent(&mut self, id: PanelId, extent: f32) {
        let Some(side) = self
            .panels
            .get(&id)
            .and_then(|entry| entry.placement.dock_side())
        else {
            return;
        };
        let min_extent = panel_minimum(&id, &self.panels, side).unwrap_or(0.0);
        let area = self.area_mut(side);
        if area.panel_ids.contains(&id) {
            area.panel_extents.insert(id, extent.max(min_extent));
        }
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

    /// The BODY rects of every docked panel for `viewport`: each panel's full
    /// rect minus its top header band. Computed with the same geometry as
    /// [`Self::panel_rects`]; empty when nothing is docked.
    pub fn docked_panel_body_rects(&self, viewport: Rect) -> Vec<Rect> {
        let mut rects = Vec::new();
        for side in [DockSide::Left, DockSide::Right, DockSide::Bottom] {
            let dock = self.dock_rect(side, viewport);
            for (_id, panel_rect) in self.panel_rects(side, dock) {
                // The header band is the top `HEADER_HEIGHT` strip; the body is
                // everything below it (mirrors `panel_nest_body_rect`).
                let body = Rect::from_min_max(
                    pos2(
                        panel_rect.left(),
                        panel_rect.top() + super::view::HEADER_HEIGHT,
                    ),
                    panel_rect.max,
                );
                if body.width() > 0.0 && body.height() > 0.0 {
                    rects.push(body);
                }
            }
        }
        rects
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
        let manager = DockManager::demo_with_dummies();
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
        let manager = DockManager::demo_with_dummies();
        let dock = Rect::from_min_max(pos2(0.0, 300.0), pos2(640.0, 480.0));
        let panels = manager.panel_rects(DockSide::Bottom, dock);

        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].1.left(), dock.left());
        assert_eq!(panels[0].1.right(), dock.right());
    }

    #[test]
    fn left_dock_panels_stack_downward_and_fill_width() {
        let mut manager = DockManager::demo_with_dummies();
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
        let mut manager = DockManager::demo_with_dummies();
        manager
            .transition_panel(PanelId::new(3), PanelPlacement::DockedLeft)
            .unwrap();
        let viewport = vec2(640.0, 480.0);
        let before = manager.dock_extent(DockSide::Left);
        manager.resize_dock_area(DockSide::Left, 40.0, viewport);
        assert_eq!(manager.dock_extent(DockSide::Left), before + 40.0);
    }

    #[test]
    fn domino_resize_shrinks_downstream_panel_when_neighbor_is_at_minimum() {
        let mut manager = DockManager::demo_with_dummies();
        let viewport = vec2(800.0, 480.0);
        // Right dock has panels [1, 2, 4]. Force panel 2 to its minimum (100px)
        // and grow panel 1 by 80px -> panel 4 must shrink to accommodate.
        let dock = Rect::from_min_max(pos2(0.0, 0.0), pos2(240.0, 480.0));
        manager.set_panel_extent(PanelId::new(1), 120.0);
        manager.set_panel_extent(PanelId::new(2), 0.0);
        let p4_before = manager
            .panel_rects(DockSide::Right, dock)
            .into_iter()
            .find(|(id, _)| *id == PanelId::new(4))
            .unwrap()
            .1
            .height();
        manager.resize_dock_panel(PanelId::new(1), 80.0, viewport);
        let rects_after = manager.panel_rects(DockSide::Right, dock);
        let p1_after = rects_after
            .iter()
            .find(|(id, _)| *id == PanelId::new(1))
            .unwrap()
            .1
            .height();
        let p4_after = rects_after
            .iter()
            .find(|(id, _)| *id == PanelId::new(4))
            .unwrap()
            .1
            .height();
        assert!(p1_after > 120.0, "panel 1 should grow via domino resize");
        assert!(p4_after < p4_before, "panel 4 should shrink via domino");
    }

    #[test]
    fn domino_resize_shrinks_upstream_panel_when_neighbor_is_at_minimum() {
        let mut manager = DockManager::demo_with_dummies();
        let viewport = vec2(800.0, 480.0);
        // Right dock has panels [1, 2, 4]. Force panel 2 to its minimum (100px)
        // and drag divider between 2 and 4 upward (negative delta on panel 2)
        // -> panel 4 grows while panel 1 shrinks upstream!
        let dock = Rect::from_min_max(pos2(0.0, 0.0), pos2(240.0, 480.0));
        manager.set_panel_extent(PanelId::new(1), 220.0);
        manager.set_panel_extent(PanelId::new(2), 0.0);
        let rects_before = manager.panel_rects(DockSide::Right, dock);
        let p1_before = rects_before
            .iter()
            .find(|(id, _)| *id == PanelId::new(1))
            .unwrap()
            .1
            .height();
        let p4_before = rects_before
            .iter()
            .find(|(id, _)| *id == PanelId::new(4))
            .unwrap()
            .1
            .height();
        manager.resize_dock_panel(PanelId::new(2), -60.0, viewport);
        let rects_after = manager.panel_rects(DockSide::Right, dock);
        let p1_after = rects_after
            .iter()
            .find(|(id, _)| *id == PanelId::new(1))
            .unwrap()
            .1
            .height();
        let p4_after = rects_after
            .iter()
            .find(|(id, _)| *id == PanelId::new(4))
            .unwrap()
            .1
            .height();
        assert!(p4_after > p4_before, "panel 4 should grow via reverse domino resize");
        assert!(p1_after < p1_before, "panel 1 should shrink upstream via domino");
    }
}
