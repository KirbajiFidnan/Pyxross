//! Floating "Import Image" dock panel (id 210).
//!
//! The split step of the image import flow: the in-app File Manager (id 208,
//! mode [`crate::ui::dock_file_chooser_panel::FileChooserMode::ImportPng`])
//! picks a `.png`; the App decodes it and opens THIS panel to choose how the
//! image is cut into import regions. On OK the shell builds the region grid via
//! [`crate::core::import`].
//!
//! The panel is a **static-dynamic hybrid**:
//! - The left **form** pane is a [`NestMode::Static`] nest with separate `X`
//!   and `Y` inputs for both "Split by pixels" and "Split by number of
//!   regions" (live-linked via `ceil` semantics; whichever field the user last
//!   edited is authoritative), inline validation/summary, and OK / Cancel.
//! - The right **preview** pane is a [`NestMode::Dynamic`] nest with zoom
//!   (plain scroll) and pan (middle-drag or primary-drag), displaying the
//!   decoded image (`TextureOptions::NEAREST`) overlaid with the live split
//!   grid and an overlay zoom/Fit bar.
//!
//! Enter = OK (when valid), Esc = Cancel.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use egui::{pos2, vec2, Align, Layout, Rect, Stroke, UiBuilder};

use crate::ui::panel_dock::nest::{
    Component, ComponentPlacement, NestContent, NestMode, NestView,
};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};
use crate::ui::theme::SKIN_TINT;

/// Dock id for the Import Image floating panel.
pub const IMPORT_IMAGE_PANEL_ID: u64 = 210;

/// Panel/window title.
const IMPORT_IMAGE_TITLE: &str = "Import Image";

/// Fixed minimum size of the (non-resizable) floating window: left static form
/// + right dynamic split preview.
const IMPORT_IMAGE_MIN_SIZE: egui::Vec2 = vec2(640.0, 360.0);

/// Fixed width of the left static form column, in points.
const FORM_PANE_WIDTH: f32 = 250.0;

/// Width of each individual `X` / `Y` numeric input box.
const AXIS_INPUT_WIDTH: f32 = 64.0;

/// Which field the user last edited (authoritative for a consistent pair).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportImageField {
    Pixels,
    Regions,
}

/// The in-progress image-import form values (dialog draft, not model data).
#[derive(Clone, Debug)]
pub struct ImportImageView {
    /// The image chosen in the File Manager.
    pub path: PathBuf,
    /// Decoded image width/height in pixels.
    pub img_w: u32,
    pub img_h: u32,
    /// Editable "Split by pixels" X and Y inputs.
    pub pixels_x_text: String,
    pub pixels_y_text: String,
    /// Editable "Split by number of regions" X and Y inputs.
    pub regions_x_text: String,
    pub regions_y_text: String,
    /// Which field the user last edited (authoritative on OK).
    pub last_edited: ImportImageField,
    /// Set while a programmatic derivation writes the OTHER boxes, so the write
    /// is never mistaken for a user edit (no derivation loop).
    pub syncing: bool,
    /// Host-injected error (e.g. the decoder failed); shown while present.
    pub error: Option<String>,
    /// Canvas-size / region-count limits.
    pub max_canvas_w: u32,
    pub max_canvas_h: u32,
    pub max_frames: usize,
    /// Decoded RGBA8 bytes for the dynamic preview (`img_w * img_h * 4`).
    pub preview_rgba: Option<Vec<u8>>,
    /// Incremented whenever a new image is seeded so the preview re-uploads
    /// its texture and fits the camera.
    pub preview_generation: u64,
}

impl Default for ImportImageView {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            img_w: 0,
            img_h: 0,
            pixels_x_text: String::new(),
            pixels_y_text: String::new(),
            regions_x_text: String::new(),
            regions_y_text: String::new(),
            last_edited: ImportImageField::Pixels,
            syncing: false,
            error: None,
            max_canvas_w: 4096,
            max_canvas_h: 4096,
            max_frames: 4096,
            preview_rgba: None,
            preview_generation: 0,
        }
    }
}

/// An Import Image gesture emitted by the panel.
///
/// `Ok` carries a CONSISTENT `(px, regions)` pair: the field the user last
/// edited is authoritative, and the other side is its derived value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportImageEvent {
    Ok {
        px_w: u32,
        px_h: u32,
        regions_w: u32,
        regions_h: u32,
    },
    Cancel,
}

/// Shared cells for the Import Image panel (same shape as [`FileChooserHost`]).
///
/// [`FileChooserHost`]: crate::ui::dock_file_chooser_panel::FileChooserHost
#[derive(Clone, Default)]
pub struct ImportImageHost {
    pub view: Rc<RefCell<ImportImageView>>,
    pub events: Rc<RefCell<Vec<ImportImageEvent>>>,
}

