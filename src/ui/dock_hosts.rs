//! Shared per-frame state cells for the dock panels (Toolbox, Layers).
//!
//! The App writes a typed snapshot into `view` before rendering the dock and drains
//! `events` after rendering; the panels are pure views that never mutate the model.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::core::color::Color;
use crate::core::model::{BlendMode, LayerId};
use crate::core::transform::TransformAlgorithm;
use crate::input::{FieldierChild, FillSettings, Tool, WandSettings};
use crate::ui::dock_color_palette_panel::PalettePanelEvent;
use crate::ui::layers::LayerPanelEvent;
use crate::ui::project::DrawSettings;
use crate::ui::toolbar::ToolbarEvent;

/// One rendered layer-list row (flattened DFS pre-order).
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRow {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub is_group: bool,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
}

/// Snapshot of the layer tree for one frame. `rows` is DFS pre-order with collapsed
/// subtrees omitted; `active` is `None` only before the first snapshot is written.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LayerView {
    pub rows: Vec<LayerRow>,
    pub active: Option<LayerId>,
    pub merge_enabled: bool,
    pub delete_enabled: bool,
}

/// Snapshot of the Tile tool's sticky placement transform for one frame,
/// mirrored from the App's [`TilePlacerTransform`](crate::ui::TilePlacerTransform)
/// so the Tool Property panel stays a pure view.
///
/// `rotation` is the DEGREE form of the App's quarter-turn counter: always a
/// multiple of 90 and always normalized into `0..360`, so the panel only ever
/// shows 0 / 90 / 180 / 270.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TilePlacerView {
    /// Clockwise rotation in degrees; a normalized multiple of 90.
    pub rotation: i32,
    /// Horizontal flip.
    pub flip_x: bool,
    /// Vertical flip.
    pub flip_y: bool,
    /// Tile eraser: when true the stamped tile erases instead of placing.
    pub tile_eraser: bool,
}

/// Snapshot of the toolbox state for one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolboxView {
    pub tool: Tool,
    /// The ACTIVE tool's draw settings (the Eraser's own bundle, or the Pen's).
    pub draw: DrawSettings,
    /// The Fieldier's active child (Rectangle / Wand / Lasso).
    pub child: FieldierChild,
    /// The Fieldier wand's stored settings.
    pub wand: WandSettings,
    /// Whether the wand's flood fill is contiguous THIS frame: the stored
    /// [`WandSettings::contiguous`] with Alt momentarily dropping contiguity
    /// (`false`) so the panel can show it read-only.
    pub wand_contiguous_effective: bool,
    /// The Fill (bucket) tool's stored flood-fill settings.
    pub fill: FillSettings,
    /// Whether a selection-transform session is live. When true the Tool
    /// Property panel shows the "Transform" algorithm selector instead of the
    /// active tool's properties.
    pub transform_active: bool,
    /// The session's stored free-angle rotation algorithm, mirrored for the
    /// selector so the panel stays a pure view.
    pub transform_algorithm: TransformAlgorithm,
    /// The Tile tool's sticky placement transform, mirrored for the rotation /
    /// flip controls.
    pub tile_placer: TilePlacerView,
}

impl Default for ToolboxView {
    fn default() -> Self {
        let wand = WandSettings::default();
        Self {
            tool: Tool::Pencil,
            draw: DrawSettings::default(),
            child: FieldierChild::default(),
            wand,
            wand_contiguous_effective: wand.contiguous,
            fill: FillSettings::default(),
            transform_active: false,
            transform_algorithm: TransformAlgorithm::default(),
            tile_placer: TilePlacerView::default(),
        }
    }
}

/// Shared cells for the Layers panel.
#[derive(Clone, Default)]
pub struct LayerPanelHost {
    pub view: Rc<RefCell<LayerView>>,
    pub events: Rc<RefCell<Vec<LayerPanelEvent>>>,
    /// Which layer's floating settings popup is open, if any.
    pub open_settings: Rc<RefCell<Option<LayerId>>>,
    /// Group rows the user has expanded (ignored for leaves).
    pub expanded_groups: Rc<RefCell<HashSet<LayerId>>>,
    /// In-progress rename text for the open settings popup, kept across
    /// frames so typed characters survive until the field commits.
    pub rename_buffer: Rc<RefCell<String>>,
}

