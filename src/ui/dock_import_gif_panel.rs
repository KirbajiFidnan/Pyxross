//! Floating "Import GIF" dock panel (id 209).
//!
//! The second step of the GIF import flow: the in-app File Manager (id 208,
//! mode [`crate::ui::dock_file_chooser_panel::FileChooserMode::ImportGif`])
//! picks a `.gif`; the App decodes it (via `io`) and opens THIS panel to choose
//! how many frames go on each sheet row. On OK the shell lays the frames onto
//! a region sheet using [`crate::core::import::gif_sheet`].
//!
//! The panel is a **static-dynamic hybrid** matching `dock_import_image_panel`:
//! - The left **form** pane is a [`NestMode::Static`] nest with **Row length**,
//!   a **Single row document** checkbox (which locks Row length to the frame
//!   count), live grid/canvas summary + validation, and OK / Cancel.
//! - The right **preview** pane is a [`NestMode::Dynamic`] nest with zoom
//!   (plain scroll) and pan (middle-drag or primary-drag), displaying the
//!   decoded GIF frames laid out left-to-right, row-by-row (`frame_w × frame_h`
//!   per region, surplus cells left empty) with the region grid overlay and a
//!   zoom / Fit overlay bar.
//!
//! Enter = OK (when valid), Esc = Cancel.

use std::cell::{Cell, RefCell};
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

/// Dock id for the Import GIF floating panel.
pub const IMPORT_GIF_PANEL_ID: u64 = 209;

/// Panel/window title.
const IMPORT_GIF_TITLE: &str = "Import GIF";

/// Fixed minimum size of the (non-resizable) floating window: left static form
/// + right dynamic sheet preview.
const IMPORT_GIF_MIN_SIZE: egui::Vec2 = vec2(640.0, 360.0);

/// Fixed width of the left static form column, in points.
const FORM_PANE_WIDTH: f32 = 250.0;

/// Default maximum total sheet cells ([`crate::core::import::gif_sheet`]'s
/// `cells`) before the import is rejected.
pub const IMPORT_GIF_DEFAULT_MAX_FRAMES: usize = 4096;

/// Default maximum canvas side (pixels) before the import is rejected.
pub const IMPORT_GIF_DEFAULT_MAX_CANVAS_SIDE: u32 = 4096;

/// The in-progress GIF-import form values (dialog draft, not model data).
#[derive(Clone, Debug)]
pub struct ImportGifView {
    /// The `.gif` chosen in the File Manager.
    pub path: PathBuf,
    /// Number of frames in the decoded animation.
    pub frame_count: usize,
    /// One frame's width in pixels (also each region's width).
    pub frame_w: u32,
    /// One frame's height in pixels (also each region's height).
    pub frame_h: u32,
    /// Editable "Row length" text (frames per sheet row); starts at `"1"`.
    pub line_length_text: String,
    /// When `true`, Row length is locked to `frame_count` so all frames sit on
    /// a single horizontal row.
    pub single_row_document: bool,
    /// Inline validation/decode error; the panel stays open with this text.
    pub error: Option<String>,
    /// Maximum total sheet cells allowed (seeded by the shell).
    pub max_frames: usize,
    /// Maximum canvas width/height allowed.
    pub max_canvas_w: u32,
    pub max_canvas_h: u32,
    /// Decoded RGBA8 pixel buffers per frame for the dynamic sheet preview.
    pub preview_frames: Vec<Vec<u8>>,
    /// Incremented whenever a new GIF is seeded/updated so the preview
    /// re-uploads its frame textures and fits the camera.
    pub preview_generation: u64,
}

impl Default for ImportGifView {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            frame_count: 0,
            frame_w: 0,
            frame_h: 0,
            line_length_text: "1".to_owned(),
            single_row_document: false,
            error: None,
            max_frames: IMPORT_GIF_DEFAULT_MAX_FRAMES,
            max_canvas_w: IMPORT_GIF_DEFAULT_MAX_CANVAS_SIDE,
            max_canvas_h: IMPORT_GIF_DEFAULT_MAX_CANVAS_SIDE,
            preview_frames: Vec::new(),
            preview_generation: 0,
        }
    }
}

/// An Import GIF gesture emitted by the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportGifEvent {
    /// The user confirmed with a valid row length.
    Ok { line_length: u32 },
    /// Return to file selection.
    Back,
    /// The user dismissed the panel.
    Cancel,
}

