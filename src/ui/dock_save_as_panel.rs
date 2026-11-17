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

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the save-as/export floating panel.
pub const SAVE_AS_PANEL_ID: u64 = 207;

/// Fixed minimum size of the (non-resizable) floating window. Tall enough for
/// the vertical stack (path, Browse, name, format, actions).
const SAVE_AS_MIN_SIZE: egui::Vec2 = vec2(380.0, 210.0);

/// What kind of save/export the modal is currently drafting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveAsKind {
    /// Project Save As…: directory project or single `.pyxross` archive.
    Project,
    /// Sprite-sheet export: PNG plus a sibling `.json` meta file.
    SpriteSheet,
    /// GIF animation export.
    Gif,
    /// Selection PNG export.
    SelectionPng,
}

impl SaveAsKind {
    /// Panel title (and window title for the floating panel).
    pub fn title(self) -> &'static str {
        match self {
            SaveAsKind::Project => "Save As",
            SaveAsKind::SpriteSheet => "Export Sprite Sheet",
            SaveAsKind::Gif => "Export GIF",
            SaveAsKind::SelectionPng => "Export Selection PNG",
        }
    }

    /// Default file/dir name suggested when the modal opens. The project
    /// default is the current project name (set by the App shell); the export
    /// defaults match the file names the old `save_file()` dialogs proposed.
    pub fn default_name(self) -> &'static str {
        match self {
            SaveAsKind::Project => "project",
            SaveAsKind::SpriteSheet => "sprite_sheet.png",
            SaveAsKind::Gif => "animation.gif",
            SaveAsKind::SelectionPng => "selection.png",
        }
    }
}

/// The format choices for `kind`, in display order; the first is the selected
/// one. Per D82 Save As is a mandatory single `.pyxross`, so that is the only
/// entry today — the slice keeps the row extensible for future formats.
pub fn formats_for(kind: SaveAsKind) -> &'static [&'static str] {
    const PROJECT: &[&str] = &[".pyxross"];
    match kind {
        SaveAsKind::Project => PROJECT,
        SaveAsKind::SpriteSheet | SaveAsKind::Gif | SaveAsKind::SelectionPng => &[],
    }
}

