//! Floating "create new document" dock panel (id 205).
//!
//! Registered as a `PanelPlacement::Floating` dock panel with
//! `can_pop_out == false`: it floats, cannot be docked, and closes from its
//! header's Close action (the dock renders Close-only for a floating-only
//! panel). `resizable == false` keeps the window fixed-size and draggable only
//! by its header.
//!
//! A static nest holds the create form: the single-region (tile) size, PER
//! AXIS, and the region counts, each on a labelled row, plus the Create action.
//! The panel is a pure view — the in-progress values live in the host's
//! [`CreateDocumentView`] cell so they survive across frames — and it never
//! touches the model: Create only pushes a [`CreateDocumentEvent::Create`] for
//! the App shell to apply.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
    FLOATING_PANEL_CHROME_HEIGHT,
};

/// Dock id for the create document floating panel.
pub const CREATE_DOCUMENT_PANEL_ID: u64 = 205;

/// Largest single-region side (pixels) the inputs accept.
const MAX_REGION_SIDE: u32 = 4096;

/// Largest region count along either axis the inputs accept.
const MAX_REGION_COUNT: u32 = 256;

/// Fixed minimum size of the (non-resizable) floating window.
/// Sized so all controls, preview, and Create/Cancel buttons fit cleanly
/// with zero scrollbars.
pub const CREATE_DOCUMENT_MIN_SIZE: egui::Vec2 = vec2(476.0, 236.0 + FLOATING_PANEL_CHROME_HEIGHT);

/// The in-progress create-document form values.
///
/// The host owns this cell; the panel edits it in place (it is the dialog's
/// draft state, not model data) and reads it back when Create is pressed.
///
/// `region_w`/`region_h` are the per-axis TILE size (the App shell maps them
/// onto `TileSize { w: region_w, h: region_h }`); regions need not be square.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateDocumentView {
    pub region_w: u32,
    pub region_h: u32,
    pub count_x: u32,
    pub count_y: u32,
}

impl Default for CreateDocumentView {
    fn default() -> Self {
        Self {
            region_w: 16,
            region_h: 16,
            count_x: 4,
            count_y: 4,
        }
    }
}

/// A create-document gesture emitted by the panel.
///
/// The App shell owns document creation; this pure view only ever pushes these
/// events into the host cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateDocumentEvent {
    /// Create a document of `region_w`×`region_h` (per-axis tile size) regions
    /// tiled `count_x`×`count_y` times.
    Create {
        region_w: u32,
        region_h: u32,
        count_x: u32,
        count_y: u32,
    },
    /// Close the create-document panel without creating a project.
    Cancel,
}

/// Shared cells for the create-document panel.
#[derive(Clone, Default)]
pub struct CreateDocumentHost {
    pub view: Rc<RefCell<CreateDocumentView>>,
    pub events: Rc<RefCell<Vec<CreateDocumentEvent>>>,
}

/// Canvas size for a view: `(region_w * count_x, region_h * count_y)`, each
/// axis independently (non-square tiles are supported).
///
/// Pure and saturating, so an extreme draft can never overflow.
pub fn canvas_size_for(view: &CreateDocumentView) -> (u32, u32) {
    (
        view.region_w.saturating_mul(view.count_x),
        view.region_h.saturating_mul(view.count_y),
    )
}

/// Pushes the current view as a [`CreateDocumentEvent::Create`].
///
/// The component calls this from the Create button; keeping it a free function
/// makes the event payload testable without driving egui.
fn push_create(host: &CreateDocumentHost) {
    let view = host.view.borrow();
    host.events.borrow_mut().push(CreateDocumentEvent::Create {
        region_w: view.region_w,
        region_h: view.region_h,
        count_x: view.count_x,
        count_y: view.count_y,
    });
}