/// Shared cells for the Import GIF panel (same shape as [`FileChooserHost`]).
///
/// [`FileChooserHost`]: crate::ui::dock_file_chooser_panel::FileChooserHost
#[derive(Clone, Default)]
pub struct ImportGifHost {
    pub view: Rc<RefCell<ImportGifView>>,
    pub events: Rc<RefCell<Vec<ImportGifEvent>>>,
}

impl ImportGifHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds the panel for a freshly decoded animation and resets the form:
    /// row length back to `"1"`, single-row unchecked, error cleared.
    pub fn seed(
        &mut self,
        path: PathBuf,
        frame_count: usize,
        frame_w: u32,
        frame_h: u32,
        max_frames: usize,
    ) {
        let mut view = self.view.borrow_mut();
        view.path = path;
        view.frame_count = frame_count;
        view.frame_w = frame_w;
        view.frame_h = frame_h;
        view.line_length_text = "1".to_owned();
        view.single_row_document = false;
        view.error = None;
        view.max_frames = max_frames;
        view.max_canvas_w = IMPORT_GIF_DEFAULT_MAX_CANVAS_SIDE;
        view.max_canvas_h = IMPORT_GIF_DEFAULT_MAX_CANVAS_SIDE;
        view.preview_frames.clear();
        view.preview_generation = view.preview_generation.wrapping_add(1);
    }

    /// Supplies decoded per-frame RGBA8 pixels for the live sheet preview.
    pub fn set_preview_frames(&mut self, frames: Vec<Vec<u8>>) {
        let mut view = self.view.borrow_mut();
        view.preview_frames = frames;
        view.preview_generation = view.preview_generation.wrapping_add(1);
    }

    /// Drains the events the panel pushed this frame.
    pub fn take_events(&mut self) -> Vec<ImportGifEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    /// Sets the inline error (e.g. the shell's GIF decoder failed).
    pub fn set_error(&mut self, msg: String) {
        self.view.borrow_mut().error = Some(msg);
    }
}

/// Validation shared between the OK button and the tests: parse a `u32` row
/// length (`>= 1`), then require the resulting sheet to fit `max_frames`.
///
/// `frame_count == 0` (nothing to lay out) is an error. Returns the parsed row
/// length on success.
pub fn validate_line_length_text(
    text: &str,
    frame_count: usize,
    frame_w: u32,
    frame_h: u32,
    max_frames: usize,
) -> Result<u32, String> {
    let line_length: u32 = text
        .trim()
        .parse()
        .map_err(|_| "Row length must be a whole number".to_owned())?;
    if line_length == 0 {
        return Err("Row length must be at least 1".to_owned());
    }
    let sheet = crate::core::import::gif_sheet(frame_count, line_length, frame_w, frame_h)
        .ok_or_else(|| "There are no frames to lay out".to_owned())?;
    if sheet.cells > max_frames {
        return Err(format!(
            "{} cells exceed the {max_frames} cell limit",
            sheet.cells
        ));
    }
    Ok(line_length)
}

/// Effective row length and full sheet validation for `view` (honouring
/// `single_row_document` and the canvas size cap).
pub fn validate_gif_sheet(
    view: &ImportGifView,
) -> Result<(u32, crate::core::import::GifSheet), String> {
    let line_length = if view.single_row_document {
        if view.frame_count == 0 {
            return Err("There are no frames to lay out".to_owned());
        }
        view.frame_count.min(u32::MAX as usize) as u32
    } else {
        validate_line_length_text(
            &view.line_length_text,
            view.frame_count,
            view.frame_w,
            view.frame_h,
            view.max_frames,
        )?
    };
    let sheet = crate::core::import::gif_sheet(
        view.frame_count,
        line_length,
        view.frame_w,
        view.frame_h,
    )
    .ok_or_else(|| "There are no frames to lay out".to_owned())?;
    if sheet.cells > view.max_frames {
        return Err(format!(
            "{} cells exceed the {} cell limit",
            sheet.cells, view.max_frames
        ));
    }
    if sheet.canvas_w > view.max_canvas_w || sheet.canvas_h > view.max_canvas_h {
        return Err(format!(
            "canvas {}x{} exceeds {}x{}",
            sheet.canvas_w, sheet.canvas_h, view.max_canvas_w, view.max_canvas_h
        ));
    }
    Ok((line_length, sheet))
}

