//! Floating "Import Banner" dock panel (id 218).
//!
//! Opened after picking an image from the Select Banner panel's "Import Banner"
//! action. Follows the same two-column orientation as Import Image / Image 2 Pixel:
//! - left static controls: Banner Name input + Cut (8:1) / Full Image toggle;
//! - right interactive preview: zoomable & pannable image with a draggable 8:1
//!   cut border and darkened shade outside the cut region;
//! - pinned footer: Import (crops & saves PNG into `banners/`) and Cancel.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui::{pos2, vec2, Color32, Rect, Sense, Stroke};

use crate::ui::panel_dock::nest::{nest_is_usable, nest_viewport};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import Banner floating panel.
pub const IMPORT_BANNER_PANEL_ID: u64 = 218;

const TITLE: &str = "Import Banner";
const MIN_SIZE: egui::Vec2 = vec2(620.0, 420.0);

/// Whether the imported banner is cropped to an 8:1 strip or uses the full image.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BannerCropMode {
    #[default]
    Cut8x1,
    FullImage,
}

/// Actions emitted by the Import Banner panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportBannerEvent {
    Import {
        name: String,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Cancel,
}

/// Computes the largest centered 8:1 crop rectangle `(x, y, w, h)` fitting inside
/// `src_w × src_h`.
pub fn compute_default_8x1_crop(src_w: u32, src_h: u32) -> (u32, u32, u32, u32) {
    let sw = src_w.max(1);
    let sh = src_h.max(1);
    let (cw, ch) = if (sw as f32) / (sh as f32) >= 8.0 {
        let ch = sh;
        let cw = (sh.saturating_mul(8)).min(sw).max(1);
        (cw, ch)
    } else {
        let cw = sw;
        let ch = ((sw as f32 / 8.0).round() as u32).clamp(1, sh);
        (cw, ch)
    };
    let cx = (sw.saturating_sub(cw)) / 2;
    let cy = (sh.saturating_sub(ch)) / 2;
    (cx, cy, cw, ch)
}

/// Extracts the sub-rectangle `(x, y, w, h)` from a tightly-packed RGBA8 buffer.
pub fn crop_rgba_region(
    src_w: u32,
    src_h: u32,
    src_rgba: &[u8],
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let sw = src_w as usize;
    let sh = src_h as usize;
    let x0 = (x as usize).min(sw);
    let y0 = (y as usize).min(sh);
    let cw = (w as usize).min(sw.saturating_sub(x0));
    let ch = (h as usize).min(sh.saturating_sub(y0));
    let mut out = vec![0u8; cw * ch * 4];
    for row in 0..ch {
        let src_start = ((y0 + row) * sw + x0) * 4;
        let src_end = src_start + cw * 4;
        let dst_start = row * cw * 4;
        if src_end <= src_rgba.len() {
            out[dst_start..dst_start + cw * 4].copy_from_slice(&src_rgba[src_start..src_end]);
        }
    }
    out
}

/// Validates and normalizes a user-entered banner name into `<stem>.png`.
pub fn sanitize_banner_filename(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Enter a banner name first.".to_owned());
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains("..") {
        return Err("Banner name cannot contain path separators.".to_owned());
    }
    let stem = trimmed
        .strip_suffix(".png")
        .or_else(|| trimmed.strip_suffix(".PNG"))
        .unwrap_or(trimmed)
        .trim();
    if stem.is_empty() {
        return Err("Enter a banner name first.".to_owned());
    }
    Ok(format!("{stem}.png"))
}

/// Writes an RGBA8 banner image as PNG into `dir/<filename>`.
pub fn save_banner_png(
    dir: &Path,
    filename: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create banners directory: {e}"))?;
    let target = dir.join(filename);
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec())
        .ok_or_else(|| "Invalid banner image buffer dimensions".to_owned())?;
    img.save_with_format(&target, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to save banner PNG: {e}"))?;
    Ok(target)
}

