//! Floating "Import Aseprite" dock panel (id 218).
//!
//! Lets the user choose how `.ase`/`.aseprite` frames are laid out into a
//! Pyxross region sheet (`Row length`, `Single row document`, or `Split by tags`)
//! and previews the resulting sheet before importing.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::core::aseprite::{build_aseprite_import, layout_grid, AsepriteLayoutMode, ParsedAseprite};
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import Aseprite floating panel.
pub const IMPORT_ASEPRITE_PANEL_ID: u64 = 218;

const IMPORT_ASEPRITE_TITLE: &str = "Import Aseprite";
const IMPORT_ASEPRITE_MIN_SIZE: egui::Vec2 = vec2(440.0, 420.0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsepriteUiMode {
    RowLength,
    SingleRow,
    SplitByTags,
}

#[derive(Clone, Debug)]
pub struct ImportAsepriteView {
    pub file_name: String,
    pub parsed: Option<ParsedAseprite>,
    pub mode: AsepriteUiMode,
    pub line_length_text: String,
    pub preview_w: usize,
    pub preview_h: usize,
    pub preview_rgba: Vec<u8>,
    pub preview_generation: u64,
    pub error: Option<String>,
}

impl Default for ImportAsepriteView {
    fn default() -> Self {
        Self {
            file_name: String::new(),
            parsed: None,
            mode: AsepriteUiMode::SplitByTags,
            line_length_text: "8".to_owned(),
            preview_w: 0,
            preview_h: 0,
            preview_rgba: Vec::new(),
            preview_generation: 0,
            error: None,
        }
    }
}

impl ImportAsepriteView {
    pub fn effective_layout_mode(&self) -> Result<AsepriteLayoutMode, String> {
        match self.mode {
            AsepriteUiMode::SingleRow => Ok(AsepriteLayoutMode::SingleRow),
            AsepriteUiMode::SplitByTags => Ok(AsepriteLayoutMode::SplitByTags),
            AsepriteUiMode::RowLength => {
                let n: u32 = self
                    .line_length_text
                    .trim()
                    .parse()
                    .map_err(|_| "Row length must be a positive integer".to_owned())?;
                if n == 0 {
                    return Err("Row length must be at least 1".to_owned());
                }
                Ok(AsepriteLayoutMode::RowLength(n))
            }
        }
    }

    pub fn refresh_preview(&mut self) {
        let Some(parsed) = &self.parsed else {
            return;
        };
        match self.effective_layout_mode() {
            Ok(mode) => match build_aseprite_import(parsed, &self.file_name, mode) {
                Ok(imp) => {
                    self.preview_w = imp.width;
                    self.preview_h = imp.height;
                    self.preview_rgba = imp.preview_rgba;
                    self.preview_generation = self.preview_generation.wrapping_add(1);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            },
            Err(e) => self.error = Some(e),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportAsepriteEvent {
    Import(AsepriteLayoutMode),
    Cancel,
}

#[derive(Clone, Default)]
pub struct ImportAsepriteHost {
    pub view: Rc<RefCell<ImportAsepriteView>>,
    pub events: Rc<RefCell<Vec<ImportAsepriteEvent>>>,
}

impl ImportAsepriteHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_events(&self) -> Vec<ImportAsepriteEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

struct ImportAsepriteComponent {
    host: ImportAsepriteHost,
    preview_tex: Option<(u64, egui::TextureHandle)>,
}

impl ImportAsepriteComponent {
    fn ensure_preview_texture(
        &mut self,
        ctx: &egui::Context,
        view: &ImportAsepriteView,
    ) -> Option<egui::TextureId> {
        if view.preview_w == 0
            || view.preview_h == 0
            || view.preview_rgba.len() != view.preview_w * view.preview_h * 4
        {
            return None;
        }
        if let Some((gen, tex)) = &self.preview_tex {
            if *gen == view.preview_generation {
                return Some(tex.id());
            }
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [view.preview_w, view.preview_h],
            &view.preview_rgba,
        );
        let tex = ctx.load_texture(
            "import-aseprite-preview",
            image,
            egui::TextureOptions::NEAREST,
        );
        let id = tex.id();
        self.preview_tex = Some((view.preview_generation, tex));
        Some(id)
    }
}

impl Component for ImportAsepriteComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let mut needs_refresh = false;
        {
            let mut view = self.host.view.borrow_mut();
            let (frame_count, frame_w, frame_h, tag_count, layer_count) = view
                .parsed
                .as_ref()
                .map(|p| (p.frames.len(), p.frame_w, p.frame_h, p.tags.len(), p.layers.len()))
                .unwrap_or((0, 0, 0, 0, 0));

            ui.label(format!("File: {}", view.file_name));
            ui.label(format!(
                "Frames: {} ({}×{} px/region)  |  Tags: {}  |  Layers: {}",
                frame_count, frame_w, frame_h, tag_count, layer_count
            ));
            if let (Some(parsed), Ok(mode)) = (&view.parsed, view.effective_layout_mode()) {
                let (cols, rows) = layout_grid(parsed, mode);
                ui.weak(format!(
                    "Sheet: {}×{} regions ({}×{} px)",
                    cols,
                    rows,
                    cols * frame_w as usize,
                    rows * frame_h as usize
                ));
            }
            ui.add_space(6.0);

            let mut single_row = view.mode == AsepriteUiMode::SingleRow;
            let mut split_tags = view.mode == AsepriteUiMode::SplitByTags;
            ui.horizontal(|ui| {
                if ui.checkbox(&mut single_row, "Single row document").changed() {
                    view.mode = if single_row {
                        AsepriteUiMode::SingleRow
                    } else {
                        AsepriteUiMode::RowLength
                    };
                    needs_refresh = true;
                }
                if ui.checkbox(&mut split_tags, "Split by tags").changed() {
                    view.mode = if split_tags {
                        AsepriteUiMode::SplitByTags
                    } else {
                        AsepriteUiMode::RowLength
                    };
                    needs_refresh = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Row length:");
                let enabled = view.mode == AsepriteUiMode::RowLength;
                let resp = ui.add_enabled(
                    enabled,
                    egui::TextEdit::singleline(&mut view.line_length_text).desired_width(64.0),
                );
                if resp.changed() {
                    needs_refresh = true;
                }
            });
            if needs_refresh {
                view.refresh_preview();
            }
        }

        let view_snap = self.host.view.borrow().clone();
        ui.add_space(6.0);

        let avail = ui.available_width().max(120.0);
        let box_h = (ui.available_height() - 56.0).clamp(130.0, 250.0);
        let (preview_rect, _) = ui.allocate_exact_size(vec2(avail, box_h), egui::Sense::hover());
        ui.painter()
            .rect_filled(preview_rect, 2.0, chrome.colors.panel_header_bg32());
        ui.painter().rect_stroke(
            preview_rect,
            2.0,
            egui::Stroke::new(1.0, chrome.colors.panel_border32()),
            egui::StrokeKind::Inside,
        );
        if let Some(tex_id) = self.ensure_preview_texture(ui.ctx(), &view_snap) {
            let inner = preview_rect.shrink(6.0);
            let aspect = view_snap.preview_w as f32 / view_snap.preview_h.max(1) as f32;
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

        if let Some(err) = &view_snap.error {
            ui.add_space(4.0);
            ui.colored_label(chrome.colors.selection_stroke_color32(), err);
        }

        ui.add_space(8.0);
        let mut emitted = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let can_import = view_snap.effective_layout_mode().is_ok() && view_snap.parsed.is_some();
            if chrome
                .button(
                    ui,
                    "Import",
                    ButtonStyle {
                        enabled: can_import,
                        ..ButtonStyle::plain()
                    },
                )
                .clicked()
            {
                if let Ok(mode) = view_snap.effective_layout_mode() {
                    emitted = Some(ImportAsepriteEvent::Import(mode));
                }
            }
            if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                emitted = Some(ImportAsepriteEvent::Cancel);
            }
        });
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(ImportAsepriteEvent::Cancel);
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            if let Ok(mode) = view_snap.effective_layout_mode() {
                emitted = Some(ImportAsepriteEvent::Import(mode));
            }
        }
        if let Some(ev) = emitted {
            self.host.events.borrow_mut().push(ev);
        }
    }
}

struct ImportAsepritePanelContent {
    nest: NestContent,
}

impl PanelContent for ImportAsepritePanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

pub fn import_aseprite_panel_spec(
    host: &ImportAsepriteHost,
    floating_rect: egui::Rect,
) -> PanelSpec {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ImportAsepriteComponent {
        host: host.clone(),
        preview_tex: None,
    });
    PanelSpec {
        id: PanelId::new(IMPORT_ASEPRITE_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(IMPORT_ASEPRITE_TITLE, false, IMPORT_ASEPRITE_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ImportAsepritePanelContent { nest }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_floating_and_resizable() {
        let host = ImportAsepriteHost::new();
        let rect = egui::Rect::from_min_size(egui::pos2(80.0, 80.0), IMPORT_ASEPRITE_MIN_SIZE);
        let spec = import_aseprite_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), IMPORT_ASEPRITE_PANEL_ID);
        assert_eq!(spec.metadata.title, "Import Aseprite");
        assert_eq!(spec.placement, PanelPlacement::Floating);
        assert!(spec.metadata.resizable);
    }
}
