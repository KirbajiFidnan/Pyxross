//! Floating "Export Animation" dock panel (id 219).
//!
//! Replaces separate Sprite Sheet and GIF exports with one unified panel:
//! - Animation selector dropdown/button
//! - Three tabs in order: `Frames` | `Sprite Sheet` | `GIF`
//! - `Pixel scale` (`1×..=16×`) nearest-neighbor upscaling
//! - Right-hand live `Preview` with zoom & pan (`PreviewViewport`).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui::{pos2, vec2};

use crate::core::math::Rect2i;
use crate::ui::dock_save_as_panel::{
    scale_rgba_nearest, validate_name, with_extension, MAX_PIXEL_SCALE,
};
use crate::ui::dock_tool_property_panel::pro_slider;
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::preview::PreviewViewport;
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
    FLOATING_PANEL_CHROME_HEIGHT,
};

/// Dock id for the Export Animation floating panel.
pub const EXPORT_ANIMATION_PANEL_ID: u64 = 219;

const EXPORT_ANIMATION_TITLE: &str = "Export Animation";
const EXPORT_ANIMATION_MIN_SIZE: egui::Vec2 = vec2(700.0, 350.0 + FLOATING_PANEL_CHROME_HEIGHT);
const FORM_PANE_WIDTH: f32 = 350.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportAnimationTab {
    Frames,
    SpriteSheet,
    Gif,
}

