//! Floating "Import Image" dock panel (id 210).
//!
//! The split step of the image import flow: the in-app File Manager (id 208,
//! mode [`crate::ui::dock_file_chooser_panel::FileChooserMode::ImportPng`])
//! picks a `.png`; the App decodes it and opens THIS panel to choose how the
//! image is cut into import regions. On OK the shell builds the region grid via
//! [`crate::core::import`].
//!
//! The two fields are LIVE-LINKED: a valid edit to one immediately re-derives
//! the other (pixels ↔ regions), and the panel confirms a CONSISTENT pair —
//! whichever field the user last edited is authoritative. The `syncing` flag
//! marks a programmatic derivation so it never re-triggers the reverse.
//!
//! It mirrors `dock_save_as_panel` (id 207): a float-only, fixed-size dock
//! panel holding a static nest; the form lives in [`ImportImageView`] and the
//! panel is a pure view that only pushes [`ImportImageEvent`]s. Enter = OK
//! (when valid), Esc = Cancel.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import Image floating panel.
pub const IMPORT_IMAGE_PANEL_ID: u64 = 210;

/// Panel/window title.
const IMPORT_IMAGE_TITLE: &str = "Import Image";

/// Fixed minimum size of the (non-resizable) floating window: two labelled
/// boxes, the inline error line and the OK / Cancel row.
const IMPORT_IMAGE_MIN_SIZE: egui::Vec2 = vec2(340.0, 220.0);

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
    /// Editable "Split by pixels" text (`"WxH"`).
    pub pixels_text: String,
    /// Editable "Split by number of regions" text (`"WxH"`).
    pub regions_text: String,
    /// Which field the user last edited (authoritative on OK).
    pub last_edited: ImportImageField,
    /// Set while a programmatic derivation writes the OTHER box, so the write
    /// is never mistaken for a user edit (no derivation loop).
    pub syncing: bool,
    /// Host-injected error (e.g. the decoder failed); shown while present.
    pub error: Option<String>,
    /// Canvas-size / region-count limits.
    pub max_canvas_w: u32,
    pub max_canvas_h: u32,
    pub max_frames: usize,
}

impl Default for ImportImageView {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            img_w: 0,
            img_h: 0,
            pixels_text: String::new(),
            regions_text: String::new(),
            last_edited: ImportImageField::Pixels,
            syncing: false,
            error: None,
            max_canvas_w: 4096,
            max_canvas_h: 4096,
            max_frames: 4096,
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

