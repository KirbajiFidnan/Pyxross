//! Floating "Import GIF" dock panel (id 209).
//!
//! The second step of the GIF import flow: the in-app File Manager (id 208,
//! mode [`crate::ui::dock_file_chooser_panel::FileChooserMode::ImportGif`])
//! picks a `.gif`; the App decodes it (via `io`) and opens THIS panel to choose
//! how many frames go on each sheet line. On OK the shell lays the frames onto
//! a region sheet using [`crate::core::import::gif_sheet`].
//!
//! It mirrors `dock_save_as_panel` (id 207): a float-only, fixed-size dock
//! panel holding a static nest; the in-progress form lives in the host's
//! [`ImportGifView`] cell and the panel is a pure view that only pushes
//! [`ImportGifEvent`]s for the App shell to apply. Enter = OK (when valid),
//! Esc = Cancel.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import GIF floating panel.
pub const IMPORT_GIF_PANEL_ID: u64 = 209;

/// Panel/window title.
const IMPORT_GIF_TITLE: &str = "Import GIF";

/// Fixed minimum size of the (non-resizable) floating window: the "Line length"
/// label + box, the inline error line and the OK / Cancel row.
const IMPORT_GIF_MIN_SIZE: egui::Vec2 = vec2(340.0, 150.0);

/// Default maximum total sheet cells ([`crate::core::import::gif_sheet`]'s
/// `cells`) before the import is rejected.
pub const IMPORT_GIF_DEFAULT_MAX_FRAMES: usize = 4096;

/// The in-progress GIF-import form values (dialog draft, not model data).
#[derive(Clone, Debug)]
pub struct ImportGifView {
    /// The `.gif` chosen in the File Manager.
    pub path: PathBuf,
    /// Number of frames in the decoded animation.
    pub frame_count: usize,
    /// One frame's width in pixels.
    pub frame_w: u32,
    /// One frame's height in pixels.
    pub frame_h: u32,
    /// Editable "Line length" text (frames per sheet row); starts at `"1"`.
    pub line_length_text: String,
    /// Inline validation/decode error; the panel stays open with this text.
    pub error: Option<String>,
    /// Maximum total sheet cells allowed (seeded by the shell).
    pub max_frames: usize,
}

impl Default for ImportGifView {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            frame_count: 0,
            frame_w: 0,
            frame_h: 0,
            line_length_text: "1".to_owned(),
            error: None,
            max_frames: IMPORT_GIF_DEFAULT_MAX_FRAMES,
        }
    }
}

/// An Import GIF gesture emitted by the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportGifEvent {
    /// The user confirmed with a valid line length.
    Ok { line_length: u32 },
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
    /// line length back to `"1"`, error cleared.
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
        view.error = None;
        view.max_frames = max_frames;
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

