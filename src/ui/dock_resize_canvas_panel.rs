//! Floating "resize canvas" dock panel (id 206).
//!
//! Registered as a `PanelPlacement::Floating` dock panel with
//! `can_pop_out == false`: it floats, cannot be docked, and closes from its
//! header's Close action (the dock renders Close-only for a floating-only
//! panel). `resizable == false` keeps the window fixed-size and draggable only
//! by its header.
//!
//! A static nest holds two side-by-side sections — resize the single region and
//! resize the region counts — each with a width/height row, a 3×3 pivot
//! selector and an Apply action. The panel is a pure view: the in-progress
//! values live in the host's [`ResizeCanvasView`] cell so they survive across
//! frames, and it never touches the model — Apply only pushes a
//! [`ResizeCanvasEvent`] for the App shell to apply.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::core::canvas_resize::Pivot;
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the resize canvas floating panel.
pub const RESIZE_CANVAS_PANEL_ID: u64 = 206;

/// Largest single-region side (pixels) the region inputs accept.
const MAX_REGION_SIDE: u32 = 4096;

/// Largest region count along either axis the count inputs accept.
const MAX_REGION_COUNT: u32 = 256;

/// Fixed minimum size of the (non-resizable) floating window.
const RESIZE_CANVAS_MIN_SIZE: egui::Vec2 = vec2(260.0, 180.0);

/// Side of one pivot-grid cell, in points.
const PIVOT_CELL: f32 = 18.0;

/// Gap between pivot-grid cells, in points.
const PIVOT_GAP: f32 = 2.0;

/// Corner radius of a pivot-grid cell.
const PIVOT_CORNER: f32 = 2.0;

/// The in-progress resize-canvas form values.
///
/// The host owns this cell; the panel edits it in place (it is the dialog's
/// draft state, not model data) and reads it back when either Apply is pressed.
///
/// `region_w`/`region_h` are the per-axis TILE size (mapped onto
/// `TileSize { w, h }` by the App shell); tiles need not be square.
#[derive(Clone, Debug, PartialEq)]
pub struct ResizeCanvasView {
    pub region_w: u32,
    pub region_h: u32,
    pub region_pivot: Pivot,
    pub resample: bool,
    pub count_w: u32,
    pub count_h: u32,
    pub count_pivot: Pivot,
}

impl Default for ResizeCanvasView {
    fn default() -> Self {
        Self {
            region_w: 16,
            region_h: 16,
            region_pivot: Pivot::Center,
            resample: false,
            count_w: 4,
            count_h: 4,
            count_pivot: Pivot::Center,
        }
    }
}

/// A resize-canvas gesture emitted by the panel.
///
/// The App shell owns canvas resizing; this pure view only ever pushes these
/// events into the host cells.
#[derive(Clone, Debug, PartialEq)]
pub enum ResizeCanvasEvent {
    /// Resize each region to `w`×`h` (the per-axis TILE size; `TileSize { w,
    /// h }`), anchored at `pivot`; `resample` selects resampling versus a hard
    /// copy.
    ResizeRegions {
        w: u32,
        h: u32,
        pivot: Pivot,
        resample: bool,
    },
    /// Resize the region grid to `w`×`h` cells, anchored at `pivot`.
    ResizeCounts { w: u32, h: u32, pivot: Pivot },
}

/// Shared cells for the resize-canvas panel.
#[derive(Clone, Default)]
pub struct ResizeCanvasHost {
    pub view: Rc<RefCell<ResizeCanvasView>>,
    pub events: Rc<RefCell<Vec<ResizeCanvasEvent>>>,
}

/// The [`Pivot`] a 3×3 grid cell index selects (row-major, `0` at top-left).
fn pivot_for_index(index: usize) -> Pivot {
    match index % 9 {
        0 => Pivot::TopLeft,
        1 => Pivot::TopCenter,
        2 => Pivot::TopRight,
        3 => Pivot::MidLeft,
        4 => Pivot::Center,
        5 => Pivot::MidRight,
        6 => Pivot::BottomLeft,
        7 => Pivot::BottomCenter,
        _ => Pivot::BottomRight,
    }
}