pub struct ImportBannerHost {
    pub banner_name: String,
    pub src_w: u32,
    pub src_h: u32,
    pub src_rgba: Vec<u8>,
    pub crop_mode: BannerCropMode,
    /// Cut rectangle position (floating-point for smooth sub-pixel drag) and size in image pixels.
    pub cut_pos: egui::Vec2,
    pub cut_w: u32,
    pub cut_h: u32,
    pub preview_zoom: f32,
    pub preview_pan: egui::Vec2,
    dragging_cut: bool,
    pub error: Option<String>,
    source_texture: Option<egui::TextureHandle>,
    pub events: Vec<ImportBannerEvent>,
}

impl Default for ImportBannerHost {
    fn default() -> Self {
        Self {
            banner_name: String::from("custom_banner"),
            src_w: 1,
            src_h: 1,
            src_rgba: vec![0, 0, 0, 255],
            crop_mode: BannerCropMode::Cut8x1,
            cut_pos: egui::Vec2::ZERO,
            cut_w: 1,
            cut_h: 1,
            preview_zoom: 1.0,
            preview_pan: egui::Vec2::ZERO,
            dragging_cut: false,
            error: None,
            source_texture: None,
            events: Vec::new(),
        }
    }
}

impl ImportBannerHost {
    pub fn seed(&mut self, default_name: String, src_w: u32, src_h: u32, src_rgba: Vec<u8>) {
        self.banner_name = default_name;
        self.src_w = src_w.max(1);
        self.src_h = src_h.max(1);
        self.src_rgba = src_rgba;
        self.crop_mode = BannerCropMode::Cut8x1;
        let (cx, cy, cw, ch) = compute_default_8x1_crop(self.src_w, self.src_h);
        self.cut_pos = vec2(cx as f32, cy as f32);
        self.cut_w = cw;
        self.cut_h = ch;
        self.preview_zoom = 1.0;
        self.preview_pan = egui::Vec2::ZERO;
        self.dragging_cut = false;
        self.error = None;
        self.source_texture = None;
        self.events.clear();
    }

    pub fn effective_crop_rect(&self) -> (u32, u32, u32, u32) {
        match self.crop_mode {
            BannerCropMode::FullImage => (0, 0, self.src_w, self.src_h),
            BannerCropMode::Cut8x1 => {
                let cw = self.cut_w.clamp(1, self.src_w);
                let ch = self.cut_h.clamp(1, self.src_h);
                let max_x = self.src_w.saturating_sub(cw);
                let max_y = self.src_h.saturating_sub(ch);
                let cx = (self.cut_pos.x.round() as i64).clamp(0, max_x as i64) as u32;
                let cy = (self.cut_pos.y.round() as i64).clamp(0, max_y as i64) as u32;
                (cx, cy, cw, ch)
            }
        }
    }

    pub fn take_events(&mut self) -> Vec<ImportBannerEvent> {
        std::mem::take(&mut self.events)
    }

    fn ensure_texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if self.source_texture.is_none() {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [self.src_w as usize, self.src_h as usize],
                &self.src_rgba,
            );
            self.source_texture = Some(ctx.load_texture(
                "import_banner_source",
                image,
                egui::TextureOptions::NEAREST,
            ));
        }
        self.source_texture.as_ref().unwrap().id()
    }
}

struct ImportBannerPanelContent {
    host: Rc<RefCell<ImportBannerHost>>,
}