/// Validation shared between the OK button and the tests: parse a `u32` line
/// length (`>= 1`), then require the resulting sheet to fit `max_frames`.
///
/// `frame_count == 0` (nothing to lay out) is an error. Returns the parsed line
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
        .map_err(|_| "Line length must be a whole number".to_owned())?;
    if line_length == 0 {
        return Err("Line length must be at least 1".to_owned());
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

/// [`validate_line_length_text`] against a view's stored fields.
pub fn validate_line_length(view: &ImportGifView) -> Result<u32, String> {
    validate_line_length_text(
        &view.line_length_text,
        view.frame_count,
        view.frame_w,
        view.frame_h,
        view.max_frames,
    )
}

/// The Import GIF form: "Line length" label + centred box, inline error, then a
/// symmetric centred OK / Cancel row.
struct ImportGifComponent {
    host: Rc<RefCell<ImportGifHost>>,
}

impl Component for ImportGifComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        // Snapshot the form out of the host cell so the borrow is released
        // before the widgets run.
        let (mut line_length_text, error, frame_count, frame_w, frame_h, max_frames) = {
            let host = self.host.borrow();
            let v = host.view.borrow();
            (
                v.line_length_text.clone(),
                v.error.clone(),
                v.frame_count,
                v.frame_w,
                v.frame_h,
                v.max_frames,
            )
        };

        ui.label("Line length");
        ui.vertical_centered(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut line_length_text)
                    .hint_text("frames per row")
                    .desired_width(160.0),
            );
        });

        // Live validation against the text the user is typing this frame.
        let live_validation =
            validate_line_length_text(&line_length_text, frame_count, frame_w, frame_h, max_frames);
        let valid = live_validation.is_ok();

        // Inline error: a host-injected error (e.g. decode failure) wins;
        // otherwise the live validation message shows while the input is bad.
        let shown_error = error.or_else(|| live_validation.clone().err());
        if let Some(err) = &shown_error {
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        }

        // Symmetric centred action pair (mirrors save_as): OK hugs the centre
        // from the left half, Cancel from the right half.
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

        // Enter = OK (only when valid), Esc = Cancel.
        let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed =
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        // Write the edited text back into the host cell.
        let host = self.host.borrow();
        {
            let mut v = host.view.borrow_mut();
            v.line_length_text = line_length_text;
        }

        if cancel_clicked || escape_pressed {
            host.view.borrow_mut().error = None;
            host.events.borrow_mut().push(ImportGifEvent::Cancel);
            return;
        }

        if ok_clicked || enter_pressed {
            let result = {
                let v = host.view.borrow();
                validate_line_length(&v)
            };
            match result {
                Ok(line_length) => {
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

/// Panel content: the static nest holding the Import GIF form.
struct ImportGifPanelContent {
    nest: NestContent,
}

impl PanelContent for ImportGifPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the Import GIF form.
pub(crate) fn build_import_gif_nest(host: &Rc<RefCell<ImportGifHost>>) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ImportGifComponent { host: host.clone() });
    nest
}

/// Dock spec for the Import GIF floating panel (dock id 209).
///
/// **Float-only:** `can_pop_out == false` (no Float/Dock button, not dockable);
/// `resizable == false` keeps the window fixed-size and draggable only by its
/// header. A fixed [`floating_rect`](PanelSpec::floating_rect) gives the small
/// form a stable size.
pub fn import_gif_panel_spec(host: Rc<RefCell<ImportGifHost>>) -> PanelSpec {
    let floating_rect = egui::Rect::from_min_size(egui::pos2(160.0, 140.0), IMPORT_GIF_MIN_SIZE);
    PanelSpec {
        id: PanelId::new(IMPORT_GIF_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(IMPORT_GIF_TITLE, false, IMPORT_GIF_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(ImportGifPanelContent {
            nest: build_import_gif_nest(&host),
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
        host.set_error("old".to_owned());
        host.seed(PathBuf::from("/tmp/anim.gif"), 22, 8, 16, 100);
        let v = host.view.borrow();
        assert_eq!(v.path, PathBuf::from("/tmp/anim.gif"));
        assert_eq!(v.frame_count, 22);
        assert_eq!((v.frame_w, v.frame_h), (8, 16));
        assert_eq!(v.line_length_text, "1", "seed resets the line length");
        assert!(v.error.is_none(), "seed clears a stale error");
        assert_eq!(v.max_frames, 100);
    }

    #[test]
    fn default_view_is_sane() {
        let v = ImportGifView::default();
        assert_eq!(v.line_length_text, "1");
        assert_eq!(v.max_frames, IMPORT_GIF_DEFAULT_MAX_FRAMES);
        assert!(v.error.is_none());
    }

    #[test]
    fn validation_rejects_non_numeric_zero_and_over_limit() {
        // "abc" is not a number.
        assert!(validate_line_length_text("abc", 22, 8, 8, 4096).is_err());
        // Empty.
        assert!(validate_line_length_text("", 22, 8, 8, 4096).is_err());
        // Zero -> error.
        assert!(validate_line_length_text("0", 22, 8, 8, 4096).is_err());
        // Too many cells: 22 frames on a line of 4097 -> cells 4097 > 4096.
        let err = validate_line_length_text("4097", 22, 8, 8, 4096).unwrap_err();
        assert!(err.contains("exceed"), "{err}");
        // No frames at all.
        assert!(validate_line_length_text("4", 0, 8, 8, 4096).is_err());
        // Valid: 22 frames / line 4 -> cells 24 <= 4096.
        assert_eq!(validate_line_length_text("4", 22, 8, 8, 4096).unwrap(), 4);
    }

    #[test]
    fn validation_trims_and_uses_gif_sheet_cells() {
        assert_eq!(
            validate_line_length_text("  4 ", 20, 8, 8, 4096).unwrap(),
            4
        );
        // 20 / 4 -> 20 cells exactly.
        assert_eq!(validate_line_length_text("4", 20, 8, 8, 20).unwrap(), 4);
        assert!(validate_line_length_text("4", 22, 8, 8, 20).is_err());
    }

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 4, 8, 8, 4096);
        let spec = import_gif_panel_spec(host.clone());
        assert_eq!(spec.id.raw(), IMPORT_GIF_PANEL_ID);
        assert_eq!(spec.metadata.title, "Import GIF");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        let nest = build_import_gif_nest(&host);
        assert_eq!(nest.mode(), NestMode::Static);
    }

    /// Drive one headless frame of the component with `events`.
    fn run_component_frame(
        ctx: &egui::Context,
        host: &Rc<RefCell<ImportGifHost>>,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(360.0, 160.0),
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
        let mut component = ImportGifComponent { host: host.clone() };
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
    fn component_paints_labels_and_centred_actions() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 22, 8, 16, 4096);
        let output = run_component_frame(&ctx, &host, Vec::new());
        assert!(!output.shapes.is_empty(), "the panel paints content");
        assert!(
            !text_positions(&output, "Line length").is_empty(),
            "the line-length label is painted"
        );
        assert!(
            !text_positions(&output, "OK").is_empty(),
            "the OK button is painted"
        );
        assert!(
            !text_positions(&output, "Cancel").is_empty(),
            "the Cancel button is painted"
        );

        let ok_x = text_positions(&output, "OK").into_iter().next().unwrap().x;
        let cancel_x = text_positions(&output, "Cancel")
            .into_iter()
            .next()
            .unwrap()
            .x;
        assert!(ok_x < cancel_x, "OK sits left of Cancel");
        assert!(
            (ok_x - (360.0 - cancel_x)).abs() < 40.0,
            "OK and Cancel are roughly symmetric about the centre (ok={ok_x}, cancel={cancel_x})"
        );
    }

    #[test]
    fn component_paints_a_live_error_for_an_invalid_line_length() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 22, 8, 16, 4096);
        host.borrow_mut().view.borrow_mut().line_length_text = "0".to_owned();
        let output = run_component_frame(&ctx, &host, Vec::new());
        assert!(
            !text_positions(&output, "Line length must be at least 1").is_empty(),
            "the validation error is painted inline"
        );
    }

    #[test]
    fn enter_emits_ok_with_the_line_length() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 22, 8, 16, 4096);
        host.borrow_mut().view.borrow_mut().line_length_text = "4".to_owned();
        let _ = run_component_frame(&ctx, &host, vec![key_event(egui::Key::Enter)]);
        let events = host.borrow_mut().take_events();
        assert_eq!(events, vec![ImportGifEvent::Ok { line_length: 4 }]);
    }

    #[test]
    fn enter_with_an_invalid_line_length_does_not_emit() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 22, 8, 16, 4096);
        host.borrow_mut().view.borrow_mut().line_length_text = "0".to_owned();
        let _ = run_component_frame(&ctx, &host, vec![key_event(egui::Key::Enter)]);
        assert!(
            host.borrow_mut().take_events().is_empty(),
            "an invalid line length must not emit Ok"
        );
        assert!(
            host.borrow().view.borrow().error.is_some(),
            "the invalid input surfaces an inline error"
        );
    }

    #[test]
    fn escape_emits_cancel() {
        let ctx = egui::Context::default();
        let host = host_rc();
        host.borrow_mut()
            .seed(PathBuf::from("/tmp/a.gif"), 22, 8, 16, 4096);
        let _ = run_component_frame(&ctx, &host, vec![key_event(egui::Key::Escape)]);
        assert_eq!(
            host.borrow_mut().take_events(),
            vec![ImportGifEvent::Cancel]
        );
    }

    #[test]
    fn take_events_drains() {
        let mut host = ImportGifHost::new();
        host.events
            .borrow_mut()
            .push(ImportGifEvent::Ok { line_length: 2 });
        assert_eq!(host.take_events().len(), 1);
        assert!(host.take_events().is_empty(), "the cell is drained");
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