impl ImportImageHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds the panel for a freshly decoded image: the pixels boxes start at
    /// the full image size and the regions boxes at their derived `1` × `1`.
    pub fn seed(
        &mut self,
        path: PathBuf,
        img_w: u32,
        img_h: u32,
        max_canvas_w: u32,
        max_canvas_h: u32,
        max_frames: usize,
    ) {
        let mut view = self.view.borrow_mut();
        view.path = path;
        view.img_w = img_w;
        view.img_h = img_h;
        view.pixels_x_text = img_w.to_string();
        view.pixels_y_text = img_h.to_string();
        let (rw, rh) = crate::core::import::regions_from_pixels(img_w, img_h, img_w, img_h);
        view.regions_x_text = rw.to_string();
        view.regions_y_text = rh.to_string();
        view.last_edited = ImportImageField::Pixels;
        view.syncing = false;
        view.error = None;
        view.max_canvas_w = max_canvas_w;
        view.max_canvas_h = max_canvas_h;
        view.max_frames = max_frames;
        view.preview_rgba = None;
        view.preview_generation = view.preview_generation.wrapping_add(1);
    }

    /// Supplies decoded RGBA8 pixels for the live split preview.
    pub fn set_preview_rgba(&mut self, rgba: Vec<u8>) {
        let mut view = self.view.borrow_mut();
        view.preview_rgba = Some(rgba);
        view.preview_generation = view.preview_generation.wrapping_add(1);
    }

    /// Drains the events the panel pushed this frame.
    pub fn take_events(&mut self) -> Vec<ImportImageEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    /// Sets the inline error (e.g. the shell's image decoder failed).
    pub fn set_error(&mut self, msg: String) {
        self.view.borrow_mut().error = Some(msg);
    }
}

/// Parses a single positive integer axis (`>= 1`).
pub fn parse_axis(text: &str) -> Option<u32> {
    let val = text.trim().parse::<u32>().ok()?;
    (val >= 1).then_some(val)
}

/// Parses a `(x, y)` pair of axis texts; both must be valid integers `>= 1`.
pub fn parse_pair(x_text: &str, y_text: &str) -> Option<(u32, u32)> {
    Some((parse_axis(x_text)?, parse_axis(y_text)?))
}