/// 3×3 pivot selector: the selected cell is drawn dark/filled, the rest are
/// empty. Clicking a different cell sets `*pivot` and returns `true`; clicking
/// the already-selected cell (or no cell) returns `false`.
///
/// Colors come from `ui.visuals()` (the active theme), never hard-coded.
pub fn pivot_grid(ui: &mut egui::Ui, pivot: &mut Pivot) -> bool {
    let side = PIVOT_CELL * 3.0 + PIVOT_GAP * 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());

    let (selected_fill, selected_stroke, empty_fill, empty_stroke) = {
        let visuals = ui.visuals();
        (
            visuals.selection.bg_fill,
            visuals.selection.stroke,
            visuals.widgets.inactive.bg_fill,
            visuals.widgets.inactive.bg_stroke,
        )
    };

    let mut changed = false;
    for index in 0..9 {
        let col = (index % 3) as f32;
        let row = (index / 3) as f32;
        let cell_rect = egui::Rect::from_min_size(
            egui::pos2(
                rect.min.x + col * (PIVOT_CELL + PIVOT_GAP),
                rect.min.y + row * (PIVOT_CELL + PIVOT_GAP),
            ),
            vec2(PIVOT_CELL, PIVOT_CELL),
        );
        let response = ui.interact(
            cell_rect,
            egui::Id::new(("resize-canvas-pivot", index)),
            egui::Sense::click(),
        );
        let target = pivot_for_index(index);
        let selected = *pivot == target;
        let (fill, stroke) = if selected {
            (selected_fill, selected_stroke)
        } else {
            (empty_fill, empty_stroke)
        };
        ui.painter().rect(
            cell_rect,
            PIVOT_CORNER,
            fill,
            stroke,
            egui::StrokeKind::Inside,
        );
        if response.clicked() && !selected {
            *pivot = target;
            changed = true;
        }
    }
    changed
}

/// Pushes the current region values as a [`ResizeCanvasEvent::ResizeRegions`].
///
/// The component calls this from the left Apply button; keeping it a free
/// function makes the event payload testable without driving egui.
fn push_resize_regions(host: &ResizeCanvasHost) {
    let view = host.view.borrow();
    host.events
        .borrow_mut()
        .push(ResizeCanvasEvent::ResizeRegions {
            w: view.region_w,
            h: view.region_h,
            pivot: view.region_pivot,
            resample: view.resample,
        });
}

/// Pushes the current count values as a [`ResizeCanvasEvent::ResizeCounts`].
///
/// The component calls this from the right Apply button; keeping it a free
/// function makes the event payload testable without driving egui.
fn push_resize_counts(host: &ResizeCanvasHost) {
    let view = host.view.borrow();
    host.events
        .borrow_mut()
        .push(ResizeCanvasEvent::ResizeCounts {
            w: view.count_w,
            h: view.count_h,
            pivot: view.count_pivot,
        });
}

/// Left section: resize the single region (size, pivot, resample) and Apply.
fn resize_regions_section(ui: &mut egui::Ui, host: &ResizeCanvasHost, chrome: &PanelChrome) {
    ui.strong("resize regions");

    // The locals are copied out of the host so the borrow is released before
    // the widgets run; on change the edited values are written back.
    let (mut region_w, mut region_h) = {
        let view = host.view.borrow();
        (view.region_w, view.region_h)
    };
    ui.horizontal(|ui| {
        ui.label("width");
        let w = ui.add(egui::DragValue::new(&mut region_w).range(1..=MAX_REGION_SIDE));
        ui.label("height");
        let h = ui.add(egui::DragValue::new(&mut region_h).range(1..=MAX_REGION_SIDE));
        if w.changed() || h.changed() {
            let mut view = host.view.borrow_mut();
            view.region_w = region_w.max(1);
            view.region_h = region_h.max(1);
        }
    });

    ui.strong("pivot point");
    let mut pivot = host.view.borrow().region_pivot;
    if pivot_grid(ui, &mut pivot) {
        host.view.borrow_mut().region_pivot = pivot;
    }

    let mut resample = host.view.borrow().resample;
    if ui.checkbox(&mut resample, "resample regions").changed() {
        host.view.borrow_mut().resample = resample;
    }

    // Apply emits the CURRENT view values for the App shell to apply; the panel
    // itself mutates no model.
    if chrome
        .button(ui, "Apply", ButtonStyle::plain())
        .on_hover_text("Resize each region")
        .clicked()
    {
        push_resize_regions(host);
    }
}