/// Paints an aspect-ratio-scaled grid of rounded region tiles styled like the
/// Resize Canvas anchor grid.
fn paint_region_grid_preview(
    ui: &mut egui::Ui,
    region_w: u32,
    region_h: u32,
    count_x: u32,
    count_y: u32,
) {
    let preview_size = vec2(188.0, 112.0);
    let (frame_rect, _) = ui.allocate_exact_size(preview_size, egui::Sense::hover());
    let visuals = ui.visuals();
    let painter = ui.painter();

    // Subtle inset preview frame.
    painter.rect_filled(frame_rect, 4.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        frame_rect,
        4.0,
        visuals.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    let total_w = (region_w.max(1) as f32) * (count_x.max(1) as f32);
    let total_h = (region_h.max(1) as f32) * (count_y.max(1) as f32);
    let aspect = (total_w / total_h).clamp(0.12, 8.0);

    let inner = frame_rect.shrink(12.0);
    let (grid_w, grid_h) = if inner.width() / inner.height() > aspect {
        (inner.height() * aspect, inner.height())
    } else {
        (inner.width(), inner.width() / aspect)
    };
    let grid_rect = egui::Rect::from_center_size(inner.center(), vec2(grid_w, grid_h));

    // Cap rendered cells at 12 per axis so extreme grids remain crisp and legible.
    let draw_cols = count_x.clamp(1, 12);
    let draw_rows = count_y.clamp(1, 12);
    let gap = if draw_cols > 8 || draw_rows > 8 {
        1.5
    } else if draw_cols > 4 || draw_rows > 4 {
        2.5
    } else {
        4.0
    };
    let cell_w = ((grid_rect.width() - gap * (draw_cols - 1) as f32) / draw_cols as f32).max(2.0);
    let cell_h = ((grid_rect.height() - gap * (draw_rows - 1) as f32) / draw_rows as f32).max(2.0);
    let rounding = (cell_w.min(cell_h) * 0.16).clamp(1.0, 3.0);

    let active_fill = visuals.selection.bg_fill;
    let active_stroke = visuals.selection.stroke;
    let cell_fill = visuals.widgets.inactive.bg_fill;
    let cell_stroke = visuals.widgets.inactive.bg_stroke;

    for row in 0..draw_rows {
        for col in 0..draw_cols {
            let x = grid_rect.left() + col as f32 * (cell_w + gap);
            let y = grid_rect.top() + row as f32 * (cell_h + gap);
            let cell_rect = egui::Rect::from_min_size(egui::pos2(x, y), vec2(cell_w, cell_h));
            let is_origin = row == 0 && col == 0;
            painter.rect_filled(cell_rect, rounding, if is_origin { active_fill } else { cell_fill });
            painter.rect_stroke(
                cell_rect,
                rounding,
                if is_origin { active_stroke } else { cell_stroke },
                egui::StrokeKind::Inside,
            );
            if is_origin && cell_w.min(cell_h) >= 10.0 {
                let dot_r = (cell_w.min(cell_h) * 0.14).clamp(2.0, 4.0);
                painter.circle_filled(cell_rect.center(), dot_r, active_stroke.color);
            }
        }
    }
}

/// The create form: region size & counts on the left, aspect-scaled region grid
/// preview on the right, and Create / Cancel actions along the bottom.
struct CreateDocumentComponent {
    host: CreateDocumentHost,
}

impl Component for CreateDocumentComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let mut view = self.host.view.borrow().clone();
        let mut changed = false;

        ui.horizontal_top(|ui| {
            // Left column: clean numeric settings (no preset clutter, no scrollbars)
            ui.vertical(|ui| {
                ui.set_min_width(228.0);
                ui.set_max_width(228.0);

                ui.strong("Region Size (px)");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Width");
                    if ui
                        .add(
                            egui::DragValue::new(&mut view.region_w)
                                .range(1..=MAX_REGION_SIDE)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    ui.add_space(10.0);
                    ui.label("Height");
                    if ui
                        .add(
                            egui::DragValue::new(&mut view.region_h)
                                .range(1..=MAX_REGION_SIDE)
                                .speed(1.0),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });

                ui.add_space(12.0);
                ui.strong("Region Grid");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Cols");
                    if ui
                        .add(
                            egui::DragValue::new(&mut view.count_x)
                                .range(1..=MAX_REGION_COUNT)
                                .speed(0.25),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    ui.add_space(14.0);
                    ui.label("Rows");
                    if ui
                        .add(
                            egui::DragValue::new(&mut view.count_y)
                                .range(1..=MAX_REGION_COUNT)
                                .speed(0.25),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });

                ui.add_space(14.0);
                let (total_w, total_h) = canvas_size_for(&view);
                let total_cells = view.count_x.saturating_mul(view.count_y);
                ui.weak(format!(
                    "Canvas: {total_w} × {total_h} px ({total_cells} regions)"
                ));
            });

            // Fixed-height vertical divider so it does not expand to the full
            // ScrollArea viewport height and push the bottom buttons off-screen.
            ui.add_space(4.0);
            let (sep_rect, _) = ui.allocate_exact_size(vec2(1.0, 152.0), egui::Sense::hover());
            ui.painter().vline(
                sep_rect.center().x,
                sep_rect.y_range(),
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
            ui.add_space(4.0);

            // Right column: aspect-ratio-scaled region grid preview
            ui.vertical(|ui| {
                ui.strong("Preview");
                ui.add_space(6.0);
                paint_region_grid_preview(
                    ui,
                    view.region_w,
                    view.region_h,
                    view.count_x,
                    view.count_y,
                );
                ui.add_space(6.0);
                ui.weak(format!(
                    "{}×{} grid · {}×{} px/cell",
                    view.count_x, view.count_y, view.region_w, view.region_h
                ));
            });
        });

        if changed {
            let mut slot = self.host.view.borrow_mut();
            slot.region_w = view.region_w.max(1);
            slot.region_h = view.region_h.max(1);
            slot.count_x = view.count_x.max(1);
            slot.count_y = view.count_y.max(1);
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome
                    .button(ui, "Create", ButtonStyle::toggled(true))
                    .on_hover_text("Create the document")
                    .clicked()
                {
                    push_create(&self.host);
                }
                ui.add_space(6.0);
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    self.host.events.borrow_mut().push(CreateDocumentEvent::Cancel);
                }
            });
        });

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.host.events.borrow_mut().push(CreateDocumentEvent::Cancel);
        } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            push_create(&self.host);
        }
    }
}