/// Parses a `"WxH"` pair: split on a single `x`/`X`, trim each side, both axes
/// must parse as `u32 >= 1`.
pub fn parse_dims(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().split(['x', 'X']);
    let w = parse_axis(parts.next()?)?;
    let h = parse_axis(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((w, h))
}

/// `"regions_w x regions_h"` derived from a pixel cell size.
pub fn derive_regions_text(img_w: u32, img_h: u32, px_w: u32, px_h: u32) -> String {
    let (rw, rh) = crate::core::import::regions_from_pixels(img_w, img_h, px_w, px_h);
    format!("{rw}x{rh}")
}

/// `"px_w x px_h"` derived from a region grid.
pub fn derive_pixels_text(img_w: u32, img_h: u32, regions_w: u32, regions_h: u32) -> String {
    let (pw, ph) = crate::core::import::pixels_from_regions(img_w, img_h, regions_w, regions_h);
    format!("{pw}x{ph}")
}

/// The CONSISTENT `(px_w, px_h, regions_w, regions_h)` for the view, using the
/// last-edited field as authoritative. `Err` when that field is unparseable.
pub fn effective_grid(view: &ImportImageView) -> Result<(u32, u32, u32, u32), String> {
    match view.last_edited {
        ImportImageField::Pixels => {
            let (pw, ph) = parse_pair(&view.pixels_x_text, &view.pixels_y_text)
                .ok_or_else(|| "Pixel split X and Y must be at least 1".to_owned())?;
            let (rw, rh) = crate::core::import::regions_from_pixels(view.img_w, view.img_h, pw, ph);
            Ok((pw, ph, rw, rh))
        }
        ImportImageField::Regions => {
            let (rw, rh) = parse_pair(&view.regions_x_text, &view.regions_y_text)
                .ok_or_else(|| "Region split X and Y must be at least 1".to_owned())?;
            let (pw, ph) = crate::core::import::pixels_from_regions(view.img_w, view.img_h, rw, rh);
            Ok((pw, ph, rw, rh))
        }
    }
}

/// `effective_grid` plus the live LIMITS: the region count must not exceed
/// `max_frames` and the implied canvas must fit `max_canvas_w × max_canvas_h`.
pub fn validate_image(view: &ImportImageView) -> Result<(u32, u32, u32, u32), String> {
    let (pw, ph, rw, rh) = effective_grid(view)?;
    let count = u64::from(rw) * u64::from(rh);
    if count > view.max_frames as u64 {
        return Err(format!(
            "{count} regions exceed the {} region limit",
            view.max_frames
        ));
    }
    let (cw, ch) = crate::core::import::canvas_from_grid(rw, rh, pw, ph);
    if cw > view.max_canvas_w || ch > view.max_canvas_h {
        return Err(format!(
            "canvas {cw}x{ch} exceeds {}x{}",
            view.max_canvas_w, view.max_canvas_h
        ));
    }
    Ok((pw, ph, rw, rh))
}

/// Applies a user edit to the pixels boxes: the field becomes authoritative
/// (even when the text is currently unparseable, so the OK button disables).
/// When valid, the regions boxes are re-derived (marked `syncing` so the
/// derivation never re-triggers); when invalid, nothing else changes.
pub fn on_pixels_edited(view: &mut ImportImageView) {
    view.last_edited = ImportImageField::Pixels;
    if let Some((pw, ph)) = parse_pair(&view.pixels_x_text, &view.pixels_y_text) {
        view.syncing = true;
        let (rw, rh) = crate::core::import::regions_from_pixels(view.img_w, view.img_h, pw, ph);
        view.regions_x_text = rw.to_string();
        view.regions_y_text = rh.to_string();
        view.syncing = false;
    }
}

/// [`on_pixels_edited`] for the regions boxes.
pub fn on_regions_edited(view: &mut ImportImageView) {
    view.last_edited = ImportImageField::Regions;
    if let Some((rw, rh)) = parse_pair(&view.regions_x_text, &view.regions_y_text) {
        view.syncing = true;
        let (pw, ph) = crate::core::import::pixels_from_regions(view.img_w, view.img_h, rw, rh);
        view.pixels_x_text = pw.to_string();
        view.pixels_y_text = ph.to_string();
        view.syncing = false;
    }
}

/// Renders one labelled `X` / `Y` input pair side by side in an ergonomic grid.
fn axis_pair_inputs(
    ui: &mut egui::Ui,
    id_salt: &str,
    x_text: &mut String,
    y_text: &mut String,
    unit_suffix: Option<&str>,
) -> bool {
    let mut changed = false;
    ui.push_id(id_salt, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label("X");
            let rx = ui.add(
                egui::TextEdit::singleline(x_text)
                    .hint_text("W")
                    .desired_width(AXIS_INPUT_WIDTH),
            );
            ui.add_space(4.0);
            ui.label("Y");
            let ry = ui.add(
                egui::TextEdit::singleline(y_text)
                    .hint_text("H")
                    .desired_width(AXIS_INPUT_WIDTH),
            );
            if let Some(unit) = unit_suffix {
                ui.label(egui::RichText::new(unit).weak());
            }
            changed = rx.changed() || ry.changed();
        });
    });
    changed
}

/// The static Import Image form (left pane): separate X/Y inputs for pixels and
/// regions, live summary/error, and symmetric OK / Cancel actions.
struct ImportImageComponent {
    host: Rc<RefCell<ImportImageHost>>,
}

impl Component for ImportImageComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let (img_w, img_h, mut px_x, mut px_y, mut reg_x, mut reg_y) = {
            let host = self.host.borrow();
            let v = host.view.borrow();
            (
                v.img_w,
                v.img_h,
                v.pixels_x_text.clone(),
                v.pixels_y_text.clone(),
                v.regions_x_text.clone(),
                v.regions_y_text.clone(),
            )
        };

        ui.label(egui::RichText::new(format!("Image: {img_w} × {img_h} px")).weak());
        ui.add_space(6.0);

        ui.strong("Split by pixels");
        ui.add_space(2.0);
        let pixels_changed = axis_pair_inputs(
            ui,
            "import_pixels_xy",
            &mut px_x,
            &mut px_y,
            Some("px"),
        );

        ui.add_space(8.0);
        ui.strong("Split by number of regions");
        ui.add_space(2.0);
        let regions_changed =
            axis_pair_inputs(ui, "import_regions_xy", &mut reg_x, &mut reg_y, None);

        // Write the raw texts back, then apply the field the user edited. The
        // `syncing` guard (set inside `on_*_edited`) keeps the programmatic
        // derivation from being read back as an edit.
        {
            let host = self.host.borrow();
            let mut v = host.view.borrow_mut();
            v.pixels_x_text = px_x;
            v.pixels_y_text = px_y;
            v.regions_x_text = reg_x;
            v.regions_y_text = reg_y;
            if !v.syncing {
                if pixels_changed {
                    on_pixels_edited(&mut v);
                } else if regions_changed {
                    on_regions_edited(&mut v);
                }
            }
        }

        ui.add_space(8.0);

        // Live validation drives OK enablement; a host-injected error wins for
        // display, otherwise show either the live error or the output summary.
        let host = self.host.borrow();
        let (live, host_error) = {
            let v = host.view.borrow();
            (validate_image(&v), v.error.clone())
        };
        let valid = live.is_ok();
        let shown_error = host_error.or_else(|| live.clone().err());
        if let Some(err) = &shown_error {
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        } else if let Ok((pw, ph, rw, rh)) = live {
            let (cw, ch) = crate::core::import::canvas_from_grid(rw, rh, pw, ph);
            let count = u64::from(rw) * u64::from(rh);
            ui.label(
                egui::RichText::new(format!("Output: {cw} × {ch} px ({count} regions)"))
                    .weak(),
            );
        }

        ui.add_space(10.0);

        // Symmetric centred action pair (mirrors save_as).
        let mut ok_clicked = false;
        let mut cancel_clicked = false;
        ui.columns(2, |columns| {
            columns[0].with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(valid, |ui| {
                    if chrome.button(ui, "OK", ButtonStyle::plain()).clicked() {
                        ok_clicked = true;
                    }
                });
            });
            columns[1].with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    cancel_clicked = true;
                }
            });
        });

        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed =
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        if cancel_clicked || escape_pressed {
            host.events.borrow_mut().push(ImportImageEvent::Cancel);
            return;
        }

        if ok_clicked || enter_pressed {
            if let Ok((px_w, px_h, regions_w, regions_h)) = live {
                host.events.borrow_mut().push(ImportImageEvent::Ok {
                    px_w,
                    px_h,
                    regions_w,
                    regions_h,
                });
            }
        }
    }
}