#[derive(Clone, Debug)]
pub struct ExportAnimationFrame {
    pub rect: Rect2i,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

#[derive(Clone, Debug)]
pub struct ExportAnimationOption {
    pub name: String,
    pub frames: Vec<ExportAnimationFrame>,
    pub canvas_w: usize,
    pub canvas_h: usize,
    pub canvas_rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ExportAnimationView {
    pub animations: Vec<ExportAnimationOption>,
    /// `None` = "All animations"; `Some(idx)` = single animation at `idx`.
    pub selected_animation: Option<usize>,
    pub tab: ExportAnimationTab,
    pub single_row: bool,
    pub pixel_scale: u32,
    pub location: PathBuf,
    pub location_edit: String,
    pub synced_location: PathBuf,
    pub name: String,
    pub error: Option<String>,
    pub preview_generation: u64,
}

impl Default for ExportAnimationView {
    fn default() -> Self {
        Self {
            animations: Vec::new(),
            selected_animation: Some(0),
            tab: ExportAnimationTab::Frames,
            single_row: true,
            pixel_scale: 1,
            location: PathBuf::new(),
            location_edit: String::new(),
            synced_location: PathBuf::new(),
            name: String::new(),
            error: None,
            preview_generation: 0,
        }
    }
}

impl ExportAnimationView {
    /// Returns the animation used for the right-hand preview pane.
    pub fn preview_animation(&self) -> Option<&ExportAnimationOption> {
        match self.selected_animation {
            Some(idx) => self.animations.get(idx),
            None => self
                .animations
                .iter()
                .find(|a| !a.frames.is_empty())
                .or_else(|| self.animations.first()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportAnimationEvent {
    ExportFrames {
        targets: Vec<(usize, PathBuf)>,
        pixel_scale: u32,
    },
    ExportSpriteSheet {
        targets: Vec<(usize, PathBuf)>,
        single_row: bool,
        pixel_scale: u32,
    },
    ExportGif {
        targets: Vec<(usize, PathBuf)>,
        pixel_scale: u32,
    },
    BrowseLocation,
    Cancel,
}

#[derive(Clone, Default)]
pub struct ExportAnimationHost {
    pub view: Rc<RefCell<ExportAnimationView>>,
    pub events: Rc<RefCell<Vec<ExportAnimationEvent>>>,
}

impl ExportAnimationHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn take_events(&self) -> Vec<ExportAnimationEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

/// Sanitizes an animation name for use as a directory name.
pub fn sanitize_folder_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | '\0') { '_' } else { c })
        .collect();
    if cleaned.is_empty() {
        "Animation".to_owned()
    } else {
        cleaned
    }
}

/// Builds a single-row RGBA8 sprite sheet from `frames`.
pub fn build_single_row_sheet(frames: &[ExportAnimationFrame]) -> (usize, usize, Vec<u8>) {
    if frames.is_empty() {
        return (0, 0, Vec::new());
    }
    let total_w: usize = frames.iter().map(|f| f.width).sum();
    let max_h: usize = frames.iter().map(|f| f.height).max().unwrap_or(0);
    if total_w == 0 || max_h == 0 {
        return (0, 0, Vec::new());
    }
    let mut out = vec![0u8; total_w * max_h * 4];
    let mut x_off = 0usize;
    for f in frames {
        for y in 0..f.height {
            let src_row = y * f.width * 4;
            let dst_row = (y * total_w + x_off) * 4;
            let bytes = f.width * 4;
            if src_row + bytes <= f.rgba.len() && dst_row + bytes <= out.len() {
                out[dst_row..dst_row + bytes].copy_from_slice(&f.rgba[src_row..src_row + bytes]);
            }
        }
        x_off += f.width;
    }
    (total_w, max_h, out)
}

/// Crops the bounding box of `anim.frames` clamped to the canvas `(0..canvas_w, 0..canvas_h)`.
pub fn build_canvas_bounds_crop(anim: &ExportAnimationOption) -> (usize, usize, Vec<u8>) {
    if anim.frames.is_empty() || anim.canvas_w == 0 || anim.canvas_h == 0 {
        return (0, 0, Vec::new());
    }
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for f in &anim.frames {
        min_x = min_x.min(f.rect.x);
        min_y = min_y.min(f.rect.y);
        max_x = max_x.max(f.rect.right());
        max_y = max_y.max(f.rect.bottom());
    }
    let x0 = min_x.clamp(0, anim.canvas_w as i32) as usize;
    let y0 = min_y.clamp(0, anim.canvas_h as i32) as usize;
    let x1 = max_x.clamp(0, anim.canvas_w as i32) as usize;
    let y1 = max_y.clamp(0, anim.canvas_h as i32) as usize;
    if x1 <= x0 || y1 <= y0 {
        return (0, 0, Vec::new());
    }
    let w = x1 - x0;
    let h = y1 - y0;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let src_start = ((y0 + y) * anim.canvas_w + x0) * 4;
        let dst_start = y * w * 4;
        let len = w * 4;
        if src_start + len <= anim.canvas_rgba.len() {
            out[dst_start..dst_start + len]
                .copy_from_slice(&anim.canvas_rgba[src_start..src_start + len]);
        }
    }
    (w, h, out)
}

/// Validates and resolves the export event for the current view.
pub fn commit_export_animation(view: &ExportAnimationView) -> Result<ExportAnimationEvent, String> {
    if view.location.as_os_str().is_empty() || !view.location.is_dir() {
        return Err(format!(
            "'{}' is not an existing folder",
            view.location.display()
        ));
    }
    let pixel_scale = view.pixel_scale.clamp(1, MAX_PIXEL_SCALE);

    let items: Vec<(usize, String)> = match view.selected_animation {
        Some(idx) => {
            let anim = view
                .animations
                .get(idx)
                .ok_or_else(|| "No animation selected".to_owned())?;
            if anim.frames.is_empty() {
                return Err("Selected animation has no frames".to_owned());
            }
            let base = if view.name.trim().is_empty() {
                sanitize_folder_name(&anim.name)
            } else {
                validate_name(&view.name)?;
                view.name.trim().to_owned()
            };
            vec![(idx, base)]
        }
        None => {
            let list: Vec<(usize, String)> = view
                .animations
                .iter()
                .enumerate()
                .filter(|(_, a)| !a.frames.is_empty())
                .map(|(idx, a)| (idx, sanitize_folder_name(&a.name)))
                .collect();
            if list.is_empty() {
                return Err("No animations with frames to export".to_owned());
            }
            list
        }
    };

    match view.tab {
        ExportAnimationTab::Frames => {
            let mut targets = Vec::with_capacity(items.len());
            for (idx, base) in items {
                let dir = view.location.join(&base);
                if dir.is_file() {
                    return Err(format!("a file already exists at {}", dir.display()));
                }
                targets.push((idx, dir));
            }
            Ok(ExportAnimationEvent::ExportFrames {
                targets,
                pixel_scale,
            })
        }
        ExportAnimationTab::SpriteSheet => {
            let mut targets = Vec::with_capacity(items.len());
            for (idx, base) in items {
                let final_name = with_extension(&base, ".png");
                let path = Path::new(&view.location).join(final_name);
                if path.is_dir() {
                    return Err(format!("a directory already exists at {}", path.display()));
                }
                targets.push((idx, path));
            }
            Ok(ExportAnimationEvent::ExportSpriteSheet {
                targets,
                single_row: view.single_row,
                pixel_scale,
            })
        }
        ExportAnimationTab::Gif => {
            let mut targets = Vec::with_capacity(items.len());
            for (idx, base) in items {
                let final_name = with_extension(&base, ".gif");
                let path = Path::new(&view.location).join(final_name);
                if path.is_dir() {
                    return Err(format!("a directory already exists at {}", path.display()));
                }
                targets.push((idx, path));
            }
            Ok(ExportAnimationEvent::ExportGif {
                targets,
                pixel_scale,
            })
        }
    }
}

fn fit_rect_aspect(container: egui::Rect, w: usize, h: usize) -> egui::Rect {
    let aspect = (w.max(1) as f32) / (h.max(1) as f32);
    let mut draw_w = container.width().max(1.0);
    let mut draw_h = draw_w / aspect;
    if draw_h > container.height().max(1.0) {
        draw_h = container.height().max(1.0);
        draw_w = draw_h * aspect;
    }
    egui::Rect::from_center_size(container.center(), vec2(draw_w.max(1.0), draw_h.max(1.0)))
}

struct ExportAnimationComponent {
    host: ExportAnimationHost,
    preview_key: (u64, Option<usize>, ExportAnimationTab, bool),
    preview_viewport: PreviewViewport,
    single_tex: Option<(usize, usize, egui::TextureHandle)>,
    frame_texs: Vec<(usize, usize, egui::TextureHandle)>,
}

impl ExportAnimationComponent {
    fn new(host: ExportAnimationHost) -> Self {
        Self {
            host,
            preview_key: (u64::MAX, Some(usize::MAX), ExportAnimationTab::Frames, true),
            preview_viewport: PreviewViewport::new(),
            single_tex: None,
            frame_texs: Vec::new(),
        }
    }