/// [`validate_line_length_text`] against a view's stored fields (honouring
/// `single_row_document`).
pub fn validate_line_length(view: &ImportGifView) -> Result<u32, String> {
    if view.single_row_document {
        validate_gif_sheet(view).map(|(len, _)| len)
    } else {
        validate_line_length_text(
            &view.line_length_text,
            view.frame_count,
            view.frame_w,
            view.frame_h,
            view.max_frames,
        )
    }
}

/// The static Import GIF form (left pane): Row length, "Single row document"
/// checkbox, live grid/canvas summary or error, and symmetric OK / Cancel.
struct ImportGifComponent {
    host: Rc<RefCell<ImportGifHost>>,
}

impl Component for ImportGifComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let (mut line_length_text, mut single_row, error, frame_count, frame_w, frame_h) = {
            let host = self.host.borrow();
            let v = host.view.borrow();
            (
                v.line_length_text.clone(),
                v.single_row_document,
                v.error.clone(),
                v.frame_count,
                v.frame_w,
                v.frame_h,
            )
        };

        ui.label(
            egui::RichText::new(format!(
                "GIF: {frame_count} frames • {frame_w} × {frame_h} px"
            ))
            .weak(),
        );
        ui.add_space(6.0);

        ui.strong("Row length");
        ui.add_space(2.0);
        if single_row {
            let mut locked_text = frame_count.max(1).to_string();
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut locked_text).desired_width(140.0),
            );
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut line_length_text)
                    .hint_text("frames per row")
                    .desired_width(140.0),
            );
        }

        ui.add_space(6.0);
        ui.checkbox(&mut single_row, "Single row document");

        // Write edits back before live validation.
        {
            let host = self.host.borrow();
            let mut v = host.view.borrow_mut();
            v.line_length_text = line_length_text;
            v.single_row_document = single_row;
        }

        ui.add_space(8.0);
        let host = self.host.borrow();
        let live_validation = {
            let v = host.view.borrow();
            validate_gif_sheet(&v)
        };
        let valid = live_validation.is_ok();
        let shown_error = error.or_else(|| live_validation.as_ref().err().cloned());
        if let Some(err) = &shown_error {
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        } else if let Ok((len, sheet)) = &live_validation {
            ui.label(
                egui::RichText::new(format!(
                    "Grid: {len} × {} regions ({frame_count}/{} used)",
                    sheet.rows, sheet.cells
                ))
                .weak(),
            );
            ui.label(
                egui::RichText::new(format!(
                    "Canvas: {} × {} px",
                    sheet.canvas_w, sheet.canvas_h
                ))
                .weak(),
            );
        }

        ui.add_space(10.0);

        // Symmetric centred action pair (mirrors import_image).
        let mut ok_clicked = false;
        let mut back_clicked = false;
        let mut cancel_clicked = false;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(valid, |ui| {
                if chrome.button(ui, "OK", ButtonStyle::plain()).clicked() {
                    ok_clicked = true;
                }
            });
            ui.add_space(6.0);
            if chrome
                .button(ui, "Back", ButtonStyle::plain())
                .on_hover_text("Return to file selection")
                .clicked()
            {
                back_clicked = true;
            }
            ui.add_space(6.0);
            if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                cancel_clicked = true;
            }
        });

        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed =
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        if back_clicked {
            host.view.borrow_mut().error = None;
            host.events.borrow_mut().push(ImportGifEvent::Back);
            return;
        }

        if cancel_clicked || escape_pressed {
            host.view.borrow_mut().error = None;
            host.events.borrow_mut().push(ImportGifEvent::Cancel);
            return;
        }

        if ok_clicked || enter_pressed {
            match live_validation {
                Ok((line_length, _)) => {
                    host.view.borrow_mut().error = None;
                    host.events
                        .borrow_mut()
                        .push(ImportGifEvent::Ok { line_length });
                }
                Err(message) => host.view.borrow_mut().error = Some(message),
            }
        }
    }
}

/// Content-space preview component (inside the dynamic nest): renders the
/// laid-out GIF frames left-to-right, row-by-row (`frame_w × frame_h` per
/// region, surplus cells empty) with the region grid overlay.
struct ImportGifPreviewComponent {
    host: Rc<RefCell<ImportGifHost>>,
    preview_scale: Rc<Cell<f32>>,
    frame_textures: Vec<egui::TextureHandle>,
    uploaded_generation: u64,
}