/// Panel content: the static nest holding the create form.
struct CreateDocumentPanelContent {
    nest: NestContent,
}

impl PanelContent for CreateDocumentPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the create-document form.
pub(crate) fn build_create_document_nest(host: &CreateDocumentHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(CreateDocumentComponent { host: host.clone() });
    nest
}

/// Dock spec for the create-document floating panel (dock id 205).
///
/// **Float-only:** `can_pop_out == false`, so the header shows no Float/Dock
/// button and the panel is not dockable. `resizable == false` keeps the window
/// fixed-size and only draggable by its header.
pub fn create_document_panel_spec(
    host: &CreateDocumentHost,
    floating_rect: egui::Rect,
) -> PanelSpec {
    let floating_rect = egui::Rect::from_min_size(
        floating_rect.min,
        vec2(
            floating_rect.width().max(CREATE_DOCUMENT_MIN_SIZE.x),
            floating_rect.height().max(CREATE_DOCUMENT_MIN_SIZE.y),
        ),
    );
    PanelSpec {
        id: PanelId::new(CREATE_DOCUMENT_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new("create new document", false, CREATE_DOCUMENT_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(CreateDocumentPanelContent {
            nest: build_create_document_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_size_multiplies_region_size_by_count() {
        let view = CreateDocumentView {
            region_w: 16,
            region_h: 16,
            count_x: 4,
            count_y: 4,
        };
        assert_eq!(canvas_size_for(&view), (64, 64));
        let tall = CreateDocumentView {
            region_w: 16,
            region_h: 16,
            count_x: 4,
            count_y: 8,
        };
        assert_eq!(canvas_size_for(&tall), (64, 128));
        assert_eq!(
            CreateDocumentView::default(),
            CreateDocumentView {
                region_w: 16,
                region_h: 16,
                count_x: 4,
                count_y: 4,
            },
            "the draft defaults must match the contract"
        );
    }

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = CreateDocumentHost::default();
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), vec2(220.0, 150.0));
        let spec = create_document_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), CREATE_DOCUMENT_PANEL_ID);
        assert_eq!(spec.metadata.title, "create new document");
        assert!(
            !spec.metadata.can_pop_out,
            "the create-document panel is float-only"
        );
        assert!(
            !spec.metadata.resizable,
            "the create-document window is fixed-size"
        );
        assert_eq!(spec.placement, PanelPlacement::Floating);
        let nest = build_create_document_nest(&host);
        assert_eq!(nest.mode(), NestMode::Static);
    }

    #[test]
    fn create_event_carries_current_view_values() {
        let host = CreateDocumentHost::default();
        *host.view.borrow_mut() = CreateDocumentView {
            region_w: 8,
            region_h: 16,
            count_x: 3,
            count_y: 5,
        };
        push_create(&host);
        let events: Vec<CreateDocumentEvent> = host.events.borrow_mut().drain(..).collect();
        assert_eq!(
            events,
            vec![CreateDocumentEvent::Create {
                region_w: 8,
                region_h: 16,
                count_x: 3,
                count_y: 5,
            }]
        );
    }

    #[test]
    fn create_document_panel_fits_without_vertical_overflow() {
        let host = CreateDocumentHost::default();
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), CREATE_DOCUMENT_MIN_SIZE);
        let mut nest = build_create_document_nest(&host);
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(CREATE_DOCUMENT_PANEL_ID)),
        );
        // Simulate the exact floating panel body area inside the floating window
        // (window margin + header + frame padding) and measure inner content size.
        let chrome_h = crate::ui::panel_dock::FLOATING_PANEL_CHROME_HEIGHT;
        let viewport_size = vec2(
            rect.width() - 22.0,
            rect.height() - chrome_h,
        );
        let body_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            vec2(
                viewport_size.x + 2.0 * crate::ui::panel_dock::nest::NEST_PADDING,
                viewport_size.y + 2.0 * crate::ui::panel_dock::nest::NEST_PADDING,
            ),
        );
        let mut measured_content_size = egui::Vec2::ZERO;
        let raw = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(body_rect), |body| {
                let nest_vp = crate::ui::panel_dock::nest::nest_viewport(body.available_rect_before_wrap());
                body.scope_builder(egui::UiBuilder::new().max_rect(nest_vp), |inner| {
                    let out = egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .min_scrolled_width(0.0)
                        .min_scrolled_height(0.0)
                        .show(inner, |scroll| {
                            let view = crate::ui::panel_dock::nest::NestView::screen(nest_vp);
                            let mut comp = CreateDocumentComponent { host: host.clone() };
                            comp.ui(scroll, &view, &chrome);
                        });
                    measured_content_size = out.content_size;
                });
                nest.ui(body, &chrome);
            });
        })
        .textures_delta
        .clear();
        assert!(
            measured_content_size.y <= viewport_size.y,
            "panel content height ({}) must fit inside viewport ({}) without vertical scrollbar",
            measured_content_size.y,
            viewport_size.y
        );
        assert!(
            measured_content_size.x <= viewport_size.x,
            "panel content width ({}) must fit inside viewport ({}) without horizontal scrollbar",
            measured_content_size.x,
            viewport_size.x
        );
    }
}