    fn upload_rgba(
        ctx: &egui::Context,
        name: &str,
        w: usize,
        h: usize,
        rgba: &[u8],
    ) -> Option<(usize, usize, egui::TextureHandle)> {
        if w == 0 || h == 0 || rgba.len() != w * h * 4 {
            return None;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
        let tex = ctx.load_texture(name, image, egui::TextureOptions::NEAREST);
        Some((w, h, tex))
    }

    fn sync_preview_textures(&mut self, ctx: &egui::Context) {
        let view = self.host.view.borrow();
        let key = (
            view.preview_generation,
            view.selected_animation,
            view.tab,
            view.single_row,
        );
        if self.preview_key == key {
            return;
        }
        self.preview_key = key;
        self.preview_viewport.reset();
        self.single_tex = None;
        self.frame_texs.clear();

        let Some(anim) = view.preview_animation() else {
            return;
        };
        match view.tab {
            ExportAnimationTab::Frames | ExportAnimationTab::Gif => {
                for (idx, f) in anim.frames.iter().enumerate() {
                    if let Some(tex) = Self::upload_rgba(
                        ctx,
                        &format!("export-anim-frame-{idx}"),
                        f.width,
                        f.height,
                        &f.rgba,
                    ) {
                        self.frame_texs.push(tex);
                    }
                }
            }
            ExportAnimationTab::SpriteSheet => {
                let (w, h, rgba) = if view.single_row {
                    build_single_row_sheet(&anim.frames)
                } else {
                    build_canvas_bounds_crop(anim)
                };
                self.single_tex = Self::upload_rgba(ctx, "export-anim-sheet", w, h, &rgba);
            }
        }
    }

    fn draw_left_pane(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let (
            anim_names,
            mut selected_animation,
            mut tab,
            mut single_row,
            mut pixel_scale,
            location,
            mut location_edit,
            mut synced,
            mut name,
            error,
            base_dims,
            frame_count,
        ) = {
            let v = self.host.view.borrow();
            let names: Vec<(String, usize)> = v
                .animations
                .iter()
                .map(|a| (a.name.clone(), a.frames.len()))
                .collect();
            let anim = v.preview_animation();
            let fc = anim.map_or(0, |a| a.frames.len());
            let dims = match (v.tab, anim) {
                (ExportAnimationTab::Frames | ExportAnimationTab::Gif, Some(a)) => {
                    a.frames.first().map_or((0, 0), |f| (f.width, f.height))
                }
                (ExportAnimationTab::SpriteSheet, Some(a)) => {
                    if v.single_row {
                        let (w, h, _) = build_single_row_sheet(&a.frames);
                        (w, h)
                    } else {
                        let (w, h, _) = build_canvas_bounds_crop(a);
                        (w, h)
                    }
                }
                _ => (0, 0),
            };
            (
                names,
                v.selected_animation,
                v.tab,
                v.single_row,
                v.pixel_scale.clamp(1, MAX_PIXEL_SCALE),
                v.location.clone(),
                v.location_edit.clone(),
                v.synced_location.clone(),
                v.name.clone(),
                v.error.clone(),
                dims,
                fc,
            )
        };

        if location != synced {
            location_edit = location.display().to_string();
            synced = location.clone();
        }

        let mut browse_clicked = false;
        let mut export_clicked = false;
        let mut cancel_clicked = false;

        // 1. Animation selector button ("All animations" + each animation)
        ui.strong("Animation:");
        ui.add_space(4.0);
        let non_empty_count = anim_names.iter().filter(|(_, c)| *c > 0).count();
        let selected_label = match selected_animation {
            None => format!("All animations ({non_empty_count})"),
            Some(idx) => anim_names
                .get(idx)
                .map(|(n, c)| format!("{n} ({c} frame(s))"))
                .unwrap_or_else(|| "None".to_owned()),
        };
        egui::ComboBox::from_id_salt("export-animation-selector")
            .width(ui.available_width())
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        selected_animation.is_none(),
                        format!("All animations ({non_empty_count})"),
                    )
                    .clicked()
                {
                    selected_animation = None;
                }
                for (idx, (aname, count)) in anim_names.iter().enumerate() {
                    let label = format!("{aname} ({count} frame(s))");
                    if ui
                        .selectable_label(selected_animation == Some(idx), label)
                        .clicked()
                    {
                        selected_animation = Some(idx);
                    }
                }
            });