    /// Seeds the panel for a freshly decoded image: the pixels box starts at
    /// the full image size and the regions box at its derived `1x1`.
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
        view.pixels_text = format!("{img_w}x{img_h}");
        view.regions_text = derive_regions_text(img_w, img_h, img_w, img_h);
        view.last_edited = ImportImageField::Pixels;
        view.syncing = false;
        view.error = None;
        view.max_canvas_w = max_canvas_w;
        view.max_canvas_h = max_canvas_h;
        view.max_frames = max_frames;
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

/// Parses a `"WxH"` pair: split on a single `x`/`X`, trim each side, both axes
/// must parse as `u32 >= 1`. Anything else (missing separator, extra parts,
/// zero, non-numeric) is `None`.
pub fn parse_dims(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().split(['x', 'X']);
    let w = parts.next()?.trim().parse::<u32>().ok()?;
    let h = parts.next()?.trim().parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if w == 0 || h == 0 {
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
            let (pw, ph) = parse_dims(&view.pixels_text)
                .ok_or_else(|| "Pixel split must be WxH, both at least 1".to_owned())?;
            let (rw, rh) = crate::core::import::regions_from_pixels(view.img_w, view.img_h, pw, ph);
            Ok((pw, ph, rw, rh))
        }
        ImportImageField::Regions => {
            let (rw, rh) = parse_dims(&view.regions_text)
                .ok_or_else(|| "Region split must be WxH, both at least 1".to_owned())?;
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

/// Applies a user edit to the pixels box: the field becomes authoritative
/// (even when the text is currently unparseable, so the OK button disables).
/// When valid, the regions box is re-derived (marked `syncing` so the
/// derivation never re-triggers); when invalid, nothing else changes.
pub fn on_pixels_edited(view: &mut ImportImageView) {
    view.last_edited = ImportImageField::Pixels;
    if let Some((pw, ph)) = parse_dims(&view.pixels_text) {
        view.syncing = true;
        view.regions_text = derive_regions_text(view.img_w, view.img_h, pw, ph);
        view.syncing = false;
    }
}

/// [`on_pixels_edited`] for the regions box.
pub fn on_regions_edited(view: &mut ImportImageView) {
    view.last_edited = ImportImageField::Regions;
    if let Some((rw, rh)) = parse_dims(&view.regions_text) {
        view.syncing = true;
        view.pixels_text = derive_pixels_text(view.img_w, view.img_h, rw, rh);
        view.syncing = false;
    }
}

/// The Import Image form: two labelled, centred boxes, the inline error and a
/// symmetric centred OK / Cancel row.
struct ImportImageComponent {
    host: Rc<RefCell<ImportImageHost>>,
}

impl Component for ImportImageComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let (mut pixels_text, mut regions_text) = {
            let host = self.host.borrow();
            let v = host.view.borrow();
            (v.pixels_text.clone(), v.regions_text.clone())
        };

        ui.label("Split by pixels");
        let pixels_response = ui
            .vertical_centered(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut pixels_text)
                        .hint_text("WxH")
                        .desired_width(160.0),
                )
            })
            .inner;
        ui.label("Split by number of regions");
        let regions_response = ui
            .vertical_centered(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut regions_text)
                        .hint_text("WxH")
                        .desired_width(160.0),
                )
            })
            .inner;

        // Write the raw texts back, then apply the field the user edited. The
        // `syncing` guard (set inside `on_*_edited`) keeps the programmatic
        // derivation from being read back as an edit.
        {
            let host = self.host.borrow();
            let mut v = host.view.borrow_mut();
            v.pixels_text = pixels_text;
            v.regions_text = regions_text;
            if !v.syncing {
                if pixels_response.changed() {
                    on_pixels_edited(&mut v);
                } else if regions_response.changed() {
                    on_regions_edited(&mut v);
                }
            }
        }

        // Live validation drives the OK enablement; a host-injected error wins
        // for display, otherwise the live validation message shows.
        let host = self.host.borrow();
        let (live, host_error) = {
            let v = host.view.borrow();
            (validate_image(&v), v.error.clone())
        };
        let valid = live.is_ok();
        let shown_error = host_error.or_else(|| live.clone().err());
        if let Some(err) = &shown_error {
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        }

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
            // Invalid input: the live inline error already shows; OK stays off.
        }
    }
}

/// Panel content: the static nest holding the Import Image form.
struct ImportImagePanelContent {
    nest: NestContent,
}

impl PanelContent for ImportImagePanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the Import Image form.
pub(crate) fn build_import_image_nest(host: &Rc<RefCell<ImportImageHost>>) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ImportImageComponent { host: host.clone() });
    nest
}

