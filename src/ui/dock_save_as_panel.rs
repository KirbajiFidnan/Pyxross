//! Floating "save as / export" dock panel (id 207).
//!
//! The modal REPLACES the `rfd::FileDialog::save_file()` flow everywhere:
//! on this machine the rfd 0.15.4 portal (ashpd) backend hangs on every
//! native dialog (`save_file()`, `pick_file()`, `pick_folder()` — the dialog
//! never opens and the app freezes). So Save As… / export entry points open
//! THIS in-app panel; the "Browse" location button opens the in-app file
//! chooser panel (id 208, [`crate::ui::dock_file_chooser_panel`]). No native
//! dialogs remain.
//!
//! It mirrors `dock_create_document_panel` (id 205): a float-only, fixed-size
//! dock panel holding a static nest; the in-progress form lives in the host's
//! [`SaveAsView`] cell and the panel is a pure view that only pushes
//! [`SaveAsEvent`]s for the App shell to apply (project saves write a single
//! `.pyxross` archive; exports write their pre-computed payload).
//!
//! Vertical layout (top to bottom): an editable destination **path box**, a
//! left-aligned **Browse** button, the **document-name** box, a **format**
//! button row, the inline error, then **Save** (left-of-center) / **Cancel**
//! (right).
//!
//! Save As ALWAYS writes a single file (the D80 user-facing dir↔file choice
//! was removed): a `.pyxross` suffix is appended when the name lacks one, and
//! an existing DIRECTORY at the target is an inline error (overwriting an
//! existing file is allowed). The final target is
//! `<path from the box>.join(<name with .pyxross>)`; a blank or non-directory
//! path is an inline error, never a panic.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui::vec2;

use crate::ui::Document;
use crate::io::GifFrameInput;
use crate::ui::dock_tool_property_panel::pro_slider;
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
    FLOATING_PANEL_CHROME_HEIGHT,
};

/// Dock id for the save-as/export floating panel.
pub const SAVE_AS_PANEL_ID: u64 = 207;

/// Maximum integer nearest-neighbor upscale factor for image/GIF exports.
pub const MAX_PIXEL_SCALE: u32 = 16;

/// Fixed minimum size of the (non-resizable) floating window. Sized so the
/// path/name rows, optional pixel scale / checkbox, inline error, and bottom
/// actions fit cleanly with zero scrollbars.
const SAVE_AS_MIN_SIZE: egui::Vec2 = vec2(440.0, 218.0 + FLOATING_PANEL_CHROME_HEIGHT);
const SAVE_AS_EXPORT_PREVIEW_MIN_SIZE: egui::Vec2 =
    vec2(680.0, 270.0 + FLOATING_PANEL_CHROME_HEIGHT);
const EXPORT_FORM_PANE_WIDTH: f32 = 340.0;

/// What kind of save/export the modal is currently drafting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveAsKind {
    /// Project Save As…: single `.pyxross` archive.
    Project,
    /// Full-canvas single PNG image export.
    Image,
    /// Sprite-sheet export: PNG plus a sibling `.json` meta file.
    SpriteSheet,
    /// GIF animation export.
    Gif,
    /// Pyxel Edit `.pyxel` project export.
    Pyxel,
    /// Aseprite `.ase` project export.
    Aseprite,
    /// Selection PNG export.
    SelectionPng,
}

impl SaveAsKind {
    /// Panel title (and window title for the floating panel).
    pub fn title(self) -> &'static str {
        match self {
            SaveAsKind::Project => "Save As",
            SaveAsKind::Image => "Export Image",
            SaveAsKind::SpriteSheet => "Export Sprite Sheet",
            SaveAsKind::Gif => "Export GIF",
            SaveAsKind::Pyxel => "Export .pyxel",
            SaveAsKind::Aseprite => "Export .ase",
            SaveAsKind::SelectionPng => "Export Selection PNG",
        }
    }

    /// Default file/dir name suggested when the modal opens (without extension;
    /// the locked extension badge shows and enforces the target extension).
    pub fn default_name(self) -> &'static str {
        match self {
            SaveAsKind::Project => "project",
            SaveAsKind::Image => "image",
            SaveAsKind::SpriteSheet => "sprite_sheet",
            SaveAsKind::Gif => "animation",
            SaveAsKind::Pyxel => "project",
            SaveAsKind::Aseprite => "project",
            SaveAsKind::SelectionPng => "selection",
        }
    }
}

/// True for raster/animated exports that support integer `pixel_scale` upscaling.
pub fn supports_pixel_scale(kind: SaveAsKind) -> bool {
    matches!(
        kind,
        SaveAsKind::Image | SaveAsKind::SpriteSheet | SaveAsKind::Gif | SaveAsKind::SelectionPng
    )
}

/// True for export panels that render a right-hand live preview pane.
pub fn has_export_preview(kind: SaveAsKind) -> bool {
    matches!(
        kind,
        SaveAsKind::Image
            | SaveAsKind::SelectionPng
            | SaveAsKind::Aseprite
            | SaveAsKind::SpriteSheet
            | SaveAsKind::Gif
            | SaveAsKind::Pyxel
    )
}