        ui.add_space(10.0);

        // 2. Three tabs in exact order: Frames - Sprite Sheet - GIF
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (t, label) in [
                (ExportAnimationTab::Frames, "Frames"),
                (ExportAnimationTab::SpriteSheet, "Sprite Sheet"),
                (ExportAnimationTab::Gif, "GIF"),
            ] {
                if chrome
                    .button(
                        ui,
                        label,
                        ButtonStyle {
                            selected: tab == t,
                            ..ButtonStyle::plain()
                        },
                    )
                    .clicked()
                {
                    tab = t;
                }
            }
        });

        ui.add_space(10.0);

        // 3. Save path + Browse
        ui.strong("Save path:");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                browse_clicked = chrome
                    .button(ui, "Browse", ButtonStyle::plain())
                    .on_hover_text("Choose the destination folder")
                    .clicked();
                ui.add_space(6.0);
                ui.add(
                    egui::TextEdit::singleline(&mut location_edit)
                        .hint_text("destination folder")
                        .desired_width(ui.available_width()),
                );
            });
        });

        ui.add_space(8.0);

        // 4. Tab-specific options
        let single_anim_name = selected_animation
            .and_then(|idx| anim_names.get(idx))
            .map(|(n, _)| sanitize_folder_name(n))
            .unwrap_or_else(|| "Animation".to_owned());

        if selected_animation.is_some() {
            let ext_badge = match tab {
                ExportAnimationTab::Frames => None,
                ExportAnimationTab::SpriteSheet => Some(".png"),
                ExportAnimationTab::Gif => Some(".gif"),
            };
            let label = if tab == ExportAnimationTab::Frames {
                "Folder name:"
            } else {
                "Document name:"
            };
            ui.strong(label);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(ext) = ext_badge {
                        let _ = chrome.button(
                            ui,
                            ext,
                            ButtonStyle {
                                selected: true,
                                ..ButtonStyle::plain()
                            },
                        );
                        ui.add_space(6.0);
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .hint_text(&single_anim_name)
                            .desired_width(ui.available_width()),
                    );
                });
            });
            ui.add_space(6.0);
        }

        match tab {
            ExportAnimationTab::Frames => {
                if selected_animation.is_none() {
                    ui.weak(format!(
                        "Creates a folder per animation ({non_empty_count} folder(s))"
                    ));
                } else {
                    let folder = if name.trim().is_empty() {
                        single_anim_name
                    } else {
                        name.trim().to_owned()
                    };
                    ui.weak(format!(
                        "Exports {frame_count} PNG(s) into \"{folder}/\""
                    ));
                }
            }
            ExportAnimationTab::SpriteSheet => {
                if selected_animation.is_none() {
                    ui.weak(format!(
                        "Exports {non_empty_count} .png file(s) named after each animation"
                    ));
                    ui.add_space(4.0);
                }
                ui.checkbox(&mut single_row, "Single row").on_hover_text(
                    "When checked, arranges frames in one horizontal row; when unchecked, exports the animation's canvas bounds directly",
                );
            }
            ExportAnimationTab::Gif => {
                if selected_animation.is_none() {
                    ui.weak(format!(
                        "Exports {non_empty_count} .gif file(s) named after each animation"
                    ));
                }
            }
        }

        // 5. Pixel scale (required on all visual export modes)
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.strong("Pixel scale:");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(format!(
                    "{} × {} px",
                    base_dims.0.saturating_mul(pixel_scale as usize),
                    base_dims.1.saturating_mul(pixel_scale as usize)
                ));
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_sized(
                vec2(46.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut pixel_scale)
                    .range(1..=MAX_PIXEL_SCALE)
                    .suffix("×"),
            );
            let mut scale_f = pixel_scale as f32;
            let slider_w = ui.available_width().max(60.0);
            let slider = ui
                .allocate_ui(vec2(slider_w, 18.0), |ui| {
                    pro_slider(
                        ui,
                        "export-anim-pixel-scale",
                        &mut scale_f,
                        1.0..=(MAX_PIXEL_SCALE as f32),
                        false,
                    )
                })
                .inner;
            if slider.changed() {
                pixel_scale = (scale_f.round() as u32).clamp(1, MAX_PIXEL_SCALE);
            }
        });

        if let Some(err) = &error {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome
                    .button(
                        ui,
                        "Export",
                        ButtonStyle {
                            selected: true,
                            ..ButtonStyle::plain()
                        },
                    )
                    .clicked()
                {
                    export_clicked = true;
                }
                ui.add_space(6.0);
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    cancel_clicked = true;
                }
            });
        });

        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed =
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        let mut view = self.host.view.borrow_mut();
        view.selected_animation = selected_animation;
        view.tab = tab;
        view.single_row = single_row;
        view.pixel_scale = pixel_scale.clamp(1, MAX_PIXEL_SCALE);
        view.location_edit = location_edit;
        view.synced_location = synced;
        view.name = name;

        if browse_clicked {
            view.location = PathBuf::from(view.location_edit.trim());
            view.synced_location = view.location.clone();
            drop(view);
            self.host
                .events
                .borrow_mut()
                .push(ExportAnimationEvent::BrowseLocation);
            return;
        }

        if export_clicked || enter_pressed {
            view.location = PathBuf::from(view.location_edit.trim());
            view.synced_location = view.location.clone();
            match commit_export_animation(&view) {
                Ok(ev) => {
                    view.error = None;
                    self.host.events.borrow_mut().push(ev);
                }
                Err(msg) => view.error = Some(msg),
            }
        } else if cancel_clicked || escape_pressed {
            self.host
                .events
                .borrow_mut()
                .push(ExportAnimationEvent::Cancel);
        }
    }

    fn draw_right_preview(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.sync_preview_textures(ui.ctx());
        let (tab, caption) = {
            let v = self.host.view.borrow();
            let scale = v.pixel_scale.clamp(1, MAX_PIXEL_SCALE) as usize;
            let anim = v.preview_animation();
            let cap = match (v.tab, anim) {
                (ExportAnimationTab::Frames, Some(a)) => {
                    let (fw, fh) = a.frames.first().map_or((0, 0), |f| (f.width, f.height));
                    format!(
                        "Preview · Frames: {} PNG(s) ({}×{} px)",
                        a.frames.len(),
                        fw * scale,
                        fh * scale
                    )
                }
                (ExportAnimationTab::SpriteSheet, Some(a)) => {
                    let (w, h) = if v.single_row {
                        let (w, h, _) = build_single_row_sheet(&a.frames);
                        (w, h)
                    } else {
                        let (w, h, _) = build_canvas_bounds_crop(a);
                        (w, h)
                    };
                    format!("Preview · Sprite Sheet: {} × {} px", w * scale, h * scale)
                }
                (ExportAnimationTab::Gif, Some(a)) => {
                    let (fw, fh) = a.frames.first().map_or((0, 0), |f| (f.width, f.height));
                    format!(
                        "Preview · GIF: {} frame(s) ({}×{} px)",
                        a.frames.len(),
                        fw * scale,
                        fh * scale
                    )
                }
                _ => "Preview".to_owned(),
            };
            (v.tab, cap)
        };

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
        let img_area = egui::Rect::from_min_max(
            pos2(inner.min.x, header_rect.max.y + 6.0),
            pos2(
                inner.max.x,
                (inner.max.y - footer_h - 4.0).max(header_rect.max.y + 12.0),
            ),
        );
        let footer_rect = egui::Rect::from_min_max(pos2(inner.min.x, img_area.max.y + 4.0), inner.max);

        painter.text(
            header_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            caption,
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );

        let interact_resp = ui.interact(
            img_area,
            ui.id().with("export-anim-preview-viewport"),
            egui::Sense::click_and_drag(),
        );
        self.preview_viewport
            .handle_interaction(ui, img_area, &interact_resp);

        let clipped = ui.painter_at(img_area);
        let uv = egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

        match tab {
            ExportAnimationTab::SpriteSheet => {
                if let Some((w, h, tex)) = &self.single_tex {
                    let base_dst = fit_rect_aspect(img_area, *w, *h);
                    let dst = self
                        .preview_viewport
                        .transform_rect(base_dst, img_area.center());
                    clipped.rect_filled(dst, 0.0, chrome.colors.panel_bg32());
                    clipped.image(tex.id(), dst, uv, crate::ui::theme::SKIN_TINT);
                    clipped.rect_stroke(
                        dst,
                        0.0,
                        egui::Stroke::new(1.0, chrome.colors.panel_border32()),
                        egui::StrokeKind::Outside,
                    );
                }
            }
            ExportAnimationTab::Frames | ExportAnimationTab::Gif => {
                let count = self.frame_texs.len();
                if count > 0 {
                    let gap = 8.0;
                    let slot_w = ((img_area.width() - gap * (count.saturating_sub(1) as f32))
                        / count as f32)
                        .max(20.0);
                    for (idx, (fw, fh, tex)) in self.frame_texs.iter().enumerate() {
                        let x0 = img_area.min.x + idx as f32 * (slot_w + gap);
                        let slot = egui::Rect::from_min_size(
                            pos2(x0, img_area.min.y),
                            vec2(slot_w, img_area.height().max(12.0)),
                        );
                        let base_dst = fit_rect_aspect(slot, *fw, *fh);
                        let dst = self
                            .preview_viewport
                            .transform_rect(base_dst, img_area.center());
                        clipped.rect_filled(dst, 0.0, chrome.colors.panel_bg32());
                        clipped.image(tex.id(), dst, uv, crate::ui::theme::SKIN_TINT);
                        clipped.rect_stroke(
                            dst,
                            0.0,
                            egui::Stroke::new(1.0, chrome.colors.panel_border32()),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
            }
        }

        self.preview_viewport.draw_footer(ui, chrome, footer_rect);
    }
}

impl Component for ExportAnimationComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let total_h = ui.available_height().max(260.0);
        ui.horizontal_top(|ui| {
            ui.set_min_height(total_h);
            ui.allocate_ui_with_layout(
                vec2(FORM_PANE_WIDTH, total_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    self.draw_left_pane(ui, chrome);
                },
            );
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.set_min_height(total_h);
                self.draw_right_preview(ui, chrome);
            });
        });
    }
}

