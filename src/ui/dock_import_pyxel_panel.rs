//! Floating "Import Pyxel" dock panel (id 217).
//!
//! Displays a canvas preview and summary metadata for a parsed `.pyxel` file,
//! letting the user confirm importing it as a Pyxross project (with tiles,
//! layers, animations, and color palette).

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import Pyxel floating panel.
pub const IMPORT_PYXEL_PANEL_ID: u64 = 217;

/// Panel/window title.
const IMPORT_PYXEL_TITLE: &str = "Import Pyxel";

/// Minimum size of the floating Import Pyxel panel.
const IMPORT_PYXEL_MIN_SIZE: egui::Vec2 = vec2(420.0, 380.0);

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
}

impl ImportPyxelComponent {
    fn ensure_preview_texture(
        &mut self,
        ctx: &egui::Context,
        view: &ImportPyxelView,
    ) -> Option<egui::TextureId> {
        if view.width == 0 || view.height == 0 || view.preview_rgba.len() != view.width * view.height * 4 {
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
}

impl Component for ImportPyxelComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow().clone();
        ui.label(format!("File: {}", view.file_name));
        ui.label(format!("Document: {}", view.doc_name));
        ui.add_space(4.0);
        ui.label(format!(
            "Canvas: {}×{} px   |   Tile: {}×{} px",
            view.width, view.height, view.tile_w, view.tile_h
        ));
        ui.label(format!(
            "Layers: {}   |   Tiles: {}   |   Animations: {}   |   Palette: {} colors",
            view.layer_count, view.tile_count, view.animation_count, view.palette_colors
        ));
        ui.add_space(8.0);

        let avail = ui.available_width().max(120.0);
        let box_h = (ui.available_height() - 56.0).clamp(140.0, 260.0);
        let (preview_rect, _) =
            ui.allocate_exact_size(vec2(avail, box_h), egui::Sense::hover());
        ui.painter()
            .rect_filled(preview_rect, 2.0, chrome.colors.panel_header_bg32());
        ui.painter().rect_stroke(
            preview_rect,
            2.0,
            egui::Stroke::new(1.0, chrome.colors.panel_border32()),
            egui::StrokeKind::Inside,
        );

        if let Some(tex_id) = self.ensure_preview_texture(ui.ctx(), &view) {
            let inner = preview_rect.shrink(6.0);
            let aspect = view.width as f32 / view.height.max(1) as f32;
            let mut draw_w = inner.width();
            let mut draw_h = draw_w / aspect;
            if draw_h > inner.height() {
                draw_h = inner.height();
                draw_w = draw_h * aspect;
            }
            let img_rect =
                egui::Rect::from_center_size(inner.center(), vec2(draw_w.max(1.0), draw_h.max(1.0)));
            ui.painter().image(
                tex_id,
                img_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                crate::ui::theme::SKIN_TINT,
            );
        }

        if let Some(err) = &view.error {
            ui.add_space(4.0);
            ui.colored_label(chrome.colors.selection_stroke_color32(), err);
        }

        ui.add_space(8.0);
        let mut emitted = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if chrome.button(ui, "Import", ButtonStyle::plain()).clicked() {
                emitted = Some(ImportPyxelEvent::Import);
            }
            if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                emitted = Some(ImportPyxelEvent::Cancel);
            }
        });
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(ImportPyxelEvent::Cancel);
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            emitted = Some(ImportPyxelEvent::Import);
        }
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