/// Dock spec for the Import Image floating panel (dock id 210).
///
/// **Float-only:** `can_pop_out == false`; `resizable == false` keeps the
/// window fixed-size and draggable only by its header.
pub fn import_image_panel_spec(host: Rc<RefCell<ImportImageHost>>) -> PanelSpec {
    let floating_rect = egui::Rect::from_min_size(egui::pos2(160.0, 140.0), IMPORT_IMAGE_MIN_SIZE);
    PanelSpec {
        id: PanelId::new(IMPORT_IMAGE_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(IMPORT_IMAGE_TITLE, false, IMPORT_IMAGE_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ImportImagePanelContent {
            nest: build_import_image_nest(&host),
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
        assert_eq!(v.pixels_text, "64x64");
        assert_eq!(v.regions_text, "1x1");
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
            v.pixels_text = "16x16".to_owned();
            on_pixels_edited(&mut v);
            assert_eq!(v.regions_text, "4x4");
        }
        // pixels 20x20 -> regions 4x4 (ceil)
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.pixels_text = "20x20".to_owned();
            on_pixels_edited(&mut v);
            assert_eq!(v.regions_text, "4x4");
        }
    }

    #[test]
    fn regions_edit_derives_pixels_both_directions() {
        // regions 4x1 -> pixels 16x64
        let host = host_rc_seeded();
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.regions_text = "4x1".to_owned();
            on_regions_edited(&mut v);
            assert_eq!(v.pixels_text, "16x64");
        }
        // regions 3x3 -> pixels 22x22
        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.regions_text = "3x3".to_owned();
            on_regions_edited(&mut v);
            assert_eq!(v.pixels_text, "22x22");
        }
    }

    #[test]
    fn editing_does_not_loop_and_marks_the_edited_field() {
        let host = host_rc_seeded();
        let h = host.borrow();
        let mut v = h.view.borrow_mut();
        v.pixels_text = "16x16".to_owned();
        on_pixels_edited(&mut v);
        assert_eq!(v.last_edited, ImportImageField::Pixels);
        assert_eq!(v.regions_text, "4x4", "the other box is derived");
        assert_eq!(v.pixels_text, "16x16", "the edited box is untouched");
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
        v.regions_text = "abc".to_owned();
        let before_pixels = v.pixels_text.clone();
        on_regions_edited(&mut v);
        assert_eq!(
            v.pixels_text, before_pixels,
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
            v.regions_text = "3x3".to_owned();
            on_regions_edited(&mut v);
        }
        // Region-driven: 3x3 -> 22x22 pixels.
        let (px_w, px_h, rw, rh) = validate_image(&host.borrow().view.borrow()).unwrap();
        assert_eq!((px_w, px_h, rw, rh), (22, 22, 3, 3));

        {
            let h = host.borrow();
            let mut v = h.view.borrow_mut();
            v.pixels_text = "16x16".to_owned();
            on_pixels_edited(&mut v);
        }
        // Pixel-driven: 16x16 -> 4x4 regions.
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
            v.regions_text = "3x3".to_owned();
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
            v.pixels_text = "20x20".to_owned();
            on_pixels_edited(&mut v);
        }
        let err = validate_image(&host.borrow().view.borrow()).unwrap_err();
        assert!(err.contains("canvas 80x80"), "{err}");
    }

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = host_rc_seeded();
        let spec = import_image_panel_spec(host.clone());
        assert_eq!(spec.id.raw(), IMPORT_IMAGE_PANEL_ID);
        assert_eq!(spec.metadata.title, "Import Image");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        let nest = build_import_image_nest(&host);
        assert_eq!(nest.mode(), NestMode::Static);
    }

    /// Drive one headless frame of the component with `events`.
    fn run_component_frame(
        ctx: &egui::Context,
        host: &Rc<RefCell<ImportImageHost>>,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(360.0, 230.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Canvas,
        );
        let mut component = ImportImageComponent { host: host.clone() };
        let mut output = ctx.run_ui(raw, |ui| {
            let view = NestView::screen(ui.max_rect());
            component.ui(ui, &view, &chrome);
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
    fn component_paints_labels_and_centred_actions() {
        let ctx = egui::Context::default();
        let host = host_rc_seeded();
        let output = run_component_frame(&ctx, &host, Vec::new());
        assert!(!output.shapes.is_empty(), "the panel paints content");
        assert!(
            !text_positions(&output, "Split by pixels").is_empty(),
            "the pixels label is painted"
        );
        assert!(
            !text_positions(&output, "Split by number of regions").is_empty(),
            "the regions label is painted"
        );
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
            v.regions_text = "3x3".to_owned();
            on_regions_edited(&mut v);
        }
        let _ = run_component_frame(&ctx, &host, vec![key_event(egui::Key::Enter)]);
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
        let _ = run_component_frame(&ctx, &host, vec![key_event(egui::Key::Escape)]);
        assert_eq!(
            host.borrow_mut().take_events(),
            vec![ImportImageEvent::Cancel]
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