impl ImportBannerPanelContent {
    fn render_body(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let mut host = self.host.borrow_mut();
        let tex_id = host.ensure_texture(ui.ctx());

        let total = ui.available_size();
        let footer_h = 38.0;
        let body_h = (total.y - footer_h).max(160.0);
        let controls_w = (total.x * 0.34).clamp(210.0, 260.0);

        ui.allocate_ui_with_layout(
            vec2(total.x, body_h),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                // Left column: static controls.
                ui.allocate_ui_with_layout(
                    vec2(controls_w, body_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.strong("Banner Name");
                        ui.add_space(4.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut host.banner_name)
                                .hint_text("my_banner")
                                .desired_width((controls_w - 8.0).max(120.0)),
                        );
                        ui.weak("Saved as PNG in banners/");

                        ui.add_space(12.0);
                        ui.strong("Crop Mode");
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            if chrome
                                .button(
                                    ui,
                                    "Cut (8:1)",
                                    ButtonStyle::toggled(host.crop_mode == BannerCropMode::Cut8x1),
                                )
                                .on_hover_text("Crop an 8:1 strip; drag the border in the preview")
                                .clicked()
                            {
                                host.crop_mode = BannerCropMode::Cut8x1;
                            }
                            if chrome
                                .button(
                                    ui,
                                    "Full Image",
                                    ButtonStyle::toggled(
                                        host.crop_mode == BannerCropMode::FullImage,
                                    ),
                                )
                                .on_hover_text("Use the entire image without cropping")
                                .clicked()
                            {
                                host.crop_mode = BannerCropMode::FullImage;
                            }
                        });

                        ui.add_space(12.0);
                        let (cx, cy, cw, ch) = host.effective_crop_rect();
                        ui.strong("Info");
                        ui.add_space(4.0);
                        ui.weak(format!("Source: {} × {} px", host.src_w, host.src_h));
                        ui.weak(format!("Output: {cw} × {ch} px"));
                        if host.crop_mode == BannerCropMode::Cut8x1 {
                            ui.weak(format!("Offset: ({cx}, {cy})"));
                            ui.add_space(6.0);
                            ui.weak("Left-drag border to move.");
                        }
                        ui.weak("Scroll to zoom · Middle/Right-drag to pan.");

                        if let Some(err) = &host.error {
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(err).color(ui.visuals().error_fg_color),
                            );
                        }
                    },
                );

                ui.separator();

                // Right column: zoomable / pannable preview with draggable 8:1 cut border + shade.
                let preview_w = ui.available_width().max(120.0);
                ui.allocate_ui_with_layout(
                    vec2(preview_w, body_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.weak("Preview");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .small_button("Fit")
                                        .on_hover_text("Reset preview zoom and pan")
                                        .clicked()
                                    {
                                        host.preview_zoom = 1.0;
                                        host.preview_pan = egui::Vec2::ZERO;
                                    }
                                    ui.weak(format!(
                                        "{}%",
                                        (host.preview_zoom * 100.0).round() as i32
                                    ));
                                },
                            );
                        });

                        let well_h = (ui.available_height() - 4.0).max(120.0);
                        let (well_rect, well_resp) = ui.allocate_exact_size(
                            vec2(ui.available_width(), well_h),
                            Sense::click_and_drag(),
                        );
                        ui.painter()
                            .rect_filled(well_rect, 2.0, chrome.colors.list_bg32());

                        let sw = host.src_w as f32;
                        let sh = host.src_h as f32;
                        let pad = 12.0;
                        let avail_w = (well_rect.width() - pad * 2.0).max(1.0);
                        let avail_h = (well_rect.height() - pad * 2.0).max(1.0);
                        let base_scale = (avail_w / sw).min(avail_h / sh);

                        // Scroll zoom around pointer (exponential in smooth_scroll_delta
                        // so sub-frames sum cleanly: ~50pt / 1 wheel notch => ~1.11x zoom).
                        if well_resp.hovered() {
                            let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
                            if scroll_y.abs() > 0.01 {
                                let factor = 2.0_f32.powf((scroll_y / 320.0).clamp(-0.25, 0.25));
                                let old_zoom = host.preview_zoom;
                                let new_zoom = (old_zoom * factor).clamp(0.25, 16.0);
                                if (new_zoom - old_zoom).abs() > 1e-4 {
                                    if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
                                        let rel = pointer - (well_rect.center() + host.preview_pan);
                                        let ratio = new_zoom / old_zoom;
                                        host.preview_pan -= rel * (ratio - 1.0);
                                    }
                                    host.preview_zoom = new_zoom;
                                }
                            }
                        }

                        let scale = base_scale * host.preview_zoom;
                        let img_rect = Rect::from_center_size(
                            well_rect.center() + host.preview_pan,
                            vec2(sw * scale, sh * scale),
                        );
                        // Drag handling:
                        // - Left drag on image in Cut8x1 mode moves the cut border.
                        // - Middle/Right drag (or Left drag in FullImage mode) pans the preview.
                        let primary_down = ui.input(|i| i.pointer.primary_down());
                        let secondary_or_middle =
                            ui.input(|i| i.pointer.secondary_down() || i.pointer.middle_down());
                        if well_resp.drag_started() {
                            if primary_down
                                && host.crop_mode == BannerCropMode::Cut8x1
                                && well_resp
                                    .interact_pointer_pos()
                                    .is_some_and(|p| img_rect.contains(p))
                            {
                                host.dragging_cut = true;
                            } else {
                                host.dragging_cut = false;
                            }
                        }
                        if well_resp.dragged() {
                            let delta = well_resp.drag_delta();
                            if host.dragging_cut && primary_down {
                                let max_x = host.src_w.saturating_sub(host.cut_w) as f32;
                                let max_y = host.src_h.saturating_sub(host.cut_h) as f32;
                                host.cut_pos.x = (host.cut_pos.x + delta.x / scale).clamp(0.0, max_x);
                                host.cut_pos.y = (host.cut_pos.y + delta.y / scale).clamp(0.0, max_y);
                            } else if secondary_or_middle || !host.dragging_cut {
                                host.preview_pan += delta;
                            }
                        }
                        if well_resp.drag_stopped() || !primary_down {
                            host.dragging_cut = false;
                        }

                        // Recompute cut_screen_rect after potential drag update so paint has zero lag.
                        let (cx, cy, cw, ch) = host.effective_crop_rect();
                        let cut_screen_rect = Rect::from_min_size(
                            pos2(
                                img_rect.left() + cx as f32 * scale,
                                img_rect.top() + cy as f32 * scale,
                            ),
                            vec2(cw as f32 * scale, ch as f32 * scale),
                        );

                        let painter = ui.painter_at(well_rect);
                        painter.image(
                            tex_id,
                            img_rect,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE, // NB: neutral texture tint
                        );

                        // Shade outside the cut border (derived from active theme token).
                        let shade = chrome.colors.clear_color32().gamma_multiply(0.60);
                        // Top strip
                        if cut_screen_rect.top() > img_rect.top() {
                            painter.rect_filled(
                                Rect::from_min_max(
                                    img_rect.min,
                                    pos2(img_rect.right(), cut_screen_rect.top()),
                                ),
                                0.0,
                                shade,
                            );
                        }
                        // Bottom strip
                        if cut_screen_rect.bottom() < img_rect.bottom() {
                            painter.rect_filled(
                                Rect::from_min_max(
                                    pos2(img_rect.left(), cut_screen_rect.bottom()),
                                    img_rect.max,
                                ),
                                0.0,
                                shade,
                            );
                        }
                        // Left strip
                        if cut_screen_rect.left() > img_rect.left() {
                            painter.rect_filled(
                                Rect::from_min_max(
                                    pos2(img_rect.left(), cut_screen_rect.top()),
                                    pos2(cut_screen_rect.left(), cut_screen_rect.bottom()),
                                ),
                                0.0,
                                shade,
                            );
                        }
                        // Right strip
                        if cut_screen_rect.right() < img_rect.right() {
                            painter.rect_filled(
                                Rect::from_min_max(
                                    pos2(cut_screen_rect.right(), cut_screen_rect.top()),
                                    pos2(img_rect.right(), cut_screen_rect.bottom()),
                                ),
                                0.0,
                                shade,
                            );
                        }

                        // Crisp cut border outline.
                        painter.rect_stroke(
                            cut_screen_rect,
                            0.0,
                            Stroke::new(2.0, chrome.colors.selection_stroke_color32()),
                            egui::StrokeKind::Inside,
                        );
                    },
                );
            },
        );

        // Pinned footer bar.
        ui.separator();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    host.events.push(ImportBannerEvent::Cancel);
                }
                if chrome.button(ui, "Import", ButtonStyle::plain()).clicked() {
                    match sanitize_banner_filename(&host.banner_name) {
                        Ok(filename) => {
                            let (cx, cy, cw, ch) = host.effective_crop_rect();
                            let cropped = crop_rgba_region(
                                host.src_w,
                                host.src_h,
                                &host.src_rgba,
                                cx,
                                cy,
                                cw,
                                ch,
                            );
                            host.error = None;
                            host.events.push(ImportBannerEvent::Import {
                                name: filename,
                                width: cw,
                                height: ch,
                                rgba: cropped,
                            });
                        }
                        Err(msg) => {
                            host.error = Some(msg);
                        }
                    }
                }
            });
        });
    }
}