/// Shared cells for the Toolbox panel.
#[derive(Clone, Default)]
pub struct ToolboxHost {
    pub view: Rc<RefCell<ToolboxView>>,
    pub events: Rc<RefCell<Vec<ToolbarEvent>>>,
}

/// Snapshot of the color picker state for one frame.
///
/// The App writes `color` from the active session every frame; `old_color` is
/// the color the panel was opened with (the "Old" preview) and only changes
/// when the panel is (re)opened; `new_color` is the last working color an edit
/// produced (the "New" preview and the target a preview click restores);
/// `secondary_color` mirrors the session's secondary color for the FG/BG
/// swatch pair; `open` mirrors whether the panel is registered in the dock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorPickerView {
    pub color: Color,
    pub old_color: Color,
    pub new_color: Color,
    pub secondary_color: Color,
    pub open: bool,
}

impl Default for ColorPickerView {
    fn default() -> Self {
        Self {
            color: Color::BLACK,
            old_color: Color::BLACK,
            new_color: Color::BLACK,
            secondary_color: Color::WHITE,
            open: false,
        }
    }
}

/// Shared cells for the Color Picker panel.
#[derive(Clone, Default)]
pub struct ColorPickerHost {
    pub view: Rc<RefCell<ColorPickerView>>,
    pub events: Rc<RefCell<Vec<ToolbarEvent>>>,
}

/// Snapshot of the Palette panel state for one frame.
///
/// `entries` is the ACTIVE palette's color list; `primary`/`secondary` are the
/// two drawing colors the panel marks among the swatches.
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteView {
    pub entries: Vec<Color>,
    pub primary: Color,
    pub secondary: Color,
}

impl Default for PaletteView {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            primary: Color::BLACK,
            secondary: Color::WHITE,
        }
    }
}

/// Shared cells for the Palette panel.
#[derive(Clone, Default)]
pub struct PalettePanelHost {
    pub view: Rc<RefCell<PaletteView>>,
    pub events: Rc<RefCell<Vec<PalettePanelEvent>>>,
    /// The palette entry the last swatch click selected (the Remove target).
    pub selected: Rc<RefCell<Option<usize>>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_panel_host_defaults_are_empty() {
        let host = LayerPanelHost::default();
        assert!(host.view.borrow().rows.is_empty());
        assert!(host.view.borrow().active.is_none());
        assert!(!host.view.borrow().merge_enabled);
        assert!(!host.view.borrow().delete_enabled);
        assert!(host.events.borrow().is_empty());
        assert!(host.open_settings.borrow().is_none());
        assert!(host.expanded_groups.borrow().is_empty());
        assert!(host.rename_buffer.borrow().is_empty());
    }

    #[test]
    fn layer_host_snapshot_round_trips() {
        let host = LayerPanelHost::default();
        let group = LayerId::new(1);
        let leaf = LayerId::new(2);
        let snapshot = LayerView {
            rows: vec![
                LayerRow {
                    id: group,
                    name: "Group".to_string(),
                    visible: true,
                    opacity: 1.0,
                    blend: BlendMode::Normal,
                    is_group: true,
                    depth: 0,
                    has_children: true,
                    expanded: true,
                },
                LayerRow {
                    id: leaf,
                    name: "Leaf".to_string(),
                    visible: false,
                    opacity: 0.5,
                    blend: BlendMode::Multiply,
                    is_group: false,
                    depth: 1,
                    has_children: false,
                    expanded: false,
                },
            ],
            active: Some(leaf),
            merge_enabled: true,
            delete_enabled: true,
        };
        *host.view.borrow_mut() = snapshot.clone();
        assert_eq!(*host.view.borrow(), snapshot);

        host.events.borrow_mut().push(LayerPanelEvent::Select(leaf));
        host.events
            .borrow_mut()
            .push(LayerPanelEvent::ToggleVisible(group));
        let drained: Vec<LayerPanelEvent> = host.events.borrow_mut().drain(..).collect();
        assert_eq!(
            drained,
            vec![
                LayerPanelEvent::Select(leaf),
                LayerPanelEvent::ToggleVisible(group)
            ]
        );
        assert!(host.events.borrow().is_empty());
    }