impl ImportGifPreviewComponent {
    fn new(host: Rc<RefCell<ImportGifHost>>, preview_scale: Rc<Cell<f32>>) -> Self {
        Self {
            host,
            preview_scale,
            frame_textures: Vec::new(),
            uploaded_generation: u64::MAX,
        }
    }

    fn ensure_textures(&mut self, ctx: &egui::Context) {
        let host = self.host.borrow();
        let v = host.view.borrow();
        if self.uploaded_generation == v.preview_generation {
            return;
        }
        self.uploaded_generation = v.preview_generation;
        self.frame_textures.clear();
        let (w, h) = (v.frame_w as usize, v.frame_h as usize);
        let expected_len = w.saturating_mul(h).saturating_mul(4);
        if w == 0 || h == 0 || expected_len == 0 {
            return;
        }
        for (idx, rgba) in v.preview_frames.iter().enumerate() {
            if rgba.len() != expected_len {
                continue;
            }
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
            let handle = ctx.load_texture(
                format!("import_gif_frame_{idx}"),
                image,
                egui::TextureOptions::NEAREST,
            );
            self.frame_textures.push(handle);
        }
    }
}

impl Component for ImportGifPreviewComponent {
    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, chrome: &PanelChrome) {
        self.preview_scale.set(view.scale());
        self.ensure_textures(ui.ctx());

        let (frame_count, frame_w, frame_h, layout) = {
            let host = self.host.borrow();
            let v = host.view.borrow();
            let layout = validate_gif_sheet(&v).ok().or_else(|| {
                // Even if limits are exceeded, still preview the parsed grid
                // when row length is a valid positive integer.
                let len = if v.single_row_document {
                    (v.frame_count.max(1)).min(u32::MAX as usize) as u32
                } else {
                    v.line_length_text.trim().parse::<u32>().ok().filter(|&n| n >= 1)?
                };
                let sheet =
                    crate::core::import::gif_sheet(v.frame_count, len, v.frame_w, v.frame_h)?;
                Some((len, sheet))
            });
            (v.frame_count, v.frame_w, v.frame_h, layout)
        };

        let Some((line_length, sheet)) = layout else {
            return;
        };
        if frame_w == 0 || frame_h == 0 || sheet.canvas_w == 0 || sheet.canvas_h == 0 {
            return;
        }

        let painter = view.painter(ui);
        let sheet_world = Rect::from_min_size(
            egui::Pos2::ZERO,
            vec2(sheet.canvas_w as f32, sheet.canvas_h as f32),
        );
        let sheet_screen = view.world_rect_to_screen(sheet_world);
        painter.rect_filled(sheet_screen, 0.0, ui.visuals().extreme_bg_color);

        // Paint each frame left-to-right, row-by-row; surplus cells stay empty.
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        let cols = line_length as usize;
        for index in 0..frame_count {
            let col = (index % cols) as u32;
            let row = (index / cols) as u32;
            let cell_world = Rect::from_min_size(
                pos2((col * frame_w) as f32, (row * frame_h) as f32),
                vec2(frame_w as f32, frame_h as f32),
            );
            let cell_screen = view.world_rect_to_screen(cell_world);
            if let Some(tex) = self.frame_textures.get(index) {
                painter.image(tex.id(), cell_screen, uv, SKIN_TINT);
            } else {
                painter.rect_filled(cell_screen, 0.0, chrome.colors.panel_bg32());
            }
        }

        // Paint the region grid lines + outer sheet boundary.
        let grid_color = ui.visuals().selection.stroke.color;
        let grid_stroke = Stroke::new(1.0, grid_color);
        if line_length <= 512 && sheet.rows <= 512 {
            for col in 1..line_length {
                let wx = (col * frame_w) as f32;
                let a = view.world_to_screen(pos2(wx, 0.0));
                let b = view.world_to_screen(pos2(wx, sheet.canvas_h as f32));
                painter.line_segment([a, b], grid_stroke);
            }
            for row in 1..sheet.rows as u32 {
                let wy = (row * frame_h) as f32;
                let a = view.world_to_screen(pos2(0.0, wy));
                let b = view.world_to_screen(pos2(sheet.canvas_w as f32, wy));
                painter.line_segment([a, b], grid_stroke);
            }
        }
        painter.rect_stroke(
            sheet_screen,
            0.0,
            Stroke::new(1.5, grid_color),
            egui::StrokeKind::Outside,
        );
    }
}

