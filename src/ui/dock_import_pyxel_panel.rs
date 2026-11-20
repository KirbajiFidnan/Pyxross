//! Floating "Import Pyxel" dock panel (id 217).
//!
//! Displays a two-pane layout: left document metadata summary & actions, right
//! live canvas preview (`NEAREST`) for a parsed `.pyxel` file, letting the
//! user confirm importing it as a Pyxross project (with tiles, layers,
//! animations, and color palette).

use std::cell::RefCell;
use std::rc::Rc;

use egui::{pos2, vec2};

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import Pyxel floating panel.
pub const IMPORT_PYXEL_PANEL_ID: u64 = 217;

/// Panel/window title.
const IMPORT_PYXEL_TITLE: &str = "Import Pyxel";

/// Minimum size of the floating Import Pyxel panel.
const IMPORT_PYXEL_MIN_SIZE: egui::Vec2 = vec2(620.0, 380.0);
const FORM_PANE_WIDTH: f32 = 250.0;

/// View state populated when a `.pyxel` file is parsed.
#[derive(Clone, Debug, Default)]
pub struct ImportPyxelView {
    pub file_name: String,
    pub doc_name: String,
    pub width: usize,
    pub height: usize,
    pub tile_w: u32,
    pub tile_h: u32,
    pub layer_count: usize,
    pub tile_count: usize,
    pub animation_count: usize,
    pub palette_colors: usize,
    pub preview_rgba: Vec<u8>,
    pub preview_generation: u64,
    pub error: Option<String>,
}

/// Actions emitted by the Import Pyxel panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportPyxelEvent {
    Import,
    Cancel,
}

/// Shared view & event state for the Import Pyxel panel.
#[derive(Clone, Default)]
pub struct ImportPyxelHost {
    pub view: Rc<RefCell<ImportPyxelView>>,
    pub events: Rc<RefCell<Vec<ImportPyxelEvent>>>,
}

impl ImportPyxelHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_events(&self) -> Vec<ImportPyxelEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    pub fn set_error(&self, msg: String) {
        self.view.borrow_mut().error = Some(msg);
    }
}

struct ImportPyxelComponent {
    host: ImportPyxelHost,
    preview_tex: Option<(u64, egui::TextureHandle)>,
    preview_viewport: crate::ui::panel_dock::preview::PreviewViewport,
}

impl ImportPyxelComponent {
    fn ensure_preview_texture(
        &mut self,
        ctx: &egui::Context,
        view: &ImportPyxelView,
    ) -> Option<egui::TextureId> {
        if view.width == 0
            || view.height == 0
            || view.preview_rgba.len() != view.width * view.height * 4
        {
            return None;
        }
        if let Some((gen, tex)) = &self.preview_tex {
            if *gen == view.preview_generation {
                return Some(tex.id());
            }
        }
        let image =
            egui::ColorImage::from_rgba_unmultiplied([view.width, view.height], &view.preview_rgba);
        let tex = ctx.load_texture(
            "import-pyxel-preview",
            image,
            egui::TextureOptions::NEAREST,
        );
        let id = tex.id();
        self.preview_tex = Some((view.preview_generation, tex));
        Some(id)
    }