/// Pre-computed export payload. Computed ONCE when the export entry point
/// opens the modal (not on every Save press); project saves carry `None` and
/// serialize through `io` when Save is confirmed.
#[derive(Clone, Debug)]
pub enum SavePayload {
    /// PNG bytes + the sibling `.json` meta bytes.
    SpriteSheet { png: Vec<u8>, meta: Vec<u8> },
    Gif(Vec<u8>),
    SelectionPng { w: usize, h: usize, rgba: Vec<u8> },
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

/// Project-name completion: ensure a `.pyxross` suffix is present (Save As
/// always writes a single file). Names are trimmed; a name already ending in
/// `.pyxross` is left as-is.
pub fn with_pyxross_extension(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.ends_with(".pyxross") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.pyxross")
    }
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
    let final_name = if view.kind == SaveAsKind::Project {
        with_pyxross_extension(&view.name)
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
/// line, Save/Cancel.
struct SaveAsComponent {
    host: SaveAsHost,
}

impl Component for SaveAsComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        // Copy the form out of the host cell so the borrow is released before
        // the widgets run; edited values are written back at the end.
        let (kind, location, mut name, error, focus_requested, mut location_edit, mut synced) = {
            let v = self.host.view.borrow();
            (
                v.kind,
                v.location.clone(),
                v.name.clone(),
                v.error.clone(),
                v.focus_requested,
                v.location_edit.clone(),
                v.synced_location.clone(),
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

        // Destination path: a real, full-width editable box, with a label
        // above it matching the File Manager's "File name:" styling.
        ui.label("Save path:");
        ui.add(
            egui::TextEdit::singleline(&mut location_edit)
                .hint_text("destination folder")
                .desired_width(f32::INFINITY),
        );

        // Folder picker on its own line, left-aligned. Emits BrowseLocation so
        // the App shell opens the File Manager in ChooseFolder mode; when it
        // returns, the shell updates `location` and the box re-syncs.
        let browse_clicked = chrome
            .button(ui, "Browse", ButtonStyle::plain())
            .on_hover_text("Choose the destination folder")
            .clicked();

        // Document-name box (full width), focused once when the modal opens.
        ui.label("Document name:");
        let name_response = ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text(if kind == SaveAsKind::Project {
                    "project or document name"
                } else {
                    "file name"
                })
                .desired_width(f32::INFINITY),
        );
        if focus_requested {
            name_response.request_focus();
        }

        // Format row: the mandatory (sole, selected) format per D82. The row
        // iterates `formats_for` so future formats can be added without
        // restructuring.
        let formats = formats_for(kind);
        if !formats.is_empty() {
            ui.horizontal(|ui| {
                for (index, label) in formats.iter().enumerate() {
                    let _ = chrome.button(ui, label, ButtonStyle::toggled(index == 0));
                }
            });
        }

        // Inline error line (theme error color, panel stays open).
        if let Some(err) = &error {
            ui.label(egui::RichText::new(err).color(ui.visuals().error_fg_color));
        }

        // Bottom action row: a symmetric pair centred on the panel — Save hugs
        // the centre from the left half, Cancel from the right half, so the
        // spacing around the panel centre is equal.
        let mut save_clicked = false;
        let mut cancel_clicked = false;
        ui.columns(2, |columns| {
            columns[0].with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome.button(ui, "Save", ButtonStyle::plain()).clicked() {
                    save_clicked = true;
                }
            });
            columns[1].with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
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
    nest.push(SaveAsComponent { host: host.clone() });
    nest
}

/// Dock spec for the save-as/export floating panel (dock id 207).
///
/// **Float-only:** `can_pop_out == false` (no Float/Dock button, not dockable);
/// `resizable == false` keeps the window fixed-size and draggable only by its
/// header. The title follows the kind the host is drafting.
pub fn save_as_panel_spec(host: &SaveAsHost, floating_rect: egui::Rect) -> PanelSpec {
    let title = host.view.borrow().kind.title();
    PanelSpec {
        id: PanelId::new(SAVE_AS_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(title, false, SAVE_AS_MIN_SIZE)
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
    fn formats_for_project_is_pyxross_only() {
        assert_eq!(formats_for(SaveAsKind::Project), &[".pyxross"][..]);
        assert!(formats_for(SaveAsKind::SpriteSheet).is_empty());
        assert!(formats_for(SaveAsKind::Gif).is_empty());
        assert!(formats_for(SaveAsKind::SelectionPng).is_empty());
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
            (SpriteSheet, "Export Sprite Sheet", "sprite_sheet.png"),
            (Gif, "Export GIF", "animation.gif"),
            (SelectionPng, "Export Selection PNG", "selection.png"),
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
        let mut component = SaveAsComponent { host: host.clone() };
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

        // Save and Cancel form a centred pair: both sit in the middle band of
        // the panel (regression against the old Save-left / Cancel-far-right).
        let width = 420.0;
        let save_x = text_positions(&output, "Save")
            .into_iter()
            .next()
            .expect("Save is painted")
            .x;
        let cancel_x = text_positions(&output, "Cancel")
            .into_iter()
            .next()
            .expect("Cancel is painted")
            .x;
        assert!(
            save_x > width * 0.25 && save_x < width * 0.75,
            "Save sits near the panel centre (x = {save_x})"
        );
        assert!(
            cancel_x > width * 0.25 && cancel_x < width * 0.75,
            "Cancel sits near the panel centre (x = {cancel_x})"
        );
    }

    #[test]
    fn format_button_is_absent_for_export_kinds() {
        let ctx = egui::Context::default();
        let host = SaveAsHost::default();
        {
            let mut v = host.view.borrow_mut();
            v.kind = SaveAsKind::Gif;
            v.location = std::env::temp_dir();
            v.location_edit = std::env::temp_dir().display().to_string();
            v.name = "animation.gif".to_owned();
        }
        let output = run_component_frame(&ctx, &host, Vec::new());
        assert!(
            text_positions(&output, ".pyxross").is_empty(),
            "export kinds show no .pyxross format button"
        );
    }
}