/// The locked extension badge(s) for `kind`, in display order; the first is
/// selected and enforced on save/export.
pub fn formats_for(kind: SaveAsKind) -> &'static [&'static str] {
    const PROJECT: &[&str] = &[".pyxross"];
    const PNG: &[&str] = &[".png"];
    const GIF: &[&str] = &[".gif"];
    const PYXEL: &[&str] = &[".pyxel"];
    const ASE: &[&str] = &[".ase", ".aseprite"];
    match kind {
        SaveAsKind::Project => PROJECT,
        SaveAsKind::Image | SaveAsKind::SpriteSheet | SaveAsKind::SelectionPng => PNG,
        SaveAsKind::Gif => GIF,
        SaveAsKind::Pyxel => PYXEL,
        SaveAsKind::Aseprite => ASE,
    }
}

/// Nearest-neighbor integer upscaling for RGBA8 buffers (`scale` clamped >= 1).
pub fn scale_rgba_nearest(w: usize, h: usize, rgba: &[u8], scale: u32) -> (usize, usize, Vec<u8>) {
    let scale = scale.max(1) as usize;
    if scale == 1 || w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return (w, h, rgba.to_vec());
    }
    let out_w = w.saturating_mul(scale);
    let out_h = h.saturating_mul(scale);
    let mut out = vec![0u8; out_w.saturating_mul(out_h).saturating_mul(4)];
    for y in 0..h {
        for x in 0..w {
            let src_idx = (y * w + x) * 4;
            let px = &rgba[src_idx..src_idx + 4];
            for sy in 0..scale {
                let dst_y = y * scale + sy;
                let row_off = dst_y * out_w * 4;
                for sx in 0..scale {
                    let dst_x = x * scale + sx;
                    let dst_idx = row_off + dst_x * 4;
                    out[dst_idx..dst_idx + 4].copy_from_slice(px);
                }
            }
        }
    }
    (out_w, out_h, out)
}

/// Pre-computed export payload. Computed ONCE when the export entry point
/// opens the modal (not on every Save press); project saves carry `None` and
/// serialize through `io` when Save is confirmed.
#[derive(Clone, Debug)]
pub enum SavePayload {
    /// Full-canvas RGBA8 pixels for single-PNG export.
    Image { w: usize, h: usize, rgba: Vec<u8> },
    /// Sprite-sheet RGBA8 pixels + source document for scaled `.json` metadata.
    SpriteSheet {
        w: usize,
        h: usize,
        rgba: Vec<u8>,
        doc: Document,
    },
    /// Raw frame buffers + delays for scaled GIF encoding.
    Gif(Vec<GifFrameInput>),
    Pyxel {
        bytes: Vec<u8>,
        w: usize,
        h: usize,
        rgba: Vec<u8>,
    },
    Aseprite {
        preview: Option<crate::core::aseprite::AsepriteExportPreview>,
    },
    SelectionPng { w: usize, h: usize, rgba: Vec<u8> },
}

impl SavePayload {
    /// Base `(width, height)` in pixels before `pixel_scale` is applied.
    pub fn base_dimensions(&self) -> Option<(usize, usize)> {
        match self {
            SavePayload::Image { w, h, .. }
            | SavePayload::SpriteSheet { w, h, .. }
            | SavePayload::SelectionPng { w, h, .. } => Some((*w, *h)),
            SavePayload::Gif(frames) => frames.first().map(|f| (f.width, f.height)),
            SavePayload::Pyxel { w, h, .. } => Some((*w, *h)),
            SavePayload::Aseprite { .. } => None,
        }
    }
}

/// The in-progress save-as/export form values (dialog draft state, not model
/// data). The host owns this cell; the panel edits it in place and reads it
/// back when Save is pressed.
#[derive(Clone, Debug)]
pub struct SaveAsView {
    pub kind: SaveAsKind,
    /// Destination directory the name is joined onto (kept in sync with
    /// [`Self::location_edit`] on Save, and with the "Browse" chooser).
    pub location: PathBuf,
    /// Editable destination path shown in the path box. This is the
    /// authoritative path on Save.
    pub location_edit: String,
    /// The [`Self::location`] value the box was last synced from. Lets the
    /// panel tell "the user is typing" apart from "the chooser changed the
    /// location externally".
    pub synced_location: PathBuf,
    /// User-edited name (file name or directory name).
    pub name: String,
    /// Payload for exports; `None` for project saves.
    pub payload: Option<SavePayload>,
    /// Inline validation/IO error; the panel stays open with this text.
    pub error: Option<String>,
    /// Set by the opener so the panel focuses the name field once.
    pub focus_requested: bool,
    /// When true on `SaveAsKind::Aseprite`, exports the entire canvas as a single frame.
    pub aseprite_unified_frames: bool,
    /// Integer nearest-neighbor upscale factor (`1..=MAX_PIXEL_SCALE`) for
    /// Image, Sprite Sheet, GIF, and Selection PNG exports.
    pub pixel_scale: u32,
    /// Incremented whenever a new export payload is loaded so cached preview
    /// textures are re-uploaded.
    pub preview_generation: u64,
}

