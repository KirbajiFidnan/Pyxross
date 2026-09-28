use std::cell::Cell;
use std::rc::Rc;

use egui::{pos2, vec2, Rect};

use super::nest::{Component, ComponentPlacement, NestContent, NestMode, NestView};
use super::preview::{PreviewControls, PreviewFeed, PreviewImage};
use super::skin::PanelChrome;
use super::state::{DockManager, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec};

struct DummyPanel;

impl PanelContent for DummyPanel {
    fn ui(&mut self, _ui: &mut egui::Ui, _chrome: &PanelChrome) {}
}

/// A theme-colored checkerboard world grid for the dynamic nest.
struct Checkerboard {
    size: egui::Vec2,
    cell: f32,
}

impl Checkerboard {
    fn new(size: egui::Vec2, cell: f32) -> Self {
        Self { size, cell }
    }
}

impl Component for Checkerboard {
    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, _chrome: &PanelChrome) {
        let painter = view.painter(ui);
        let dark = ui.visuals().extreme_bg_color;
        let light = ui.visuals().faint_bg_color;
        let visible = view.visible_world_rect();
        let mut row = 0;
        let mut y = 0.0;
        while y < self.size.y {
            let mut col = 0;
            let mut x = 0.0;
            while x < self.size.x {
                let cell = Rect::from_min_size(pos2(x, y), vec2(self.cell, self.cell));
                if visible.intersects(cell) {
                    let color = if (row + col) % 2 == 0 { dark } else { light };
                    painter.rect_filled(view.world_rect_to_screen(cell), 0.0, color);
                }
                col += 1;
                x += self.cell;
            }
            row += 1;
            y += self.cell;
        }
    }
}

/// A screen-space overlay label showing the dynamic nest's zoom percent.
struct ZoomLabel;

impl Component for ZoomLabel {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, _chrome: &PanelChrome) {
        ui.label(format!("zoom {:.0}%", view.scale() * 100.0));
    }
}

/// A tall/wide static content block with an interactive button row.
struct StaticRows {
    size: egui::Vec2,
    clicks: Rc<Cell<u32>>,
}

impl StaticRows {
    fn new(size: egui::Vec2) -> Self {
        Self {
            size,
            clicks: Rc::new(Cell::new(0)),
        }
    }
}

impl Component for StaticRows {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, _chrome: &PanelChrome) {
        ui.set_min_size(self.size);
        ui.painter().rect_filled(
            Rect::from_min_size(ui.min_rect().min, self.size),
            0.0,
            ui.visuals().extreme_bg_color,
        );
        ui.horizontal(|ui| {
            if ui.button("Nest button").clicked() {
                self.clicks.set(self.clicks.get() + 1);
            }
            ui.label(format!("clicks: {}", self.clicks.get()));
        });
        for index in 0..40 {
            ui.label(format!("Row {index} of the static nest"));
        }
    }
}

/// A screen-space overlay pinned to the static nest viewport: a button row
/// that does not scroll with the content.
struct StaticOverlay {
    clicks: Rc<Cell<u32>>,
}

impl Component for StaticOverlay {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, _chrome: &PanelChrome) {
        ui.horizontal(|ui| {
            if ui.button("Pinned button").clicked() {
                self.clicks.set(self.clicks.get() + 1);
            }
            ui.label(format!("pinned: {}", self.clicks.get()));
        });
    }
}

impl DockManager {
    pub fn demo() -> Self {
        let preview = PreviewFeed::default();
        let specs = [
            demo_spec(
                1,
                "Preview",
                true,
                PanelPlacement::DockedRight,
                pos2(40.0, 80.0),
                &preview,
            ),
            demo_spec(
                2,
                "Panel B",
                false,
                PanelPlacement::DockedRight,
                pos2(220.0, 140.0),
                &preview,
            ),
            demo_spec(
                4,
                "Panel D",
                true,
                PanelPlacement::DockedRight,
                pos2(400.0, 200.0),
                &preview,
            ),
            demo_spec(
                3,
                "Panel C",
                false,
                PanelPlacement::DockedBottom,
                pos2(160.0, 360.0),
                &preview,
            ),
        ];
        match Self::try_new(specs) {
            Ok(mut manager) => {
                manager.preview_feed = preview;
                manager
            }
            Err(_) => Self::empty(),
        }
    }
}

fn demo_spec(
    raw_id: u64,
    title: &'static str,
    can_pop_out: bool,
    placement: PanelPlacement,
    position: egui::Pos2,
    preview: &PreviewFeed,
) -> PanelSpec {
    let content: Box<dyn PanelContent> = match raw_id {
        1 => {
            let mut nest = NestContent::new(NestMode::Dynamic);
            let reset = nest.reset_handle();
            nest.push(PreviewImage::new(preview.clone()))
                .push(PreviewControls::new(reset));
            Box::new(nest)
        }
        2 => {
            let mut nest = NestContent::new(NestMode::Dynamic);
            nest.push(Checkerboard::new(vec2(512.0, 512.0), 32.0))
                .push(ZoomLabel);
            Box::new(nest)
        }
        4 => {
            let mut nest = NestContent::new(NestMode::Static);
            nest.push(StaticRows::new(vec2(600.0, 900.0)))
                .push(StaticOverlay {
                    clicks: Rc::new(Cell::new(0)),
                });
            Box::new(nest)
        }
        _ => Box::new(DummyPanel),
    };
    PanelSpec {
        id: PanelId::new(raw_id),
        metadata: PanelMetadata::new(title, can_pop_out, vec2(180.0, 120.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(position, vec2(260.0, 190.0)),
        content,
    }
}