/// Screen-space overlay inside the dynamic preview nest: zoom readout + Fit
/// button.
struct ImportGifPreviewOverlay {
    fit_requested: Rc<RefCell<bool>>,
    preview_scale: Rc<Cell<f32>>,
}

impl Component for ImportGifPreviewOverlay {
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
                ui.label(
                    egui::RichText::new(format!("{pct}% • Scroll: zoom • Drag: pan")).weak(),
                );
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
/// - Left: `form_nest` (`NestMode::Static`) — strictly static form.
/// - Right: `preview_nest` (`NestMode::Dynamic`) — strictly dynamic zoom/pan
///   sheet preview.
struct ImportGifPanelContent {
    host: Rc<RefCell<ImportGifHost>>,
    form_nest: NestContent,
    preview_nest: NestContent,
    fit_requested: Rc<RefCell<bool>>,
    fitted_key: (u64, u32, u32),
}

impl PanelContent for ImportGifPanelContent {
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

        let (gen, sheet_size) = {
            let h = self.host.borrow();
            let v = h.view.borrow();
            let size = validate_gif_sheet(&v)
                .map(|(_, s)| (s.canvas_w, s.canvas_h))
                .unwrap_or((v.frame_w, v.frame_h));
            (v.preview_generation, size)
        };
        let fit_clicked = {
            let mut flag = self.fit_requested.borrow_mut();
            let was = *flag;
            *flag = false;
            was
        };
        let key = (gen, sheet_size.0, sheet_size.1);
        if (self.fitted_key != key || fit_clicked) && sheet_size.0 > 0 && sheet_size.1 > 0 {
            self.fitted_key = key;
            self.preview_nest.camera_mut().fit(
                vec2(sheet_size.0 as f32, sheet_size.1 as f32),
                right_rect.size(),
            );
        }

        // Support primary-button drag pan inside the preview pane in addition to
        // `NestMode::Dynamic`'s built-in middle-drag pan & scroll zoom.
        let pointer = ui.input(|i| i.pointer.latest_pos());
        if pointer.is_some_and(|p| right_rect.contains(p))
            && ui.input(|i| i.pointer.primary_down())
        {
            let delta = ui.input(|i| i.pointer.delta());
            if delta != egui::Vec2::ZERO {
                self.preview_nest.camera_mut().pan_by(delta);
            }
        }

        ui.scope_builder(UiBuilder::new().max_rect(right_rect), |ui| {
            self.preview_nest.ui(ui, chrome);
        });
    }
}

/// Static nest holding the Import GIF form.
pub(crate) fn build_import_gif_nest(host: &Rc<RefCell<ImportGifHost>>) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ImportGifComponent { host: host.clone() });
    nest
}

/// Dynamic nest holding the zoomable/pannable GIF sheet preview.
pub(crate) fn build_import_gif_preview_nest(
    host: &Rc<RefCell<ImportGifHost>>,
    fit_requested: Rc<RefCell<bool>>,
    preview_scale: Rc<Cell<f32>>,
) -> NestContent {
    let mut nest = NestContent::new(NestMode::Dynamic);
    nest.push(ImportGifPreviewComponent::new(
        host.clone(),
        preview_scale.clone(),
    ));
    nest.push(ImportGifPreviewOverlay {
        fit_requested,
        preview_scale,
    });
    nest
}