    fn draw_left_pane(
        &self,
        ui: &mut egui::Ui,
        view: &ImportPyxelView,
        chrome: &PanelChrome,
    ) -> Option<ImportPyxelEvent> {
        let mut emitted = None;
        let total_h = ui.available_height();
        let footer_h = 44.0;
        let top_h = (total_h - footer_h).max(120.0);

        ui.allocate_ui_with_layout(
            vec2(FORM_PANE_WIDTH, top_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                let card_w = ui.available_width();
                egui::Frame::new()
                    .fill(chrome.colors.panel_header_bg32())
                    .stroke(egui::Stroke::new(1.0, chrome.colors.panel_border32()))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_min_width((card_w - 22.0).max(120.0));
                        ui.strong(&view.doc_name);
                        ui.add_space(2.0);
                        ui.weak(&view.file_name);
                    });

                ui.add_space(10.0);
                ui.strong("Document details");
                ui.add_space(6.0);

                let rows = [
                    ("Canvas", format!("{} × {} px", view.width, view.height)),
                    ("Tile size", format!("{} × {} px", view.tile_w, view.tile_h)),
                    ("Layers", view.layer_count.to_string()),
                    ("Tiles", view.tile_count.to_string()),
                    ("Animations", view.animation_count.to_string()),
                    ("Palette", format!("{} colors", view.palette_colors)),
                ];
                egui::Grid::new("import-pyxel-details")
                    .num_columns(2)
                    .spacing([12.0, 5.0])
                    .show(ui, |ui| {
                        for (label, value) in rows {
                            ui.weak(label);
                            ui.label(value);
                            ui.end_row();
                        }
                    });

                if let Some(err) = &view.error {
                    ui.add_space(10.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
            },
        );

        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.set_width(FORM_PANE_WIDTH);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome
                    .button(
                        ui,
                        "Import",
                        ButtonStyle {
                            selected: true,
                            ..ButtonStyle::plain()
                        },
                    )
                    .clicked()
                {
                    emitted = Some(ImportPyxelEvent::Import);
                }
                ui.add_space(6.0);
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    emitted = Some(ImportPyxelEvent::Cancel);
                }
            });
        });

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(ImportPyxelEvent::Cancel);
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            emitted = Some(ImportPyxelEvent::Import);
        }
        emitted
    }

    fn draw_right_preview(
        &mut self,
        ui: &mut egui::Ui,
        view: &ImportPyxelView,
        chrome: &PanelChrome,
    ) {
        let avail = ui.available_size();
        let (box_rect, _) = ui.allocate_exact_size(
            vec2(avail.x.max(140.0), avail.y.max(140.0)),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        painter.rect_filled(box_rect, 0.0, chrome.colors.panel_header_bg32());
        painter.rect_stroke(
            box_rect,
            0.0,
            egui::Stroke::new(1.0, chrome.colors.panel_border32()),
            egui::StrokeKind::Inside,
        );

        let inner = box_rect.shrink(8.0);
        let header_rect =
            egui::Rect::from_min_max(inner.min, pos2(inner.max.x, inner.min.y + 18.0));
        let footer_h = 22.0;
        let canvas_area = egui::Rect::from_min_max(
            pos2(inner.min.x, header_rect.max.y + 6.0),
            pos2(inner.max.x, (inner.max.y - footer_h - 4.0).max(header_rect.max.y + 12.0)),
        );
        let footer_rect = egui::Rect::from_min_max(
            pos2(inner.min.x, canvas_area.max.y + 4.0),
            inner.max,
        );

        painter.text(
            header_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "Preview",
            egui::FontId::proportional(12.0),
            ui.visuals().text_color(),
        );
        if view.width > 0 && view.height > 0 {
            painter.text(
                header_rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                format!("{} × {} px", view.width, view.height),
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }

        self.preview_viewport
            .sync_generation(view.preview_generation);
        let interact_resp = ui.interact(
            canvas_area,
            ui.id().with("import-pyxel-preview-viewport"),
            egui::Sense::click_and_drag(),
        );
        self.preview_viewport
            .handle_interaction(ui, canvas_area, &interact_resp);

        if let Some(tex_id) = self.ensure_preview_texture(ui.ctx(), view) {
            let aspect = view.width as f32 / view.height.max(1) as f32;
            let mut draw_w = canvas_area.width().max(1.0);
            let mut draw_h = draw_w / aspect;
            if draw_h > canvas_area.height().max(1.0) {
                draw_h = canvas_area.height().max(1.0);
                draw_w = draw_h * aspect;
            }
            let base_rect = egui::Rect::from_center_size(
                canvas_area.center(),
                vec2(draw_w.max(1.0), draw_h.max(1.0)),
            );
            let img_rect = self
                .preview_viewport
                .transform_rect(base_rect, canvas_area.center());
            let clipped = ui.painter_at(canvas_area);
            clipped.rect_filled(img_rect, 0.0, chrome.colors.panel_bg32());
            clipped.image(
                tex_id,
                img_rect,
                egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                crate::ui::theme::SKIN_TINT,
            );
            clipped.rect_stroke(
                img_rect,
                0.0,
                egui::Stroke::new(1.0, chrome.colors.panel_border32()),
                egui::StrokeKind::Outside,
            );
        }

        self.preview_viewport.draw_footer(ui, chrome, footer_rect);
    }
}

impl Component for ImportPyxelComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow().clone();
        let mut emitted = None;
        let total_h = ui.available_height().max(240.0);
        ui.horizontal_top(|ui| {
            ui.set_min_height(total_h);
            ui.allocate_ui_with_layout(
                vec2(FORM_PANE_WIDTH, total_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    emitted = self.draw_left_pane(ui, &view, chrome);
                },
            );
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.set_min_height(total_h);
                self.draw_right_preview(ui, &view, chrome);
            });
        });

        if let Some(ev) = emitted {
            self.host.events.borrow_mut().push(ev);
        }
    }
}

struct ImportPyxelPanelContent {
    nest: NestContent,
}

impl PanelContent for ImportPyxelPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

pub(crate) fn build_import_pyxel_nest(host: &ImportPyxelHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ImportPyxelComponent {
        host: host.clone(),
        preview_tex: None,
        preview_viewport: crate::ui::panel_dock::preview::PreviewViewport::new(),
    });
    nest
}

/// Dock spec for the Import Pyxel floating panel (id 217).
pub fn import_pyxel_panel_spec(host: &ImportPyxelHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(IMPORT_PYXEL_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(IMPORT_PYXEL_TITLE, false, IMPORT_PYXEL_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ImportPyxelPanelContent {
            nest: build_import_pyxel_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_uses_static_floating_panel() {
        let host = ImportPyxelHost::new();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), IMPORT_PYXEL_MIN_SIZE);
        let spec = import_pyxel_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), IMPORT_PYXEL_PANEL_ID);
        assert_eq!(spec.metadata.title, "Import Pyxel");
        assert!(!spec.metadata.can_pop_out);
        assert_eq!(spec.placement, PanelPlacement::Floating);
    }
}