    #[test]
    fn toolbox_host_snapshot_round_trips() {
        let host = ToolboxHost::default();
        assert_eq!(host.view.borrow().tool, Tool::Pencil);
        assert_eq!(host.view.borrow().draw, DrawSettings::default());

        let draw = DrawSettings {
            size: 12,
            shape: crate::core::brush::BrushShape::Round,
            scatter: 6,
            scatter_shape: crate::core::brush::ScatterShape::Diamond,
            tail: -4,
        };
        *host.view.borrow_mut() = ToolboxView {
            tool: Tool::Fill,
            draw,
            ..ToolboxView::default()
        };
        assert_eq!(host.view.borrow().tool, Tool::Fill);
        assert_eq!(host.view.borrow().draw, draw);

        host.events
            .borrow_mut()
            .push(ToolbarEvent::ToolSelected(Tool::Eraser));
        let drained: Vec<ToolbarEvent> = host.events.borrow_mut().drain(..).collect();
        assert_eq!(drained, vec![ToolbarEvent::ToolSelected(Tool::Eraser)]);
        assert!(host.events.borrow().is_empty());
    }

    #[test]
    fn palette_host_defaults_are_empty() {
        let host = PalettePanelHost::default();
        assert!(host.view.borrow().entries.is_empty());
        assert_eq!(host.view.borrow().primary, Color::BLACK);
        assert_eq!(host.view.borrow().secondary, Color::WHITE);
        assert!(host.events.borrow().is_empty());
        assert_eq!(*host.selected.borrow(), None);
    }

    #[test]
    fn palette_host_snapshot_and_events_round_trip() {
        let host = PalettePanelHost::default();
        let snapshot = PaletteView {
            entries: vec![Color::rgb(1, 2, 3), Color::rgb(4, 5, 6)],
            primary: Color::rgb(4, 5, 6),
            secondary: Color::WHITE,
        };
        *host.view.borrow_mut() = snapshot.clone();
        assert_eq!(*host.view.borrow(), snapshot);

        *host.selected.borrow_mut() = Some(1);
        host.events.borrow_mut().push(PalettePanelEvent::Add);
        host.events.borrow_mut().push(PalettePanelEvent::Remove(1));
        host.events
            .borrow_mut()
            .push(PalettePanelEvent::Primary(snapshot.primary));
        host.events
            .borrow_mut()
            .push(PalettePanelEvent::Secondary(snapshot.secondary));
        let drained: Vec<PalettePanelEvent> = host.events.borrow_mut().drain(..).collect();
        assert_eq!(
            drained,
            vec![
                PalettePanelEvent::Add,
                PalettePanelEvent::Remove(1),
                PalettePanelEvent::Primary(snapshot.primary),
                PalettePanelEvent::Secondary(snapshot.secondary),
            ]
        );
        assert_eq!(*host.selected.borrow(), Some(1));
    }

    #[test]
    fn expanded_groups_is_keyed_by_layer_id() {
        let host = LayerPanelHost::default();
        let group_a = LayerId::new(10);
        let group_b = LayerId::new(20);
        host.expanded_groups.borrow_mut().insert(group_a);
        host.expanded_groups.borrow_mut().insert(group_b);
        assert!(host.expanded_groups.borrow().contains(&group_a));
        assert!(host.expanded_groups.borrow().contains(&group_b));
        assert_eq!(host.expanded_groups.borrow().len(), 2);
        host.expanded_groups.borrow_mut().remove(&group_a);
        assert!(!host.expanded_groups.borrow().contains(&group_a));
        assert!(host.expanded_groups.borrow().contains(&group_b));
    }
}