/// Dock spec for the Import GIF floating panel (dock id 209).
///
/// **Float-only:** `can_pop_out == false` (no Float/Dock button, not dockable);
/// `resizable == false` keeps the window fixed-size and draggable only by its
/// header.
pub fn import_gif_panel_spec(host: Rc<RefCell<ImportGifHost>>) -> PanelSpec {
    let floating_rect = egui::Rect::from_min_size(egui::pos2(160.0, 140.0), IMPORT_GIF_MIN_SIZE);
    let fit_requested = Rc::new(RefCell::new(false));
    let preview_scale = Rc::new(Cell::new(1.0));
    PanelSpec {
        id: PanelId::new(IMPORT_GIF_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(IMPORT_GIF_TITLE, false, IMPORT_GIF_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ImportGifPanelContent {
            host: host.clone(),
            form_nest: build_import_gif_nest(&host),
            preview_nest: build_import_gif_preview_nest(
                &host,
                fit_requested.clone(),
                preview_scale.clone(),
            ),
            fit_requested,
            fitted_key: (u64::MAX, 0, 0),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_rc() -> Rc<RefCell<ImportGifHost>> {
        Rc::new(RefCell::new(ImportGifHost::new()))
    }

    #[test]
    fn seed_sets_fields_and_resets_the_form() {
        let mut host = ImportGifHost::new();
        host.view.borrow_mut().line_length_text = "9".to_owned();
        host.view.borrow_mut().single_row_document = true;
        host.set_error("old".to_owned());
        host.seed(PathBuf::from("/tmp/anim.gif"), 22, 8, 16, 100);
        let v = host.view.borrow();
        assert_eq!(v.path, PathBuf::from("/tmp/anim.gif"));
        assert_eq!(v.frame_count, 22);
        assert_eq!((v.frame_w, v.frame_h), (8, 16));
        assert_eq!(v.line_length_text, "1", "seed resets the line length");
        assert!(!v.single_row_document, "seed resets single_row_document");
        assert!(v.error.is_none(), "seed clears a stale error");
        assert_eq!(v.max_frames, 100);
    }

    #[test]
    fn default_view_is_sane() {
        let v = ImportGifView::default();
        assert_eq!(v.line_length_text, "1");
        assert!(!v.single_row_document);
        assert_eq!(v.max_frames, IMPORT_GIF_DEFAULT_MAX_FRAMES);
        assert!(v.error.is_none());
    }

    #[test]
    fn validation_rejects_non_numeric_zero_and_over_limit() {
        assert!(validate_line_length_text("abc", 22, 8, 8, 4096).is_err());
        assert!(validate_line_length_text("", 22, 8, 8, 4096).is_err());
        assert!(validate_line_length_text("0", 22, 8, 8, 4096).is_err());
        let err = validate_line_length_text("4097", 22, 8, 8, 4096).unwrap_err();
        assert!(err.contains("exceed"), "{err}");
        assert!(validate_line_length_text("4", 0, 8, 8, 4096).is_err());
        assert_eq!(validate_line_length_text("4", 22, 8, 8, 4096).unwrap(), 4);
    }

    #[test]
    fn validation_trims_and_uses_gif_sheet_cells() {
        assert_eq!(
            validate_line_length_text("  4 ", 20, 8, 8, 4096).unwrap(),
            4
        );
        assert_eq!(validate_line_length_text("4", 20, 8, 8, 20).unwrap(), 4);
        assert!(validate_line_length_text("4", 22, 8, 8, 20).is_err());
    }

    #[test]
    fn sheet_rounds_up_14_frames_by_row_length_4_to_4x4_and_supports_single_row() {
        let mut host = ImportGifHost::new();
        host.seed(PathBuf::from("/tmp/14.gif"), 14, 64, 64, 4096);
        {
            let mut v = host.view.borrow_mut();
            v.line_length_text = "4".to_owned();
            let (len, sheet) = validate_gif_sheet(&v).unwrap();
            assert_eq!(len, 4);
            assert_eq!(sheet.rows, 4);
            assert_eq!(sheet.cells, 16);
            assert_eq!((sheet.canvas_w, sheet.canvas_h), (256, 256));

            // Single row document locks row length to 14 -> 14x1 regions.
            v.single_row_document = true;
            let (len, sheet) = validate_gif_sheet(&v).unwrap();
            assert_eq!(len, 14);
            assert_eq!(sheet.rows, 1);
            assert_eq!(sheet.cells, 14);
            assert_eq!((sheet.canvas_w, sheet.canvas_h), (14 * 64, 64));
        }
    }

    #[test]
    fn spec_is_float_only_not_resizable_and_hybrid_nests_match_contract() {
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 4, 8, 8, 4096);
        let spec = import_gif_panel_spec(host.clone());
        assert_eq!(spec.id.raw(), IMPORT_GIF_PANEL_ID);
        assert_eq!(spec.metadata.title, "Import GIF");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);

        let form_nest = build_import_gif_nest(&host);
        assert_eq!(
            form_nest.mode(),
            NestMode::Static,
            "the data-entry form nest must be strictly static"
        );
        let preview_nest = build_import_gif_preview_nest(
            &host,
            Rc::new(RefCell::new(false)),
            Rc::new(Cell::new(1.0)),
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
                IMPORT_GIF_MIN_SIZE,
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = crate::ui::theme::Theme::default_dark();
        let surface =
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(IMPORT_GIF_PANEL_ID));
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
    fn component_paints_labels_single_row_checkbox_preview_and_centred_actions() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 14, 16, 16, 4096);
        host.borrow_mut()
            .set_preview_frames(vec![vec![200u8; 16 * 16 * 4]; 14]);
        let mut spec = import_gif_panel_spec(host);
        let output = run_panel_frame(&ctx, &mut spec.content, Vec::new());
        assert!(!output.shapes.is_empty(), "the panel paints content");
        assert!(!text_positions(&output, "Row length").is_empty());
        assert!(!text_positions(&output, "Single row document").is_empty());
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
    fn component_paints_a_live_error_for_an_invalid_line_length() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 4, 8, 8, 4096);
        host.borrow_mut().view.borrow_mut().line_length_text = "0".to_owned();
        let mut spec = import_gif_panel_spec(host);
        let output = run_panel_frame(&ctx, &mut spec.content, Vec::new());
        assert!(
            !text_positions(&output, "Row length must be at least 1").is_empty(),
            "live validation shows an inline error when the input is 0"
        );
    }

    #[test]
    fn enter_emits_ok_with_the_line_length() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 22, 8, 8, 4096);
        host.borrow_mut().view.borrow_mut().line_length_text = "4".to_owned();
        let mut spec = import_gif_panel_spec(host.clone());
        let _ = run_panel_frame(&ctx, &mut spec.content, vec![key_event(egui::Key::Enter)]);
        assert_eq!(
            host.borrow_mut().take_events(),
            vec![ImportGifEvent::Ok { line_length: 4 }]
        );
    }

    #[test]
    fn enter_with_single_row_document_emits_frame_count() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 14, 8, 8, 4096);
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.line_length_text = "3".to_owned();
            v.single_row_document = true;
        }
        let mut spec = import_gif_panel_spec(host.clone());
        let _ = run_panel_frame(&ctx, &mut spec.content, vec![key_event(egui::Key::Enter)]);
        assert_eq!(
            host.borrow_mut().take_events(),
            vec![ImportGifEvent::Ok { line_length: 14 }]
        );
    }

    #[test]
    fn enter_with_an_invalid_line_length_does_not_emit() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 4, 8, 8, 4096);
        host.borrow_mut().view.borrow_mut().line_length_text = "nope".to_owned();
        let mut spec = import_gif_panel_spec(host.clone());
        let _ = run_panel_frame(&ctx, &mut spec.content, vec![key_event(egui::Key::Enter)]);
        assert!(
            host.borrow_mut().take_events().is_empty(),
            "Enter must not confirm an invalid line length"
        );
        assert!(
            host.borrow().view.borrow().error.is_some(),
            "an error stays visible in the panel"
        );
    }

    #[test]
    fn escape_emits_cancel() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 4, 8, 8, 4096);
        let mut spec = import_gif_panel_spec(host.clone());
        let _ = run_panel_frame(&ctx, &mut spec.content, vec![key_event(egui::Key::Escape)]);
        assert_eq!(
            host.borrow_mut().take_events(),
            vec![ImportGifEvent::Cancel]
        );
    }

    #[test]
    fn preview_zooms_on_scroll_inside_dynamic_pane() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 14, 16, 16, 4096);
        host.borrow_mut().view.borrow_mut().line_length_text = "4".to_owned();
        host.borrow_mut()
            .set_preview_frames(vec![vec![200u8; 16 * 16 * 4]; 14]);
        let mut spec = import_gif_panel_spec(host);

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

        let out2 = run_panel_frame(
            &ctx,
            &mut spec.content,
            vec![
                egui::Event::PointerMoved(pos2(450.0, 180.0)),
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
    fn take_events_drains() {
        let mut host = ImportGifHost::new();
        host.events.borrow_mut().push(ImportGifEvent::Cancel);
        assert_eq!(host.take_events().len(), 1);
        assert!(host.take_events().is_empty());
    }

    #[test]
    fn no_hardcoded_color_constructors_in_production() {
        let src = include_str!("dock_import_gif_panel.rs");
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
