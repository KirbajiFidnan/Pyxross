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
};

/// Dock id for the create document floating panel.
pub const CREATE_DOCUMENT_PANEL_ID: u64 = 205;

/// Largest single-region side (pixels) the inputs accept.
const MAX_REGION_SIDE: u32 = 4096;

/// Largest region count along either axis the inputs accept.
const MAX_REGION_COUNT: u32 = 256;

/// Fixed minimum size of the (non-resizable) floating window.
const CREATE_DOCUMENT_MIN_SIZE: egui::Vec2 = vec2(220.0, 150.0);

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

/// The create form: region size and counts, then the Create action.
struct CreateDocumentComponent {
    host: CreateDocumentHost,
}

impl Component for CreateDocumentComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        // Single-region size: a width/height pair on one row. The locals are
        // copied out of the host so the borrow is released before the widgets
        // run; on change the edited values are written back into the view cell.
        ui.strong("size of single region");
        ui.horizontal(|ui| {
            let (mut region_w, mut region_h) = {
                let view = self.host.view.borrow();
                (view.region_w, view.region_h)
            };
            ui.label("width");
            let w_response = ui.add(egui::DragValue::new(&mut region_w).range(1..=MAX_REGION_SIDE));
            ui.label("height");
            let h_response = ui.add(egui::DragValue::new(&mut region_h).range(1..=MAX_REGION_SIDE));
            if w_response.changed() || h_response.changed() {
                let mut view = self.host.view.borrow_mut();
                view.region_w = region_w.max(1);
                view.region_h = region_h.max(1);
            }
        });

        // Region counts: the same width/height row shape, bound to the count
        // axes.
        ui.strong("numbers of regions");
        ui.horizontal(|ui| {
            let (mut count_x, mut count_y) = {
                let view = self.host.view.borrow();
                (view.count_x, view.count_y)
            };
            ui.label("width");
            let x_response = ui.add(egui::DragValue::new(&mut count_x).range(1..=MAX_REGION_COUNT));
            ui.label("height");
            let y_response = ui.add(egui::DragValue::new(&mut count_y).range(1..=MAX_REGION_COUNT));
            if x_response.changed() || y_response.changed() {
                let mut view = self.host.view.borrow_mut();
                view.count_x = count_x.max(1);
                view.count_y = count_y.max(1);
            }
        });

        // Create emits the CURRENT view values for the App shell to apply; the
        // panel itself mutates no model.
        if chrome
            .button(ui, "Create", ButtonStyle::plain())
            .on_hover_text("Create the document")
            .clicked()
        {
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
}