struct ExportAnimationPanelContent {
    nest: NestContent,
}

impl PanelContent for ExportAnimationPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

pub fn export_animation_panel_spec(
    host: &ExportAnimationHost,
    floating_rect: egui::Rect,
) -> PanelSpec {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ExportAnimationComponent::new(host.clone()));
    let rect = egui::Rect::from_min_size(
        floating_rect.min,
        vec2(
            floating_rect.width().max(EXPORT_ANIMATION_MIN_SIZE.x),
            floating_rect.height().max(EXPORT_ANIMATION_MIN_SIZE.y),
        ),
    );
    PanelSpec {
        id: PanelId::new(EXPORT_ANIMATION_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(EXPORT_ANIMATION_TITLE, false, EXPORT_ANIMATION_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect: rect,
        content: Box::new(ExportAnimationPanelContent { nest }),
    }
}

/// Scales `rgba` by `pixel_scale` using nearest-neighbor and encodes as PNG bytes.
pub fn encode_scaled_png(
    w: usize,
    h: usize,
    rgba: &[u8],
    pixel_scale: u32,
) -> Result<Vec<u8>, String> {
    let (sw, sh, scaled) = scale_rgba_nearest(w, h, rgba, pixel_scale);
    crate::io::encode_png(sw, sh, &scaled).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_option() -> ExportAnimationOption {
        ExportAnimationOption {
            name: "Walk".to_owned(),
            frames: vec![
                ExportAnimationFrame {
                    rect: Rect2i::new(0, 0, 2, 2),
                    width: 2,
                    height: 2,
                    rgba: vec![255; 16],
                    delay_ms: 100,
                },
                ExportAnimationFrame {
                    rect: Rect2i::new(2, 0, 2, 2),
                    width: 2,
                    height: 2,
                    rgba: vec![128; 16],
                    delay_ms: 120,
                },
            ],
            canvas_w: 4,
            canvas_h: 4,
            canvas_rgba: vec![64; 4 * 4 * 4],
        }
    }

    #[test]
    fn single_row_sheet_and_canvas_bounds_crop_produce_expected_dimensions() {
        let opt = sample_option();
        let (sw, sh, sbytes) = build_single_row_sheet(&opt.frames);
        assert_eq!((sw, sh), (4, 2));
        assert_eq!(sbytes.len(), 4 * 2 * 4);

        let (cw, ch, cbytes) = build_canvas_bounds_crop(&opt);
        assert_eq!((cw, ch), (4, 2));
        assert_eq!(cbytes.len(), 4 * 2 * 4);
    }

    #[test]
    fn commit_export_animation_resolves_frames_folder_and_extensions() {
        let mut view = ExportAnimationView::default();
        view.animations.push(sample_option());
        view.location = std::env::temp_dir();
        view.pixel_scale = 3;

        // Default empty name falls back to animation name ("Walk") for all tabs.
        view.tab = ExportAnimationTab::Frames;
        assert_eq!(
            commit_export_animation(&view).unwrap(),
            ExportAnimationEvent::ExportFrames {
                targets: vec![(0, std::env::temp_dir().join("Walk"))],
                pixel_scale: 3,
            }
        );

        view.tab = ExportAnimationTab::SpriteSheet;
        assert_eq!(
            commit_export_animation(&view).unwrap(),
            ExportAnimationEvent::ExportSpriteSheet {
                targets: vec![(0, std::env::temp_dir().join("Walk.png"))],
                single_row: true,
                pixel_scale: 3,
            }
        );

        // Custom name overrides the animation name.
        view.name = "walk_sheet".to_owned();
        view.single_row = false;
        assert_eq!(
            commit_export_animation(&view).unwrap(),
            ExportAnimationEvent::ExportSpriteSheet {
                targets: vec![(0, std::env::temp_dir().join("walk_sheet.png"))],
                single_row: false,
                pixel_scale: 3,
            }
        );

        view.tab = ExportAnimationTab::Gif;
        view.name = "walk_anim".to_owned();
        assert_eq!(
            commit_export_animation(&view).unwrap(),
            ExportAnimationEvent::ExportGif {
                targets: vec![(0, std::env::temp_dir().join("walk_anim.gif"))],
                pixel_scale: 3,
            }
        );
    }

    #[test]
    fn commit_export_animation_all_animations_exports_each_into_own_target() {
        let mut view = ExportAnimationView::default();
        let mut second = sample_option();
        second.name = "Run".to_owned();
        view.animations.push(sample_option());
        view.animations.push(second);
        view.selected_animation = None;
        view.location = std::env::temp_dir();

        view.tab = ExportAnimationTab::Frames;
        assert_eq!(
            commit_export_animation(&view).unwrap(),
            ExportAnimationEvent::ExportFrames {
                targets: vec![
                    (0, std::env::temp_dir().join("Walk")),
                    (1, std::env::temp_dir().join("Run")),
                ],
                pixel_scale: 1,
            }
        );

        view.tab = ExportAnimationTab::SpriteSheet;
        assert_eq!(
            commit_export_animation(&view).unwrap(),
            ExportAnimationEvent::ExportSpriteSheet {
                targets: vec![
                    (0, std::env::temp_dir().join("Walk.png")),
                    (1, std::env::temp_dir().join("Run.png")),
                ],
                single_row: true,
                pixel_scale: 1,
            }
        );

        view.tab = ExportAnimationTab::Gif;
        assert_eq!(
            commit_export_animation(&view).unwrap(),
            ExportAnimationEvent::ExportGif {
                targets: vec![
                    (0, std::env::temp_dir().join("Walk.gif")),
                    (1, std::env::temp_dir().join("Run.gif")),
                ],
                pixel_scale: 1,
            }
        );
    }
}