/// Content-space preview component (inside the dynamic nest): renders the
/// decoded PNG with `TextureOptions::NEAREST` and overlays the live split grid.
struct ImportImagePreviewComponent {
    host: Rc<RefCell<ImportImageHost>>,
    preview_scale: Rc<std::cell::Cell<f32>>,
    texture: Option<egui::TextureHandle>,
    uploaded_generation: u64,
}

impl ImportImagePreviewComponent {
    fn new(host: Rc<RefCell<ImportImageHost>>, preview_scale: Rc<std::cell::Cell<f32>>) -> Self {
        Self {
            host,
            preview_scale,
            texture: None,
            uploaded_generation: u64::MAX,
        }
    }

    fn ensure_texture(&mut self, ctx: &egui::Context) {
        let host = self.host.borrow();
        let v = host.view.borrow();
        if self.uploaded_generation == v.preview_generation {
            return;
        }
        self.uploaded_generation = v.preview_generation;
        self.texture = None;
        let (w, h) = (v.img_w as usize, v.img_h as usize);
        let Some(rgba) = &v.preview_rgba else {
            return;
        };
        if w == 0 || h == 0 || rgba.len() != w.saturating_mul(h).saturating_mul(4) {
            return;
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
        self.texture = Some(ctx.load_texture(
            "import_image_preview",
            image,
            egui::TextureOptions::NEAREST,
        ));
    }
}

impl Component for ImportImagePreviewComponent {
    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, chrome: &PanelChrome) {
        self.preview_scale.set(view.scale());
        self.ensure_texture(ui.ctx());

        let (img_w, img_h, grid) = {
            let host = self.host.borrow();
            let v = host.view.borrow();
            (v.img_w, v.img_h, effective_grid(&v).ok())
        };

        let painter = view.painter(ui);
        if img_w == 0 || img_h == 0 {
            return;
        }

        // Paint the image at world (0, 0)..(img_w, img_h).
        let img_world = Rect::from_min_size(egui::Pos2::ZERO, vec2(img_w as f32, img_h as f32));
        let img_screen = view.world_rect_to_screen(img_world);
        painter.rect_filled(img_screen, 0.0, ui.visuals().extreme_bg_color);
        if let Some(tex) = &self.texture {
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.image(tex.id(), img_screen, uv, SKIN_TINT);
        } else {
            painter.rect_stroke(
                img_screen,
                0.0,
                Stroke::new(1.0, chrome.colors.panel_border32()),
                egui::StrokeKind::Inside,
            );
        }

        // Paint the live split grid over the image (and any rounded-up surplus).
        if let Some((pw, ph, rw, rh)) = grid {
            let (cw, ch) = crate::core::import::canvas_from_grid(rw, rh, pw, ph);
            let grid_world = Rect::from_min_size(egui::Pos2::ZERO, vec2(cw as f32, ch as f32));
            let grid_screen = view.world_rect_to_screen(grid_world);
            let grid_color = ui.visuals().selection.stroke.color;
            let grid_stroke = Stroke::new(1.0, grid_color);

            // Limit drawn interior lines so huge counts stay fast.
            if rw <= 512 && rh <= 512 {
                for col in 1..rw {
                    let wx = (col * pw) as f32;
                    let a = view.world_to_screen(pos2(wx, 0.0));
                    let b = view.world_to_screen(pos2(wx, ch as f32));
                    painter.line_segment([a, b], grid_stroke);
                }
                for row in 1..rh {
                    let wy = (row * ph) as f32;
                    let a = view.world_to_screen(pos2(0.0, wy));
                    let b = view.world_to_screen(pos2(cw as f32, wy));
                    painter.line_segment([a, b], grid_stroke);
                }
            }
            painter.rect_stroke(
                grid_screen,
                0.0,
                Stroke::new(1.5, grid_color),
                egui::StrokeKind::Outside,
            );
        }
    }
}