impl Default for SaveAsView {
    fn default() -> Self {
        Self {
            kind: SaveAsKind::Project,
            location: PathBuf::new(),
            location_edit: String::new(),
            synced_location: PathBuf::new(),
            name: SaveAsKind::Project.default_name().to_owned(),
            payload: None,
            error: None,
            focus_requested: false,
            aseprite_unified_frames: false,
            pixel_scale: 1,
            preview_generation: 0,
        }
    }
}

/// A save/cancel gesture emitted by the panel.
///
/// `Save` carries the FULLY RESOLVED target (`location.join(final_name)` with
/// the `.pyxross` suffix already enforced for project saves) so the App shell
/// writes without re-deriving anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveAsEvent {
    Save {
        path: PathBuf,
        name: String,
    },
    /// The "Browse" location button was pressed: the App shell opens the
    /// in-app file chooser (ChooseFolder mode) seeded at the current
    /// location — the native `pick_folder` portal dialog hangs on this
    /// machine.
    BrowseLocation,
    Cancel,
}

/// Shared cells for the save-as/export panel.
#[derive(Clone, Default)]
pub struct SaveAsHost {
    pub view: Rc<RefCell<SaveAsView>>,
    pub events: Rc<RefCell<Vec<SaveAsEvent>>>,
}

/// Validation shared between the Save button and the tests: the name must be
/// non-empty (after trimming) and free of path separators.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("name must not be empty".to_owned());
    }
    if name.contains(['/', '\\']) {
        return Err("name must not contain path separators".to_owned());
    }
    Ok(())
}

/// Ensures `name` ends with `ext` (case-insensitive); names are trimmed.
pub fn with_extension(name: &str, ext: &str) -> String {
    let trimmed = name.trim();
    if trimmed.to_ascii_lowercase().ends_with(&ext.to_ascii_lowercase()) {
        trimmed.to_owned()
    } else {
        format!("{trimmed}{ext}")
    }
}

/// Project-name completion: ensure a `.pyxross` suffix is present (Save As
/// always writes a single file).
pub fn with_pyxross_extension(name: &str) -> String {
    with_extension(name, ".pyxross")
}

/// Copies the editable path box into [`SaveAsView::location`], so a typed or
/// pasted folder is honoured on Save (and when seeding the "Browse" chooser).
/// A blank box yields an empty path, which [`checked_target`] rejects.
pub fn apply_location_edit(view: &mut SaveAsView) {
    view.location = PathBuf::from(view.location_edit.trim());
}

/// The final target `location.join(name)`.
pub fn target_path(location: &Path, name: &str) -> PathBuf {
    location.join(name)
}

/// Resolves the final `(path, final_name)` for the current view, running all
/// inline validation:
/// - empty name or path separators → error;
/// - the destination path is blank or not an existing directory → error;
/// - an existing DIRECTORY at the target → error (overwriting an existing
///   file is allowed);
/// - project names get a `.pyxross` suffix appended first (Save As always
///   writes a single file).
pub fn checked_target(view: &SaveAsView) -> Result<(PathBuf, String), String> {
    validate_name(&view.name)?;
    if view.location.as_os_str().is_empty() || !view.location.is_dir() {
        return Err(format!(
            "'{}' is not an existing folder",
            view.location.display()
        ));
    }
    let final_name = if let Some(ext) = formats_for(view.kind).first() {
        with_extension(&view.name, ext)
    } else {
        view.name.trim().to_owned()
    };
    let final_path = target_path(&view.location, &final_name);
    if final_path.is_dir() {
        return Err(format!("a directory already exists at {}", final_path.display()));
    }
    Ok((final_path, final_name))
}

/// Builds the [`SaveAsEvent::Save`] for the current view, or the inline error.
/// The panel calls this from the Save button; keeping it free makes the full
/// validation + completion chain testable without egui.
pub fn commit_save(view: &SaveAsView) -> Result<SaveAsEvent, String> {
    let (path, name) = checked_target(view)?;
    Ok(SaveAsEvent::Save { path, name })
}

/// The save-as/export form: path box, Browse, name box, format row, error
/// line, Save/Cancel, plus right-hand preview for export kinds.
struct SaveAsComponent {
    host: SaveAsHost,
    preview_gen: u64,
    preview_viewport: crate::ui::panel_dock::preview::PreviewViewport,
    single_tex: Option<(usize, usize, egui::TextureHandle)>,
    frame_texs: Vec<(usize, usize, egui::TextureHandle)>,
}

