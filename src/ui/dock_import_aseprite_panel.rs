//! Floating "Import Aseprite" dock panel (id 218).
//!
//! Supports two import methods:
//! - **Timeline canvas**: tiles Aseprite timeline frames into a Pyxross region
//!   sheet (`Split by tags`, `Single row`, or `Row length`) and imports tags as
//!   animations.
//! - **Single canvas**: ignores the timeline and treats frame 0 as a single
//!   canvas that can be split by pixels or by region count (matching Import
//!   Image).
//!
//! Two-pane layout: left static form controls, right live preview with region
//! grid overlay.

use std::cell::RefCell;
use std::rc::Rc;

use egui::{pos2, vec2};

use crate::core::aseprite::{build_aseprite_import, layout_grid, AsepriteLayoutMode, ParsedAseprite};
use crate::ui::dock_import_image_panel::{parse_pair, ImportImageField};
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import Aseprite floating panel.
pub const IMPORT_ASEPRITE_PANEL_ID: u64 = 218;

const IMPORT_ASEPRITE_TITLE: &str = "Import Aseprite";
const IMPORT_ASEPRITE_MIN_SIZE: egui::Vec2 = vec2(660.0, 420.0);
const FORM_PANE_WIDTH: f32 = 268.0;
const AXIS_INPUT_WIDTH: f32 = 64.0;

/// Top-level import method in the Import Aseprite dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsepriteImportMethod {
    TimelineCanvas,
    SingleCanvas,
}

/// Timeline tiling layout when [`AsepriteImportMethod::TimelineCanvas`] is active.
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
    pub method: AsepriteImportMethod,
    pub mode: AsepriteUiMode,
    pub line_length_text: String,
    pub pixels_x_text: String,
    pub pixels_y_text: String,
    pub regions_x_text: String,
    pub regions_y_text: String,
    pub last_edited: ImportImageField,
    pub syncing: bool,
    pub max_canvas_w: u32,
    pub max_canvas_h: u32,
    pub max_frames: usize,
    pub preview_w: usize,
    pub preview_h: usize,
    pub preview_rgba: Vec<u8>,
    pub preview_grid: (usize, usize),
    pub preview_generation: u64,
    pub error: Option<String>,
}

impl Default for ImportAsepriteView {
    fn default() -> Self {
        Self {
            file_name: String::new(),
            parsed: None,
            method: AsepriteImportMethod::TimelineCanvas,
            mode: AsepriteUiMode::SplitByTags,
            line_length_text: "8".to_owned(),
            pixels_x_text: String::new(),
            pixels_y_text: String::new(),
            regions_x_text: "1".to_owned(),
            regions_y_text: "1".to_owned(),
            last_edited: ImportImageField::Pixels,
            syncing: false,
            max_canvas_w: 4096,
            max_canvas_h: 4096,
            max_frames: 4096,
            preview_w: 0,
            preview_h: 0,
            preview_rgba: Vec::new(),
            preview_grid: (1, 1),
            preview_generation: 0,
            error: None,
        }
    }
}

impl ImportAsepriteView {
    pub fn seed_single_canvas_defaults(&mut self, frame_w: u32, frame_h: u32) {
        let fw = frame_w.max(1);
        let fh = frame_h.max(1);
        self.pixels_x_text = fw.to_string();
        self.pixels_y_text = fh.to_string();
        let (rw, rh) = crate::core::import::regions_from_pixels(fw, fh, fw, fh);
        self.regions_x_text = rw.to_string();
        self.regions_y_text = rh.to_string();
        self.last_edited = ImportImageField::Pixels;
        self.syncing = false;
    }