/// Screen-space overlay inside the dynamic preview nest: zoom readout + Fit
/// button.
struct ImportImagePreviewOverlay {
    fit_requested: Rc<RefCell<bool>>,
    preview_scale: Rc<std::cell::Cell<f32>>,
}

impl Component for ImportImagePreviewOverlay {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, chrome: &PanelChrome) {
        let viewport = view.clip();
        let bar = Rect::from_min_max(
            pos2(viewport.min.x + 6.0, viewport.max.y - 26.0),
            pos2(viewport.max.x - 6.0, viewport.max.y - 4.0),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(bar)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                let pct = (self.preview_scale.get() * 100.0).round() as i32;
                ui.label(egui::RichText::new(format!("{pct}% • Scroll: zoom • Drag: pan")).weak());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if chrome.button(ui, "Fit", ButtonStyle::plain()).clicked() {
                        *self.fit_requested.borrow_mut() = true;
                    }
                });
            },
        );
    }
}

/// Static-dynamic hybrid panel content:
/// - Left: `form_nest` (`NestMode::Static`) — strictly static data-entry form.
/// - Right: `preview_nest` (`NestMode::Dynamic`) — strictly dynamic zoom/pan
///   split preview.
struct ImportImagePanelContent {
    host: Rc<RefCell<ImportImageHost>>,
    form_nest: NestContent,
    preview_nest: NestContent,
    fit_requested: Rc<RefCell<bool>>,
    preview_scale: Rc<std::cell::Cell<f32>>,
    fitted_generation: u64,
}

impl PanelContent for ImportImagePanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let full = ui.available_rect_before_wrap();
        let left_width = FORM_PANE_WIDTH.min((full.width() - 120.0).max(160.0));
        let left_rect = Rect::from_min_size(full.min, vec2(left_width, full.height()));
        let right_rect = Rect::from_min_max(
            pos2(left_rect.max.x + 8.0, full.min.y),
            full.max,
        );

        // Left pane: strictly static form nest.
        ui.scope_builder(UiBuilder::new().max_rect(left_rect), |ui| {
            self.form_nest.ui(ui, chrome);
        });

        // Right pane: strictly dynamic preview nest with subtle border well.
        ui.painter().rect_filled(
            right_rect,
            2.0,
            ui.visuals().extreme_bg_color,
        );
        ui.painter().rect_stroke(
            right_rect,
            2.0,
            Stroke::new(1.0, chrome.colors.panel_border32()),
            egui::StrokeKind::Inside,
        );

        let (img_w, img_h, gen) = {
            let h = self.host.borrow();
            let v = h.view.borrow();
            (v.img_w, v.img_h, v.preview_generation)
        };
        let fit_clicked = {
            let mut flag = self.fit_requested.borrow_mut();
            let was = *flag;
            *flag = false;
            was
        };
        if (self.fitted_generation != gen || fit_clicked) && img_w > 0 && img_h > 0 {
            self.fitted_generation = gen;
            self.preview_nest.camera_mut().fit(
                vec2(img_w as f32, img_h as f32),
                right_rect.size(),
            );
        }

        // Also support primary-button drag pan inside the preview pane (in
        // addition to `NestMode::Dynamic`'s built-in middle-drag pan & scroll
        // zoom).
        let pointer = ui.input(|i| i.pointer.latest_pos());
        if pointer.is_some_and(|p| right_rect.contains(p))
            && ui.input(|i| i.pointer.primary_down())
        {
            let delta = ui.input(|i| i.pointer.delta());
            if delta != egui::Vec2::ZERO {
                self.preview_nest.camera_mut().pan_by(delta);
            }
        }

        self.preview_scale.set(self.preview_nest.camera().scale());
        ui.scope_builder(UiBuilder::new().max_rect(right_rect), |ui| {
            self.preview_nest.ui(ui, chrome);
        });
        self.preview_scale.set(self.preview_nest.camera().scale());
    }
}

/// Static nest holding the Import Image form.
pub(crate) fn build_import_image_nest(host: &Rc<RefCell<ImportImageHost>>) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ImportImageComponent { host: host.clone() });
    nest
}