impl SaveAsComponent {
    fn new(host: SaveAsHost) -> Self {
        Self {
            host,
            preview_gen: u64::MAX,
            preview_viewport: crate::ui::panel_dock::preview::PreviewViewport::new(),
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

    fn sync_export_textures(&mut self, ctx: &egui::Context) {
        let view = self.host.view.borrow();
        if self.preview_gen == view.preview_generation {
            return;
        }
        self.preview_gen = view.preview_generation;
        self.preview_viewport.sync_generation(view.preview_generation);
        self.single_tex = None;
        self.frame_texs.clear();

        match &view.payload {
            Some(
                SavePayload::Image { w, h, rgba }
                | SavePayload::SelectionPng { w, h, rgba }
                | SavePayload::SpriteSheet { w, h, rgba, .. }
                | SavePayload::Pyxel { w, h, rgba, .. },
            ) => {
                self.single_tex =
                    Self::upload_rgba(ctx, "export-single-preview", *w, *h, rgba);
            }
            Some(SavePayload::Gif(frames)) => {
                for (idx, frame) in frames.iter().enumerate() {
                    if let Some(tex) = Self::upload_rgba(
                        ctx,
                        &format!("export-gif-frame-preview-{idx}"),
                        frame.width,
                        frame.height,
                        &frame.rgba,
                    ) {
                        self.frame_texs.push(tex);
                    }
                }
            }
            Some(SavePayload::Aseprite {
                preview: Some(preview),
                ..
            }) => {
                self.single_tex = Self::upload_rgba(
                    ctx,
                    "export-aseprite-unified-preview",
                    preview.unified_w,
                    preview.unified_h,
                    &preview.unified_rgba,
                );
                for (idx, (fw, fh, rgba)) in preview.frames.iter().enumerate() {
                    if let Some(tex) = Self::upload_rgba(
                        ctx,
                        &format!("export-aseprite-frame-preview-{idx}"),
                        *fw,
                        *fh,
                        rgba,
                    ) {
                        self.frame_texs.push(tex);
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_right_preview(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.sync_export_textures(ui.ctx());
        let (kind, unified_frames, pixel_scale, caption) = {
            let v = self.host.view.borrow();
            let scale = v.pixel_scale.clamp(1, MAX_PIXEL_SCALE) as usize;
            let cap = match (v.kind, &v.payload) {
                (
                    SaveAsKind::Aseprite,
                    Some(SavePayload::Aseprite {
                        preview: Some(p), ..
                    }),
                ) => {
                    if v.aseprite_unified_frames {
                        format!(
                            "Preview · Unified: 1 frame ({}×{} px)",
                            p.unified_w, p.unified_h
                        )
                    } else {
                        format!(
                            "Preview · Timeline: {} frame(s) ({}×{} px)",
                            p.total_timeline_frames, p.frame_w, p.frame_h
                        )
                    }
                }
                (SaveAsKind::Gif, Some(SavePayload::Gif(frames))) => {
                    let (fw, fh) = frames.first().map_or((0, 0), |f| (f.width, f.height));
                    format!(
                        "Preview · {} frame(s) ({}×{} px)",
                        frames.len(),
                        fw * scale,
                        fh * scale
                    )
                }
                (
                    _,
                    Some(
                        SavePayload::Image { w, h, .. }
                        | SavePayload::SelectionPng { w, h, .. }
                        | SavePayload::SpriteSheet { w, h, .. }
                        | SavePayload::Pyxel { w, h, .. },
                    ),
                ) => format!("Preview · {} × {} px", w * scale, h * scale),
                _ => "Preview".to_owned(),
            };
            (v.kind, v.aseprite_unified_frames, scale, cap)
        };
        let _ = pixel_scale;

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
            egui::Rect::from_min_max(inner.min, egui::pos2(inner.max.x, inner.min.y + 18.0));
        let footer_h = 22.0;
        let img_area = egui::Rect::from_min_max(
            egui::pos2(inner.min.x, header_rect.max.y + 6.0),
            egui::pos2(inner.max.x, (inner.max.y - footer_h - 4.0).max(header_rect.max.y + 12.0)),
        );
        let footer_rect = egui::Rect::from_min_max(
            egui::pos2(inner.min.x, img_area.max.y + 4.0),
            inner.max,
        );

        painter.text(
            header_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            caption,
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );

        let interact_resp = ui.interact(
            img_area,
            ui.id().with("export-preview-viewport"),
            egui::Sense::click_and_drag(),
        );
        self.preview_viewport
            .handle_interaction(ui, img_area, &interact_resp);

        let clipped = ui.painter_at(img_area);
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        let show_multi_frames = (kind == SaveAsKind::Aseprite && !unified_frames)
            || (kind == SaveAsKind::Gif && self.frame_texs.len() > 1);

        if show_multi_frames {
            let count = self.frame_texs.len();
            if count > 0 {
                let gap = 8.0;
                let slot_w = ((img_area.width() - gap * (count.saturating_sub(1) as f32))
                    / count as f32)
                    .max(20.0);
                for (idx, (fw, fh, tex)) in self.frame_texs.iter().enumerate() {
                    let x0 = img_area.min.x + idx as f32 * (slot_w + gap);
                    let slot = egui::Rect::from_min_size(
                        egui::pos2(x0, img_area.min.y),
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
        } else {
            let single = self
                .single_tex
                .as_ref()
                .or_else(|| self.frame_texs.first());
            if let Some((w, h, tex)) = single {
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

        self.preview_viewport.draw_footer(ui, chrome, footer_rect);
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

impl SaveAsComponent {
    fn draw_form_pane(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        // Copy the form out of the host cell so the borrow is released before
        // the widgets run; edited values are written back at the end.
        let (
            kind,
            location,
            mut name,
            error,
            focus_requested,
            mut location_edit,
            mut synced,
            mut aseprite_unified_frames,
            mut pixel_scale,
            base_dims,
        ) = {
            let v = self.host.view.borrow();
            (
                v.kind,
                v.location.clone(),
                v.name.clone(),
                v.error.clone(),
                v.focus_requested,
                v.location_edit.clone(),
                v.synced_location.clone(),
                v.aseprite_unified_frames,
                v.pixel_scale.clamp(1, MAX_PIXEL_SCALE),
                v.payload.as_ref().and_then(SavePayload::base_dimensions),
            )
        };

        // Keep the editable path in sync with the stored location: on open
        // (`focus_requested`) and whenever an external actor (the File Manager
        // "Browse" chooser) updates it. User typing only edits `location_edit`,
        // so it is never clobbered here.
        if focus_requested || location != synced {
            location_edit = location.display().to_string();
            synced = location.clone();
        }

        // Destination path: label above, path input + Browse button on one aligned row.
        ui.strong("Save path:");
        ui.add_space(4.0);
        let mut browse_clicked = false;
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

        ui.add_space(10.0);

        // Document-name row: label above, name input + format badge on one aligned row.
        ui.strong("Document name:");
        ui.add_space(4.0);
        let formats = formats_for(kind);
        let name_hint = if kind == SaveAsKind::Project {
            "project or document name"
        } else {
            "file name"
        };
        let name_response = if !formats.is_empty() {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    for (index, label) in formats.iter().enumerate() {
                        let _ = chrome.button(ui, label, ButtonStyle::toggled(index == 0));
                    }
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .hint_text(name_hint)
                            .desired_width(ui.available_width()),
                    )
                })
                .inner
            })
            .inner
        } else {
            ui.add(
                egui::TextEdit::singleline(&mut name)
                    .hint_text(name_hint)
                    .desired_width(f32::INFINITY),
            )
        };
        if focus_requested {
            name_response.request_focus();
        }

        if supports_pixel_scale(kind) {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.strong("Pixel scale:");
                if let Some((bw, bh)) = base_dims {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(format!(
                            "{} × {} px",
                            bw.saturating_mul(pixel_scale as usize),
                            bh.saturating_mul(pixel_scale as usize)
                        ));
                    });
                }
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
                            "export-pixel-scale",
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
        }

        if kind == SaveAsKind::Aseprite {
            ui.add_space(6.0);
            ui.checkbox(&mut aseprite_unified_frames, "Unified frames")
                .on_hover_text("Export the entire canvas as a single frame");
        }

        // Inline error line (theme error color, panel stays open).
        if let Some(err) = &error {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        // Bottom action row: right-aligned primary Save + Cancel matching Create New Document.
        let mut save_clicked = false;
        let mut cancel_clicked = false;
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome
                    .button(ui, "Save", ButtonStyle::toggled(true))
                    .clicked()
                {
                    save_clicked = true;
                }
                ui.add_space(6.0);
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    cancel_clicked = true;
                }
            });
        });

        // Enter = Save, Esc = Cancel (kept from the old panel).
        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed =
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        // Write the edited form back into the host cell.
        let mut view = self.host.view.borrow_mut();
        view.location_edit = location_edit;
        view.synced_location = synced;
        view.name = name;
        view.focus_requested = false;
        view.aseprite_unified_frames = aseprite_unified_frames;
        view.pixel_scale = pixel_scale.clamp(1, MAX_PIXEL_SCALE);

        if browse_clicked {
            // Seed the chooser at the path currently in the box.
            apply_location_edit(&mut view);
            view.synced_location = view.location.clone();
            drop(view);
            self.host
                .events
                .borrow_mut()
                .push(SaveAsEvent::BrowseLocation);
            return;
        }

        if save_clicked || enter_pressed {
            // The path box is authoritative on Save.
            apply_location_edit(&mut view);
            view.synced_location = view.location.clone();
            match commit_save(&view) {
                Ok(event) => {
                    view.error = None;
                    self.host.events.borrow_mut().push(event);
                }
                Err(message) => view.error = Some(message),
            }
        } else if cancel_clicked || escape_pressed {
            self.host.events.borrow_mut().push(SaveAsEvent::Cancel);
        }
    }
}

impl Component for SaveAsComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let kind = self.host.view.borrow().kind;
        if !has_export_preview(kind) {
            self.draw_form_pane(ui, chrome);
            return;
        }
        let total_h = ui.available_height().max(240.0);
        ui.horizontal_top(|ui| {
            ui.set_min_height(total_h);
            ui.allocate_ui_with_layout(
                vec2(EXPORT_FORM_PANE_WIDTH, total_h),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    self.draw_form_pane(ui, chrome);
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

/// Panel content: the static nest holding the save-as/export form.
struct SaveAsPanelContent {
    nest: NestContent,
}

impl PanelContent for SaveAsPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the save-as/export form.
pub(crate) fn build_save_as_nest(host: &SaveAsHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(SaveAsComponent::new(host.clone()));
    nest
}

/// Dock spec for the save-as/export floating panel (dock id 207).
///
/// **Float-only:** `can_pop_out == false` (no Float/Dock button, not dockable);
/// `resizable == false` keeps the window fixed-size and draggable only by its
/// header. The title follows the kind the host is drafting.
pub fn save_as_panel_spec(host: &SaveAsHost, floating_rect: egui::Rect) -> PanelSpec {
    let view = host.view.borrow();
    let title = view.kind.title();
    let min_size = if has_export_preview(view.kind) {
        SAVE_AS_EXPORT_PREVIEW_MIN_SIZE
    } else {
        SAVE_AS_MIN_SIZE
    };
    drop(view);
    let floating_rect = egui::Rect::from_min_size(
        floating_rect.min,
        vec2(
            floating_rect.width().max(min_size.x),
            floating_rect.height().max(min_size.y),
        ),
    );
    PanelSpec {
        id: PanelId::new(SAVE_AS_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(title, false, min_size)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(SaveAsPanelContent {
            nest: build_save_as_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique scratch dir per test (process id + tag), removed on drop of
    /// the returned guard by the caller via `remove_dir_all`.
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pyx_saveas_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A view whose destination is the (existing) system temp dir.
    fn view(kind: SaveAsKind) -> SaveAsView {
        let location = std::env::temp_dir();
        SaveAsView {
            kind,
            location: location.clone(),
            location_edit: location.display().to_string(),
            synced_location: location,
            name: kind.default_name().to_owned(),
            payload: None,
            error: None,
            focus_requested: false,
            aseprite_unified_frames: false,
            pixel_scale: 1,
            preview_generation: 0,
        }
    }

    #[test]
    fn validate_name_rejects_empty_and_separators() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name(r"a\b").is_err());
        assert!(validate_name("doc.pyxross").is_ok());
    }

    #[test]
    fn with_pyxross_extension_appends_exactly_once() {
        assert_eq!(with_pyxross_extension("doc"), "doc.pyxross");
        assert_eq!(with_pyxross_extension("  doc  "), "doc.pyxross");
        assert_eq!(with_pyxross_extension("doc.pyxross"), "doc.pyxross");
        assert_eq!(with_pyxross_extension("a.b"), "a.b.pyxross");
    }

    #[test]
    fn checked_target_project_appends_pyxross_and_rejects_dirs() {
        let dir = scratch_dir("project");
        let mut v = view(SaveAsKind::Project);
        v.location = dir.clone();
        v.name = "my-sprite".to_owned();
        let (path, name) = checked_target(&v).unwrap();
        assert_eq!(path, dir.join("my-sprite.pyxross"));
        assert_eq!(name, "my-sprite.pyxross");

        // An existing DIRECTORY at the file target is an error.
        std::fs::create_dir_all(dir.join("blocked.pyxross")).unwrap();
        let mut blocked = view(SaveAsKind::Project);
        blocked.location = dir.clone();
        blocked.name = "blocked".to_owned();
        let err = checked_target(&blocked).unwrap_err();
        assert!(err.contains("a directory already exists"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checked_target_allows_overwriting_an_existing_file() {
        let dir = scratch_dir("overwrite");
        let existing = dir.join("saveas_overwrite.pyxross");
        std::fs::write(&existing, b"old").unwrap();
        let mut v = view(SaveAsKind::Project);
        v.location = dir.clone();
        v.name = "saveas_overwrite".to_owned();
        let (path, name) = checked_target(&v).unwrap();
        assert_eq!(path, existing);
        assert_eq!(name, "saveas_overwrite.pyxross");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checked_target_export_kind_never_applies_pyxross() {
        let dir = scratch_dir("export");
        let mut v = view(SaveAsKind::SpriteSheet);
        v.location = dir.clone();
        v.name = "sheet.png".to_owned();
        let (path, name) = checked_target(&v).unwrap();
        assert_eq!(path, dir.join("sheet.png"));
        assert_eq!(name, "sheet.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checked_target_rejects_a_missing_or_blank_location() {
        let mut missing = view(SaveAsKind::Project);
        missing.location = PathBuf::from("/definitely/missing/saveas_dir");
        missing.name = "doc".to_owned();
        let err = checked_target(&missing).unwrap_err();
        assert!(err.contains("not an existing folder"), "{err}");

        let mut blank = view(SaveAsKind::Project);
        blank.location = PathBuf::new();
        blank.name = "doc".to_owned();
        assert!(checked_target(&blank).is_err());
    }

    #[test]
    fn save_resolves_the_typed_path_from_the_box() {
        let dir = scratch_dir("typedpath");
        let mut v = view(SaveAsKind::Project);
        v.name = "doc".to_owned();
        // The box is authoritative: copy it into `location` first.
        v.location_edit = dir.display().to_string();
        apply_location_edit(&mut v);
        let (path, name) = checked_target(&v).unwrap();
        assert_eq!(path, dir.join("doc.pyxross"));
        assert_eq!(name, "doc.pyxross");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_location_edit_blank_box_yields_an_empty_location() {
        let mut v = view(SaveAsKind::Project);
        v.location_edit = "   ".to_owned();
        apply_location_edit(&mut v);
        assert!(v.location.as_os_str().is_empty());
        assert!(checked_target(&v).is_err());
    }

    #[test]
    fn commit_save_carries_the_resolved_file_path() {
        let dir = scratch_dir("commit");
        let mut v = view(SaveAsKind::Project);
        v.location = dir.clone();
        v.name = "proj".to_owned();
        match commit_save(&v).unwrap() {
            SaveAsEvent::Save { path, name } => {
                assert_eq!(path, dir.join("proj.pyxross"));
                assert_eq!(name, "proj.pyxross");
            }
            SaveAsEvent::Cancel => panic!("expected Save"),
            SaveAsEvent::BrowseLocation => panic!("expected Save"),
        }
        // Empty name keeps the error inline instead of emitting.
        v.name = " ".to_owned();
        assert!(commit_save(&v).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn formats_for_every_kind_returns_locked_extension() {
        assert_eq!(formats_for(SaveAsKind::Project), &[".pyxross"][..]);
        assert_eq!(formats_for(SaveAsKind::Image), &[".png"][..]);
        assert_eq!(formats_for(SaveAsKind::SpriteSheet), &[".png"][..]);
        assert_eq!(formats_for(SaveAsKind::SelectionPng), &[".png"][..]);
        assert_eq!(formats_for(SaveAsKind::Gif), &[".gif"][..]);
        assert_eq!(formats_for(SaveAsKind::Pyxel), &[".pyxel"][..]);
        assert_eq!(formats_for(SaveAsKind::Aseprite), &[".ase", ".aseprite"][..]);
    }

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = SaveAsHost::default();
        {
            let mut v = host.view.borrow_mut();
            v.kind = SaveAsKind::SpriteSheet;
            v.location = PathBuf::from("/tmp");
            v.name = "sheet.png".to_owned();
        }
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), vec2(380.0, 210.0));
        let spec = save_as_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), SAVE_AS_PANEL_ID);
        assert_eq!(spec.metadata.title, "Export Sprite Sheet");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        let nest = build_save_as_nest(&host);
        assert_eq!(nest.mode(), NestMode::Static);
    }

    #[test]
    fn kind_titles_and_default_names_cover_every_kind() {
        use SaveAsKind::*;
        let cases = [
            (Project, "Save As", "project"),
            (Image, "Export Image", "image"),
            (SpriteSheet, "Export Sprite Sheet", "sprite_sheet"),
            (Gif, "Export GIF", "animation"),
            (Pyxel, "Export .pyxel", "project"),
            (Aseprite, "Export .ase", "project"),
            (SelectionPng, "Export Selection PNG", "selection"),
        ];
        for (kind, title, default_name) in cases {
            assert_eq!(kind.title(), title);
            assert_eq!(kind.default_name(), default_name);
        }
    }

    /// Drive one headless frame of the save-as component.
    fn run_component_frame(
        ctx: &egui::Context,
        host: &SaveAsHost,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(420.0, 220.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome =
            PanelChrome::flat(&theme, &capture, crate::ui::input_capture::SurfaceId::Canvas);
        let mut component = SaveAsComponent::new(host.clone());
        let mut output = ctx.run_ui(raw, |ui| {
            let view = NestView::screen(ui.max_rect());
            component.ui(ui, &view, &chrome);
        });
        output.textures_delta.clear();
        output
    }

    /// Top-left positions of every rendered text equal to `text`.
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
    fn component_paints_labels_browse_name_and_format_button() {
        let ctx = egui::Context::default();
        let host = SaveAsHost::default();
        {
            let mut v = host.view.borrow_mut();
            v.kind = SaveAsKind::Project;
            v.location = std::env::temp_dir();
            v.location_edit = std::env::temp_dir().display().to_string();
            v.name = "project".to_owned();
        }
        let output = run_component_frame(&ctx, &host, Vec::new());
        assert!(!output.shapes.is_empty(), "the panel paints content");
        assert!(
            !text_positions(&output, "Save path:").is_empty(),
            "the path label is painted"
        );
        assert!(
            !text_positions(&output, "Document name:").is_empty(),
            "the document-name label is painted"
        );
        assert!(
            !text_positions(&output, "Browse").is_empty(),
            "the Browse button is painted"
        );
        assert!(
            !text_positions(&output, ".pyxross").is_empty(),
            "the selected format button is painted"
        );
        assert!(
            !text_positions(&output, "Save").is_empty()
                && !text_positions(&output, "Cancel").is_empty(),
            "the action row is painted"
        );

        // Save and Cancel are right-aligned on the bottom action row (Cancel
        // left of Save) matching the Create New Document dialog.
        let save_pos = text_positions(&output, "Save")
            .into_iter()
            .next()
            .expect("Save is painted");
        let cancel_pos = text_positions(&output, "Cancel")
            .into_iter()
            .next()
            .expect("Cancel is painted");
        assert!(
            save_pos.x > cancel_pos.x,
            "Save ({}) sits to the right of Cancel ({})",
            save_pos.x,
            cancel_pos.x
        );
        assert!(
            (save_pos.y - cancel_pos.y).abs() <= 4.0,
            "Save ({}) and Cancel ({}) share the same action row",
            save_pos.y,
            cancel_pos.y
        );
    }

    #[test]
    fn save_as_panel_fits_without_overflow() {
        let host = SaveAsHost::default();
        {
            let mut v = host.view.borrow_mut();
            v.kind = SaveAsKind::Project;
            v.location = std::env::temp_dir();
            v.location_edit = std::env::temp_dir().display().to_string();
            v.name = "Untitled".to_owned();
            v.error = Some("Sample validation message".to_owned());
        }
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), SAVE_AS_MIN_SIZE);
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(SAVE_AS_PANEL_ID)),
        );
        let chrome_h = FLOATING_PANEL_CHROME_HEIGHT;
        let viewport_size = vec2(rect.width() - 22.0, rect.height() - chrome_h);
        let body_rect = egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            vec2(
                viewport_size.x + 2.0 * crate::ui::panel_dock::nest::NEST_PADDING,
                viewport_size.y + 2.0 * crate::ui::panel_dock::nest::NEST_PADDING,
            ),
        );
        let mut measured_content_size = egui::Vec2::ZERO;
        let raw = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(body_rect), |body| {
                let nest_vp =
                    crate::ui::panel_dock::nest::nest_viewport(body.available_rect_before_wrap());
                body.scope_builder(egui::UiBuilder::new().max_rect(nest_vp), |inner| {
                    let out = egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .min_scrolled_width(0.0)
                        .min_scrolled_height(0.0)
                        .show(inner, |scroll| {
                            let view = NestView::screen(nest_vp);
                            let mut comp = SaveAsComponent::new(host.clone());
                            comp.ui(scroll, &view, &chrome);
                        });
                    measured_content_size = out.content_size;
                });
            });
        })
        .textures_delta
        .clear();
        assert!(
            measured_content_size.y <= viewport_size.y,
            "Save As content height ({}) must fit inside viewport ({}) without vertical scrollbar",
            measured_content_size.y,
            viewport_size.y
        );
        assert!(
            measured_content_size.x <= viewport_size.x,
            "Save As content width ({}) must fit inside viewport ({}) without horizontal scrollbar",
            measured_content_size.x,
            viewport_size.x
        );
    }

    #[test]
    fn locked_extension_badge_is_painted_for_export_kinds() {
        let ctx = egui::Context::default();
        let host = SaveAsHost::default();
        {
            let mut v = host.view.borrow_mut();
            v.kind = SaveAsKind::Gif;
            v.location = std::env::temp_dir();
            v.location_edit = std::env::temp_dir().display().to_string();
            v.name = "animation".to_owned();
        }
        let output = run_component_frame(&ctx, &host, Vec::new());
        assert!(
            !text_positions(&output, ".gif").is_empty(),
            "GIF export shows the locked .gif extension badge"
        );
    }

    #[test]
    fn scale_rgba_nearest_upscales_pixels_crisply() {
        let src = vec![
            10, 20, 30, 255, // (0,0)
            40, 50, 60, 255, // (1,0)
        ];
        let (w, h, out) = scale_rgba_nearest(2, 1, &src, 3);
        assert_eq!((w, h), (6, 3));
        assert_eq!(out.len(), 6 * 3 * 4);
        // Top-left 3x3 block matches first pixel.
        for y in 0..3 {
            for x in 0..3 {
                let idx = (y * 6 + x) * 4;
                assert_eq!(&out[idx..idx + 4], &[10, 20, 30, 255]);
            }
            for x in 3..6 {
                let idx = (y * 6 + x) * 4;
                assert_eq!(&out[idx..idx + 4], &[40, 50, 60, 255]);
            }
        }
    }

    #[test]
    fn aseprite_export_panel_shows_timeline_and_unified_preview_captions() {
        let ctx = egui::Context::default();
        let host = SaveAsHost::default();
        {
            let mut v = host.view.borrow_mut();
            v.kind = SaveAsKind::Aseprite;
            v.location = std::env::temp_dir();
            v.location_edit = std::env::temp_dir().display().to_string();
            v.name = "sprite".to_owned();
            v.aseprite_unified_frames = false;
            v.payload = Some(SavePayload::Aseprite {
                preview: Some(crate::core::aseprite::AsepriteExportPreview {
                    unified_w: 64,
                    unified_h: 16,
                    unified_rgba: vec![255; 64 * 16 * 4],
                    frames: vec![
                        (16, 16, vec![255; 16 * 16 * 4]),
                        (16, 16, vec![255; 16 * 16 * 4]),
                        (16, 16, vec![255; 16 * 16 * 4]),
                        (16, 16, vec![255; 16 * 16 * 4]),
                    ],
                    total_timeline_frames: 6,
                    frame_w: 16,
                    frame_h: 16,
                }),
            });
        }
        let out_timeline = run_component_frame(&ctx, &host, Vec::new());
        assert!(
            !text_positions(
                &out_timeline,
                "Preview · Timeline: 6 frame(s) (16×16 px)"
            )
            .is_empty(),
            "timeline preview caption must be shown when unified frames is off"
        );

        host.view.borrow_mut().aseprite_unified_frames = true;
        let out_unified = run_component_frame(&ctx, &host, Vec::new());
        assert!(
            !text_positions(&out_unified, "Preview · Unified: 1 frame (64×16 px)").is_empty(),
            "unified preview caption must be shown when unified frames is on"
        );
    }
}