    pub fn effective_single_canvas_grid(&self) -> Result<(u32, u32, u32, u32), String> {
        let parsed = self
            .parsed
            .as_ref()
            .ok_or_else(|| "No Aseprite file loaded".to_owned())?;
        let fw = parsed.frame_w.max(1);
        let fh = parsed.frame_h.max(1);
        let (pw, ph, rw, rh) = match self.last_edited {
            ImportImageField::Pixels => {
                let (pw, ph) = parse_pair(&self.pixels_x_text, &self.pixels_y_text)
                    .ok_or_else(|| "Pixel split X and Y must be at least 1".to_owned())?;
                let (rw, rh) = crate::core::import::regions_from_pixels(fw, fh, pw, ph);
                (pw, ph, rw, rh)
            }
            ImportImageField::Regions => {
                let (rw, rh) = parse_pair(&self.regions_x_text, &self.regions_y_text)
                    .ok_or_else(|| "Region split X and Y must be at least 1".to_owned())?;
                let (pw, ph) = crate::core::import::pixels_from_regions(fw, fh, rw, rh);
                (pw, ph, rw, rh)
            }
        };
        let count = u64::from(rw) * u64::from(rh);
        if count > self.max_frames as u64 {
            return Err(format!(
                "{count} regions exceed the {} region limit",
                self.max_frames
            ));
        }
        let (cw, ch) = crate::core::import::canvas_from_grid(rw, rh, pw, ph);
        if cw > self.max_canvas_w || ch > self.max_canvas_h {
            return Err(format!(
                "canvas {cw}×{ch} exceeds {}×{}",
                self.max_canvas_w, self.max_canvas_h
            ));
        }
        Ok((pw, ph, rw, rh))
    }