impl PanelContent for ImportBannerPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let full = ui.available_rect_before_wrap();
        ui.set_min_size(full.size());
        let viewport = nest_viewport(full);
        if !nest_is_usable(viewport) {
            return;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(viewport), |inner| {
            inner.set_clip_rect(viewport.intersect(inner.clip_rect()));
            self.render_body(inner, chrome);
        });
    }
}

pub fn import_banner_panel_spec(
    host: Rc<RefCell<ImportBannerHost>>,
    floating_rect: egui::Rect,
) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(IMPORT_BANNER_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(TITLE, false, MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ImportBannerPanelContent { host }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_8x1_crop_fits_standard_and_ultrawide_images() {
        // 800x400 -> 8:1 strip is 800x100 centered at y=150.
        assert_eq!(compute_default_8x1_crop(800, 400), (0, 150, 800, 100));
        // 1600x100 (wider than 8:1) -> 800x100 centered at x=400.
        assert_eq!(compute_default_8x1_crop(1600, 100), (400, 0, 800, 100));
    }

    #[test]
    fn crop_rgba_region_extracts_exact_pixels() {
        let mut src = vec![0u8; 4 * 4 * 4];
        // Paint pixel (2, 1) red.
        let idx = (1 * 4 + 2) * 4;
        src[idx..idx + 4].copy_from_slice(&[255, 0, 0, 255]);
        let cropped = crop_rgba_region(4, 4, &src, 2, 1, 2, 1);
        assert_eq!(cropped.len(), 2 * 1 * 4);
        assert_eq!(&cropped[0..4], &[255, 0, 0, 255]);
        assert_eq!(&cropped[4..8], &[0, 0, 0, 0]);
    }

    #[test]
    fn sanitize_banner_filename_appends_png_and_rejects_invalid() {
        assert_eq!(sanitize_banner_filename("night_sky").unwrap(), "night_sky.png");
        assert_eq!(sanitize_banner_filename("night_sky.png").unwrap(), "night_sky.png");
        assert!(sanitize_banner_filename("   ").is_err());
        assert!(sanitize_banner_filename("../evil").is_err());
    }

    #[test]
    fn full_image_mode_covers_entire_image() {
        let mut host = ImportBannerHost::default();
        host.seed("test".to_owned(), 800, 400, vec![255; 800 * 400 * 4]);
        assert_eq!(host.effective_crop_rect(), (0, 150, 800, 100));
        host.crop_mode = BannerCropMode::FullImage;
        assert_eq!(host.effective_crop_rect(), (0, 0, 800, 400));
    }
}