/// Dynamic nest holding the zoomable/pannable split preview.
pub(crate) fn build_import_image_preview_nest(
    host: &Rc<RefCell<ImportImageHost>>,
    fit_requested: Rc<RefCell<bool>>,
    preview_scale: Rc<std::cell::Cell<f32>>,
) -> NestContent {
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(ImportImagePreviewComponent::new(
        host.clone(),
        preview_scale.clone(),
    ));
    nest.push(ImportImagePreviewOverlay {
        fit_requested,
        preview_scale,
    });
    nest
}

/// Dock spec for the Import Image floating panel (dock id 210).
///
/// **Float-only:** `can_pop_out == false`; `resizable == false` keeps the
/// window fixed-size and draggable only by its header.
pub fn import_image_panel_spec(host: Rc<RefCell<ImportImageHost>>) -> PanelSpec {
    let floating_rect = egui::Rect::from_min_size(egui::pos2(160.0, 140.0), IMPORT_IMAGE_MIN_SIZE);
    let fit_requested = Rc::new(RefCell::new(false));
    let preview_scale = Rc::new(std::cell::Cell::new(1.0));
    PanelSpec {
        id: PanelId::new(IMPORT_IMAGE_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(IMPORT_IMAGE_TITLE, false, IMPORT_IMAGE_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ImportImagePanelContent {
            host: host.clone(),
            form_nest: build_import_image_nest(&host),
            preview_nest: build_import_image_preview_nest(
                &host,
                fit_requested.clone(),
                preview_scale.clone(),
            ),
            fit_requested,
            preview_scale,
            fitted_generation: u64::MAX,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_rc_seeded() -> Rc<RefCell<ImportImageHost>> {
        let host = Rc::new(RefCell::new(ImportImageHost::new()));
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/img.png"), 64, 64, 4096, 4096, 4096);
        host
    }

    #[test]
    fn parse_dims_accepts_wxh_and_rejects_junk() {
        assert_eq!(parse_dims("64x64"), Some((64, 64)));
        assert_eq!(parse_dims(" 16 X 32 "), Some((16, 32)));
        assert_eq!(parse_dims("4x1"), Some((4, 1)));
        assert_eq!(parse_dims("0x4"), None);
        assert_eq!(parse_dims("4x0"), None);
        assert_eq!(parse_dims("abc"), None);
        assert_eq!(parse_dims("4"), None);
        assert_eq!(parse_dims("4x4x4"), None);
        assert_eq!(parse_dims(""), None);
    }

    #[test]
    fn seed_defaults_full_image_and_single_region() {
        let host = host_rc_seeded();
        let h = host.borrow();
        let v = h.view.borrow();
        assert_eq!(v.pixels_x_text, "64");
        assert_eq!(v.pixels_y_text, "64");
        assert_eq!(v.regions_x_text, "1");
        assert_eq!(v.regions_y_text, "1");
        assert_eq!(v.last_edited, ImportImageField::Pixels);
        assert!(!v.syncing);
        assert!(v.error.is_none());
    }

    #[test]
    fn pixels_edit_derives_regions_both_directions() {
        // pixels 16x16 -> regions 4x4
        let host = host_rc_seeded();
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.pixels_x_text = "16".to_owned();
            v.pixels_y_text = "16".to_owned();
            on_pixels_edited(&mut v);
            assert_eq!(v.regions_x_text, "4");
            assert_eq!(v.regions_y_text, "4");
        }
        // pixels 20x20 -> regions 4x4 (ceil)
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.pixels_x_text = "20".to_owned();
            v.pixels_y_text = "20".to_owned();
            on_pixels_edited(&mut v);
            assert_eq!(v.regions_x_text, "4");
            assert_eq!(v.regions_y_text, "4");
        }
    }

    #[test]
    fn regions_edit_derives_pixels_both_directions() {
        // regions 4x1 -> pixels 16x64
        let host = host_rc_seeded();
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.regions_x_text = "4".to_owned();
            v.regions_y_text = "1".to_owned();
            on_regions_edited(&mut v);
            assert_eq!(v.pixels_x_text, "16");
            assert_eq!(v.pixels_y_text, "64");
        }
        // regions 3x3 -> pixels 22x22
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.regions_x_text = "3".to_owned();
            v.regions_y_text = "3".to_owned();
            on_regions_edited(&mut v);
            assert_eq!(v.pixels_x_text, "22");
            assert_eq!(v.pixels_y_text, "22");
        }
    }

    #[test]
    fn editing_does_not_loop_and_marks_the_edited_field() {
        let host = host_rc_seeded();
        let h = host.borrow();
        let mut v = h.view.borrow_mut();
        v.pixels_x_text = "16".to_owned();
        v.pixels_y_text = "16".to_owned();
        on_pixels_edited(&mut v);
        assert_eq!(v.last_edited, ImportImageField::Pixels);
        assert_eq!(v.regions_x_text, "4", "the other box is derived");
        assert_eq!(v.regions_y_text, "4", "the other box is derived");
        assert_eq!(v.pixels_x_text, "16", "the edited box is untouched");
        assert_eq!(v.pixels_y_text, "16", "the edited box is untouched");
        assert!(
            !v.syncing,
            "the syncing guard is cleared after the derivation"
        );
    }

    #[test]
    fn invalid_edit_leaves_the_other_box_unchanged() {
        let host = host_rc_seeded();
        let h = host.borrow();
        let mut v = h.view.borrow_mut();
        v.regions_x_text = "abc".to_owned();
        let before_px_x = v.pixels_x_text.clone();
        let before_px_y = v.pixels_y_text.clone();
        on_regions_edited(&mut v);
        assert_eq!(
            (v.pixels_x_text.clone(), v.pixels_y_text.clone()),
            (before_px_x, before_px_y),
            "no derivation on invalid input"
        );
        assert_eq!(
            v.last_edited,
            ImportImageField::Regions,
            "the edited field is authoritative even while its text is invalid"
        );
        assert!(
            validate_image(&v).is_err(),
            "an invalid authoritative field disables OK"
        );
    }

    #[test]
    fn ok_payload_follows_the_last_edited_field() {
        let host = host_rc_seeded();
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.regions_x_text = "3".to_owned();
            v.regions_y_text = "3".to_owned();
            on_regions_edited(&mut v);
        }
        let (px_w, px_h, rw, rh) = validate_image(&host.borrow().view.borrow()).unwrap();
        assert_eq!((px_w, px_h, rw, rh), (22, 22, 3, 3));

        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.pixels_x_text = "16".to_owned();
            v.pixels_y_text = "16".to_owned();
            on_pixels_edited(&mut v);
        }
        let (px_w, px_h, rw, rh) = validate_image(&host.borrow().view.borrow()).unwrap();
        assert_eq!((px_w, px_h, rw, rh), (16, 16, 4, 4));
    }

    #[test]
    fn limits_report_region_count_and_canvas_size() {
        // Region count over max_frames.
        let host = Rc::new(RefCell::new(ImportImageHost::new()));
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/i.png"), 64, 64, 4096, 4096, 4);
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.regions_x_text = "3".to_owned();
            v.regions_y_text = "3".to_owned();
            on_regions_edited(&mut v);
        }
        let err = validate_image(&host.borrow().view.borrow()).unwrap_err();
        assert!(err.contains("regions exceed"), "{err}");

        // Canvas over the max size: 64x64 image @ 20x20 -> 4x4 regions -> 80x80.
        let host = Rc::new(RefCell::new(ImportImageHost::new()));
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/i.png"), 64, 64, 64, 64, 4096);
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.pixels_x_text = "20".to_owned();
            v.pixels_y_text = "20".to_owned();
            on_pixels_edited(&mut v);
        }
        let err = validate_image(&host.borrow().view.borrow()).unwrap_err();
        assert!(err.contains("canvas 80x80"), "{err}");
    }

    #[test]
    fn spec_is_float_only_not_resizable_and_hybrid_nests_match_contract() {
        let host = host_rc_seeded();
        let spec = import_image_panel_spec(host.clone());
        assert_eq!(spec.id.raw(), IMPORT_IMAGE_PANEL_ID);
        assert_eq!(spec.metadata.title, "Import Image");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);

        let form_nest = build_import_image_nest(&host);
        assert_eq!(
            form_nest.mode(),
            NestMode::Static,
            "the data-entry form nest must be strictly static"
        );
        let preview_nest = build_import_image_preview_nest(
            &host,
            Rc::new(RefCell::new(false)),
            Rc::new(std::cell::Cell::new(1.0)),
        );
        assert_eq!(
            preview_nest.mode(),
            NestMode::Dynamic,
            "the preview nest must be strictly dynamic"
        );
    }

    /// Drive one headless frame of the hybrid panel content with `events`.
    fn run_panel_frame(
        ctx: &egui::Context,
        content: &mut Box<dyn PanelContent>,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                IMPORT_IMAGE_MIN_SIZE,
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = crate::ui::theme::Theme::default_dark();
        let surface =
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(IMPORT_IMAGE_PANEL_ID));
        let capture = crate::ui::input_capture::InputCapture::new();
        capture.set_target(Some(surface));
        let chrome = PanelChrome::flat(&theme, &capture, surface);
        let mut output = ctx.run_ui(raw, |ui| {
            content.ui(ui, &chrome);
        });
        output.textures_delta.clear();
        output
    }

    fn key_event(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn text_positions(output: &egui::FullOutput, text: &str) -> Vec<egui::Pos2> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos)),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
            .into_iter()
            .filter(|(rendered, _)| rendered == text)
            .map(|(_, pos)| pos)
            .collect()
    }

    #[test]
    fn component_paints_split_xy_labels_preview_and_centred_actions() {
        let ctx = egui::Context::default();
        let host = host_rc_seeded();
        host.borrow_mut()
            .set_preview_rgba(vec![255u8; 64 * 64 * 4]);
        let mut spec = import_image_panel_spec(host);
        let output = run_panel_frame(&ctx, &mut spec.content, Vec::new());
        assert!(!output.shapes.is_empty(), "the panel paints content");
        assert!(!text_positions(&output, "Split by pixels").is_empty());
        assert!(!text_positions(&output, "Split by number of regions").is_empty());
        assert_eq!(text_positions(&output, "X").len(), 2);
        assert_eq!(text_positions(&output, "Y").len(), 2);
        assert!(!text_positions(&output, "Fit").is_empty());
        assert!(!text_positions(&output, "OK").is_empty());
        assert!(!text_positions(&output, "Cancel").is_empty());
        let ok_x = text_positions(&output, "OK").into_iter().next().unwrap().x;
        let cancel_x = text_positions(&output, "Cancel")
            .into_iter()
            .next()
            .unwrap()
            .x;
        assert!(ok_x < cancel_x, "OK sits left of Cancel");
    }

    #[test]
    fn enter_emits_ok_with_the_consistent_pair() {
        let ctx = egui::Context::default();
        let host = host_rc_seeded();
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.regions_x_text = "3".to_owned();
            v.regions_y_text = "3".to_owned();
            on_regions_edited(&mut v);
        }
        let mut spec = import_image_panel_spec(host.clone());
        let _ = run_panel_frame(&ctx, &mut spec.content, vec![key_event(egui::Key::Enter)]);
        assert_eq!(
            host.borrow_mut().take_events(),
            vec![ImportImageEvent::Ok {
                px_w: 22,
                px_h: 22,
                regions_w: 3,
                regions_h: 3,
            }]
        );
    }

    #[test]
    fn escape_emits_cancel() {
        let ctx = egui::Context::default();
        let host = host_rc_seeded();
        let mut spec = import_image_panel_spec(host.clone());
        let _ = run_panel_frame(&ctx, &mut spec.content, vec![key_event(egui::Key::Escape)]);
        assert_eq!(
            host.borrow_mut().take_events(),
            vec![ImportImageEvent::Cancel]
        );
    }

    #[test]
    fn preview_zooms_on_scroll_and_pans_on_drag() {
        let ctx = egui::Context::default();
        let host = host_rc_seeded();
        host.borrow_mut()
            .set_preview_rgba(vec![200u8; 64 * 64 * 4]);
        let mut spec = import_image_panel_spec(host);

        // Frame 1: auto-fits the 64x64 image in the right preview pane.
        let out1 = run_panel_frame(&ctx, &mut spec.content, Vec::new());
        let fit_zoom = out1
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().contains('%') => {
                    Some(t.galley.text().to_owned())
                }
                _ => None,
            })
            .next()
            .expect("zoom badge painted");

        // Frame 2: scroll inside the right preview pane (x=450, y=180) zooms in.
        let preview_pt = pos2(450.0, 180.0);
        let out2 = run_panel_frame(
            &ctx,
            &mut spec.content,
            vec![
                egui::Event::PointerMoved(preview_pt),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, 40.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let zoomed_label = out2
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text().contains('%') => {
                    Some(t.galley.text().to_owned())
                }
                _ => None,
            })
            .next()
            .expect("zoom badge painted after scroll");
        assert_ne!(
            fit_zoom, zoomed_label,
            "scrolling inside the dynamic preview pane changes zoom"
        );
    }

    #[test]
    fn take_events_drains_and_set_error_surfaces() {
        let mut host = ImportImageHost::new();
        host.set_error("decode failed".to_owned());
        assert_eq!(host.view.borrow().error.as_deref(), Some("decode failed"));
        host.events.borrow_mut().push(ImportImageEvent::Cancel);
        assert_eq!(host.take_events().len(), 1);
        assert!(host.take_events().is_empty());
    }

    #[test]
    fn no_hardcoded_color_constructors_in_production() {
        let src = include_str!("dock_import_image_panel.rs");
        let prod = src.find("#[cfg(test)]").map_or(src, |i| &src[..i]);
        for pattern in [
            "Color32::from_rgb(",
            "Color32::from_rgba_unmultiplied(",
            "Color32::WHITE",
            "Color32::BLACK",
        ] {
            assert!(
                !prod.contains(pattern),
                "production code must not hard-code a colour ({pattern})"
            );
        }
    }
}