    pub fn effective_layout_mode(&self) -> Result<AsepriteLayoutMode, String> {
        match self.method {
            AsepriteImportMethod::SingleCanvas => {
                let (pw, ph, _, _) = self.effective_single_canvas_grid()?;
                Ok(AsepriteLayoutMode::SingleCanvas { px_w: pw, px_h: ph })
            }
            AsepriteImportMethod::TimelineCanvas => {
                let parsed = self
                    .parsed
                    .as_ref()
                    .ok_or_else(|| "No Aseprite file loaded".to_owned())?;
                let mode = match self.mode {
                    AsepriteUiMode::SingleRow => AsepriteLayoutMode::SingleRow,
                    AsepriteUiMode::SplitByTags => AsepriteLayoutMode::SplitByTags,
                    AsepriteUiMode::RowLength => {
                        let n: u32 = self
                            .line_length_text
                            .trim()
                            .parse()
                            .map_err(|_| "Row length must be a positive integer".to_owned())?;
                        if n == 0 {
                            return Err("Row length must be at least 1".to_owned());
                        }
                        AsepriteLayoutMode::RowLength(n)
                    }
                };
                let (cols, rows) = layout_grid(parsed, mode);
                let regions = (cols as u64).saturating_mul(rows as u64);
                if regions > self.max_frames as u64 {
                    return Err(format!(
                        "{regions} regions exceed the {} region limit",
                        self.max_frames
                    ));
                }
                let cw = (cols as u64).saturating_mul(parsed.frame_w.max(1) as u64);
                let ch = (rows as u64).saturating_mul(parsed.frame_h.max(1) as u64);
                if cw > self.max_canvas_w as u64 || ch > self.max_canvas_h as u64 {
                    return Err(format!(
                        "Sheet {cw}×{ch} px exceeds {}×{} limit (try Row length or Single canvas)",
                        self.max_canvas_w, self.max_canvas_h
                    ));
                }
                Ok(mode)
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
                    self.preview_grid = layout_grid(parsed, mode);
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
    Back,
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
    preview_viewport: crate::ui::panel_dock::preview::PreviewViewport,
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

    fn draw_left_pane(
        &mut self,
        ui: &mut egui::Ui,
        chrome: &PanelChrome,
    ) -> Option<ImportAsepriteEvent> {
        let mut emitted = None;
        let mut needs_refresh = false;

        let total_h = ui.available_height();
        let footer_h = 44.0;
        let top_h = (total_h - footer_h).max(120.0);

        ui.allocate_ui_with_layout(
            vec2(FORM_PANE_WIDTH, top_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                let mut view = self.host.view.borrow_mut();
                let (frame_count, frame_w, frame_h, tag_count, layer_count) = view
                    .parsed
                    .as_ref()
                    .map(|p| {
                        (
                            p.frames.len(),
                            p.frame_w,
                            p.frame_h,
                            p.tags.len(),
                            p.layers.len(),
                        )
                    })
                    .unwrap_or((0, 0, 0, 0, 0));

                // Compact document metadata card.
                let card_w = ui.available_width();
                egui::Frame::new()
                    .fill(chrome.colors.panel_header_bg32())
                    .stroke(egui::Stroke::new(1.0, chrome.colors.panel_border32()))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width((card_w - 18.0).max(120.0));
                        ui.strong(view.file_name.clone());
                        ui.add_space(2.0);
                        ui.weak(format!(
                            "{frame_count} frames · {frame_w}×{frame_h} px · {tag_count} tags · {layer_count} layers"
                        ));
                    });

                ui.add_space(10.0);
                ui.strong("Import method");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let is_timeline = view.method == AsepriteImportMethod::TimelineCanvas;
                    if chrome
                        .button(
                            ui,
                            "Timeline canvas",
                            ButtonStyle::toggled(is_timeline),
                        )
                        .clicked()
                        && !is_timeline
                    {
                        view.method = AsepriteImportMethod::TimelineCanvas;
                        needs_refresh = true;
                    }
                    let is_single = view.method == AsepriteImportMethod::SingleCanvas;
                    if chrome
                        .button(ui, "Single canvas", ButtonStyle::toggled(is_single))
                        .clicked()
                        && !is_single
                    {
                        view.method = AsepriteImportMethod::SingleCanvas;
                        needs_refresh = true;
                    }
                });

                ui.add_space(10.0);

                match view.method {
                    AsepriteImportMethod::TimelineCanvas => {
                        ui.strong("Timeline layout");
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let tags_enabled = tag_count > 0;
                            if chrome
                                .button(
                                    ui,
                                    "Split by tags",
                                    ButtonStyle {
                                        selected: view.mode == AsepriteUiMode::SplitByTags,
                                        enabled: tags_enabled,
                                        ..ButtonStyle::plain()
                                    },
                                )
                                .clicked()
                                && view.mode != AsepriteUiMode::SplitByTags
                            {
                                view.mode = AsepriteUiMode::SplitByTags;
                                needs_refresh = true;
                            }
                            if chrome
                                .button(
                                    ui,
                                    "Single row",
                                    ButtonStyle::toggled(view.mode == AsepriteUiMode::SingleRow),
                                )
                                .clicked()
                                && view.mode != AsepriteUiMode::SingleRow
                            {
                                view.mode = AsepriteUiMode::SingleRow;
                                needs_refresh = true;
                            }
                            if chrome
                                .button(
                                    ui,
                                    "Row length",
                                    ButtonStyle::toggled(view.mode == AsepriteUiMode::RowLength),
                                )
                                .clicked()
                                && view.mode != AsepriteUiMode::RowLength
                            {
                                view.mode = AsepriteUiMode::RowLength;
                                needs_refresh = true;
                            }
                        });
                        if view.mode == AsepriteUiMode::RowLength {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label("Columns per row:");
                                let resp = ui.add(
                                    egui::TextEdit::singleline(&mut view.line_length_text)
                                        .desired_width(AXIS_INPUT_WIDTH),
                                );
                                if resp.changed() {
                                    needs_refresh = true;
                                }
                            });
                        }
                        ui.add_space(8.0);
                        if let (Some(parsed), Ok(mode)) = (&view.parsed, view.effective_layout_mode())
                        {
                            let (cols, rows) = layout_grid(parsed, mode);
                            ui.weak(format!(
                                "Sheet: {}×{} regions ({}×{} px)",
                                cols,
                                rows,
                                cols * frame_w as usize,
                                rows * frame_h as usize
                            ));
                        }
                    }
                    AsepriteImportMethod::SingleCanvas => {
                        ui.weak("Imports frame 1 layers without timeline.");
                        ui.add_space(6.0);

                        ui.strong("Split by pixels");
                        ui.add_space(4.0);
                        let mut px_changed = false;
                        ui.horizontal(|ui| {
                            ui.label("W:");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut view.pixels_x_text)
                                        .desired_width(AXIS_INPUT_WIDTH),
                                )
                                .changed()
                            {
                                px_changed = true;
                            }
                            ui.add_space(6.0);
                            ui.label("H:");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut view.pixels_y_text)
                                        .desired_width(AXIS_INPUT_WIDTH),
                                )
                                .changed()
                            {
                                px_changed = true;
                            }
                        });
                        if px_changed && !view.syncing {
                            view.last_edited = ImportImageField::Pixels;
                            if let Some((pw, ph)) =
                                parse_pair(&view.pixels_x_text, &view.pixels_y_text)
                            {
                                let (rw, rh) = crate::core::import::regions_from_pixels(
                                    frame_w.max(1),
                                    frame_h.max(1),
                                    pw,
                                    ph,
                                );
                                view.syncing = true;
                                view.regions_x_text = rw.to_string();
                                view.regions_y_text = rh.to_string();
                                view.syncing = false;
                            }
                            needs_refresh = true;
                        }

                        ui.add_space(8.0);
                        ui.strong("Split by regions");
                        ui.add_space(4.0);
                        let mut reg_changed = false;
                        ui.horizontal(|ui| {
                            ui.label("X:");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut view.regions_x_text)
                                        .desired_width(AXIS_INPUT_WIDTH),
                                )
                                .changed()
                            {
                                reg_changed = true;
                            }
                            ui.add_space(6.0);
                            ui.label("Y:");
                            if ui
                                .add(
                                    egui::TextEdit::singleline(&mut view.regions_y_text)
                                        .desired_width(AXIS_INPUT_WIDTH),
                                )
                                .changed()
                            {
                                reg_changed = true;
                            }
                        });
                        if reg_changed && !view.syncing {
                            view.last_edited = ImportImageField::Regions;
                            if let Some((rw, rh)) =
                                parse_pair(&view.regions_x_text, &view.regions_y_text)
                            {
                                let (pw, ph) = crate::core::import::pixels_from_regions(
                                    frame_w.max(1),
                                    frame_h.max(1),
                                    rw,
                                    rh,
                                );
                                view.syncing = true;
                                view.pixels_x_text = pw.to_string();
                                view.pixels_y_text = ph.to_string();
                                view.syncing = false;
                            }
                            needs_refresh = true;
                        }

                        ui.add_space(8.0);
                        if let Ok((pw, ph, rw, rh)) = view.effective_single_canvas_grid() {
                            let (cw, ch) = crate::core::import::canvas_from_grid(rw, rh, pw, ph);
                            ui.weak(format!(
                                "Output: {}×{} regions ({}×{} px/region · {}×{} px)",
                                rw, rh, pw, ph, cw, ch
                            ));
                        }
                    }
                }

                if needs_refresh {
                    view.refresh_preview();
                }

                if let Some(err) = &view.error {
                    ui.add_space(8.0);
                    ui.colored_label(ui.visuals().error_fg_color, err);
                }
            },
        );

        let view_snap = self.host.view.borrow().clone();
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.set_width(FORM_PANE_WIDTH);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let can_import =
                    view_snap.effective_layout_mode().is_ok() && view_snap.parsed.is_some();
                if chrome
                    .button(
                        ui,
                        "Import",
                        ButtonStyle {
                            selected: true,
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
                ui.add_space(6.0);
                if chrome
                    .button(ui, "Back", ButtonStyle::plain())
                    .on_hover_text("Return to file selection")
                    .clicked()
                {
                    emitted = Some(ImportAsepriteEvent::Back);
                }
                ui.add_space(6.0);
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    emitted = Some(ImportAsepriteEvent::Cancel);
                }
            });
        });

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(ImportAsepriteEvent::Cancel);
        } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            if let Ok(mode) = view_snap.effective_layout_mode() {
                emitted = Some(ImportAsepriteEvent::Import(mode));
            }
        }
        emitted
    }

    fn draw_right_preview(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let view_snap = self.host.view.borrow().clone();
        let avail = ui.available_size();
        let (box_rect, _) =
            ui.allocate_exact_size(vec2(avail.x.max(140.0), avail.y.max(140.0)), egui::Sense::hover());

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
        if view_snap.preview_w > 0 && view_snap.preview_h > 0 {
            painter.text(
                header_rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                format!("{} × {} px", view_snap.preview_w, view_snap.preview_h),
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }

        self.preview_viewport
            .sync_generation(view_snap.preview_generation);
        let interact_resp = ui.interact(
            canvas_area,
            ui.id().with("import-aseprite-preview-viewport"),
            egui::Sense::click_and_drag(),
        );
        self.preview_viewport
            .handle_interaction(ui, canvas_area, &interact_resp);

        if let Some(tex_id) = self.ensure_preview_texture(ui.ctx(), &view_snap) {
            let aspect = view_snap.preview_w as f32 / view_snap.preview_h.max(1) as f32;
            let mut draw_w = canvas_area.width().max(1.0);
            let mut draw_h = draw_w / aspect;
            if draw_h > canvas_area.height().max(1.0) {
                draw_h = canvas_area.height().max(1.0);
                draw_w = draw_h * aspect;
            }
            let base_rect =
                egui::Rect::from_center_size(canvas_area.center(), vec2(draw_w.max(1.0), draw_h.max(1.0)));
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

            // Overlay region split grid lines.
            let (cols, rows) = view_snap.preview_grid;
            let grid_color = chrome.colors.selection_stroke_color32().gamma_multiply(0.55);
            let grid_stroke = egui::Stroke::new(1.0, grid_color);
            if cols > 1 {
                for c in 1..cols {
                    let x = img_rect.min.x + img_rect.width() * (c as f32 / cols as f32);
                    clipped.line_segment(
                        [pos2(x, img_rect.min.y), pos2(x, img_rect.max.y)],
                        grid_stroke,
                    );
                }
            }
            if rows > 1 {
                for r in 1..rows {
                    let y = img_rect.min.y + img_rect.height() * (r as f32 / rows as f32);
                    clipped.line_segment(
                        [pos2(img_rect.min.x, y), pos2(img_rect.max.x, y)],
                        grid_stroke,
                    );
                }
            }

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

impl Component for ImportAsepriteComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let mut emitted = None;
        let total_h = ui.available_height().max(260.0);
        ui.horizontal_top(|ui| {
            ui.set_min_height(total_h);
            ui.allocate_ui_with_layout(
                vec2(FORM_PANE_WIDTH, total_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    emitted = self.draw_left_pane(ui, chrome);
                },
            );
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.set_min_height(total_h);
                self.draw_right_preview(ui, chrome);
            });
        });

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
        preview_viewport: crate::ui::panel_dock::preview::PreviewViewport::new(),
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

    #[test]
    fn single_canvas_mode_splits_first_frame_by_pixels_or_regions() {
        let mut view = ImportAsepriteView::default();
        view.parsed = Some(ParsedAseprite {
            frame_w: 64,
            frame_h: 32,
            layers: Vec::new(),
            frames: Vec::new(),
            tags: Vec::new(),
            palette: None,
        });
        view.method = AsepriteImportMethod::SingleCanvas;
        view.seed_single_canvas_defaults(64, 32);
        assert_eq!(
            view.effective_layout_mode().unwrap(),
            AsepriteLayoutMode::SingleCanvas { px_w: 64, px_h: 32 }
        );

        view.last_edited = ImportImageField::Regions;
        view.regions_x_text = "4".to_owned();
        view.regions_y_text = "2".to_owned();
        assert_eq!(
            view.effective_layout_mode().unwrap(),
            AsepriteLayoutMode::SingleCanvas { px_w: 16, px_h: 16 }
        );
    }
}