/// Right section: resize the region counts (size, pivot) and Apply.
fn resize_counts_section(ui: &mut egui::Ui, host: &ResizeCanvasHost, chrome: &PanelChrome) {
    ui.strong("resize numbers of regions");

    let (mut count_w, mut count_h) = {
        let view = host.view.borrow();
        (view.count_w, view.count_h)
    };
    ui.horizontal(|ui| {
        ui.label("width");
        let w = ui.add(egui::DragValue::new(&mut count_w).range(1..=MAX_REGION_COUNT));
        ui.label("height");
        let h = ui.add(egui::DragValue::new(&mut count_h).range(1..=MAX_REGION_COUNT));
        if w.changed() || h.changed() {
            let mut view = host.view.borrow_mut();
            view.count_w = count_w.max(1);
            view.count_h = count_h.max(1);
        }
    });

    ui.strong("pivot point");
    let mut pivot = host.view.borrow().count_pivot;
    if pivot_grid(ui, &mut pivot) {
        host.view.borrow_mut().count_pivot = pivot;
    }

    if chrome
        .button(ui, "Apply", ButtonStyle::plain())
        .on_hover_text("Resize the region grid")
        .clicked()
    {
        push_resize_counts(host);
    }
}

/// The resize form: the region section and the counts section side by side.
struct ResizeCanvasComponent {
    host: ResizeCanvasHost,
}

impl Component for ResizeCanvasComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        ui.columns(2, |columns| {
            resize_regions_section(&mut columns[0], &self.host, chrome);
            resize_counts_section(&mut columns[1], &self.host, chrome);
        });
    }
}

/// Panel content: the static nest holding the resize form.
struct ResizeCanvasPanelContent {
    nest: NestContent,
}

impl PanelContent for ResizeCanvasPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the resize-canvas form.
pub(crate) fn build_resize_canvas_nest(host: &ResizeCanvasHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ResizeCanvasComponent { host: host.clone() });
    nest
}

/// Dock spec for the resize-canvas floating panel (dock id 206).
///
/// **Float-only:** `can_pop_out == false`, so the header shows no Float/Dock
/// button and the panel is not dockable. `resizable == false` keeps the window
/// fixed-size and only draggable by its header.
pub fn resize_canvas_panel_spec(host: &ResizeCanvasHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(RESIZE_CANVAS_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new("resize canvas", false, RESIZE_CANVAS_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ResizeCanvasPanelContent {
            nest: build_resize_canvas_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = ResizeCanvasHost::default();
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), vec2(260.0, 180.0));
        let spec = resize_canvas_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), RESIZE_CANVAS_PANEL_ID);
        assert_eq!(spec.metadata.title, "resize canvas");
        assert!(
            !spec.metadata.can_pop_out,
            "the resize-canvas panel is float-only"
        );
        assert!(
            !spec.metadata.resizable,
            "the resize-canvas window is fixed-size"
        );
        assert_eq!(spec.placement, PanelPlacement::Floating);
        let nest = build_resize_canvas_nest(&host);
        assert_eq!(nest.mode(), NestMode::Static);
    }

    #[test]
    fn pivot_for_index_maps_all_nine_cells() {
        assert_eq!(pivot_for_index(0), Pivot::TopLeft);
        assert_eq!(pivot_for_index(1), Pivot::TopCenter);
        assert_eq!(pivot_for_index(2), Pivot::TopRight);
        assert_eq!(pivot_for_index(3), Pivot::MidLeft);
        assert_eq!(pivot_for_index(4), Pivot::Center);
        assert_eq!(pivot_for_index(5), Pivot::MidRight);
        assert_eq!(pivot_for_index(6), Pivot::BottomLeft);
        assert_eq!(pivot_for_index(7), Pivot::BottomCenter);
        assert_eq!(pivot_for_index(8), Pivot::BottomRight);
    }

    #[test]
    fn apply_events_carry_current_view_values() {
        let host = ResizeCanvasHost::default();
        *host.view.borrow_mut() = ResizeCanvasView {
            region_w: 8,
            region_h: 16,
            region_pivot: Pivot::BottomRight,
            resample: true,
            count_w: 3,
            count_h: 5,
            count_pivot: Pivot::TopLeft,
        };
        push_resize_regions(&host);
        push_resize_counts(&host);
        let events: Vec<ResizeCanvasEvent> = host.events.borrow_mut().drain(..).collect();
        assert_eq!(
            events,
            vec![
                ResizeCanvasEvent::ResizeRegions {
                    w: 8,
                    h: 16,
                    pivot: Pivot::BottomRight,
                    resample: true,
                },
                ResizeCanvasEvent::ResizeCounts {
                    w: 3,
                    h: 5,
                    pivot: Pivot::TopLeft,
                },
            ]
        );
    }

    #[test]
    fn view_defaults_match_the_contract() {
        assert_eq!(
            ResizeCanvasView::default(),
            ResizeCanvasView {
                region_w: 16,
                region_h: 16,
                region_pivot: Pivot::Center,
                resample: false,
                count_w: 4,
                count_h: 4,
                count_pivot: Pivot::Center,
            }
        );
    }
}
