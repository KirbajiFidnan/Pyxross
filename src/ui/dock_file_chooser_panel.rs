//! Floating "File Manager" dock panel (id 208).
//!
//! The in-app file chooser REPLACES every remaining native file dialog: on
//! this machine the rfd 0.15.4 portal (ashpd) backend hangs before any dialog
//! opens (`save_file`, `pick_file` and `pick_folder` are all untrustworthy).
//! Save As… already lives in [`crate::ui::dock_save_as_panel`]; this panel
//! covers project open, PNG import, and the save-as "Change…" folder pick.
//!
//! It mirrors `dock_create_document_panel` (id 205): a float-only dock panel
//! with a static nest; the in-progress state lives in the host's
//! [`FileChooserView`] cell and the panel only pushes [`FileChooserEvent`]s
//! for the App shell to apply. Directories are read on navigation / the
//! new-folder action — never per row, never per frame.
//!
//! Layout (mirrors a classic file dialog):
//! - toolbar: back / forward / Up / new-folder, the editable current path and
//!   an ancestor-jump dropdown;
//! - a darker listing well that grows/shrinks with the panel and scrolls,
//!   with zebra-striped rows styled like the Layers panel (never buttons);
//!   directories first, then files; single click selects, double click
//!   navigates into a directory or confirms a file;
//! - footer: "File name" field + quick-pick dropdown, and OK / Cancel.
//!
//! Filters per mode: `OpenProject` lists directories + `*.pyxross` +
//! `manifest.json`; `ImportPng` lists directories + `*.png`; `ImportGif`
//! lists directories + `*.gif`; `ChooseFolder` lists directories only. Every
//! category is case-insensitive.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the file chooser floating panel.
pub const FILE_CHOOSER_PANEL_ID: u64 = 208;

/// Window title. The floating header paints this plus its Close action.
const FILE_CHOOSER_TITLE: &str = "File Manager";

/// Fixed minimum size of the floating window (resizable).
const FILE_CHOOSER_MIN_SIZE: egui::Vec2 = vec2(620.0, 420.0);

/// Height of one listing row (matches the Layers panel convention).
const ROW_HEIGHT: f32 = 28.0;

/// What the chooser is picking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileChooserMode {
    /// Pick a project: a directory (native form), a `.pyxross` archive, or a
    /// `manifest.json` inside a directory project.
    OpenProject,
    /// Pick a `.png` to import.
    ImportPng,
    /// Pick a `.gif` to import as an animation sheet.
    ImportGif,
    /// Pick a `.pyxel` Pyxel Edit project to import.
    ImportPyxel,
    /// Pick an `.ase` / `.aseprite` file to import.
    ImportAseprite,
    /// Pick an image (PNG/JPG/BMP/WebP/TGA/GIF) to convert to pixel art.
    ImportImage2Pixel,
    /// Pick a destination folder (save-as "Change…").
    ChooseFolder,
}

impl FileChooserMode {
    /// Descriptive label for this mode (kept for callers/tests; the floating
    /// window itself is titled [`FILE_CHOOSER_TITLE`]).
    pub fn title(self) -> &'static str {
        match self {
            FileChooserMode::OpenProject => "Open Project",
            FileChooserMode::ImportPng => "Import PNG",
            FileChooserMode::ImportGif => "Import GIF",
            FileChooserMode::ImportPyxel => "Import Pyxel",
            FileChooserMode::ImportAseprite => "Import Aseprite",
            FileChooserMode::ImportImage2Pixel => "Import Image 2 Pixel",
            FileChooserMode::ChooseFolder => "Choose Folder",
        }
    }

    /// Label of the mode's confirm action (shown as the OK button's tooltip).
    pub fn confirm_label(self) -> &'static str {
        match self {
            FileChooserMode::OpenProject => "Open",
            FileChooserMode::ImportPng => "Import",
            FileChooserMode::ImportGif => "Import",
            FileChooserMode::ImportPyxel => "Import",
            FileChooserMode::ImportAseprite => "Import",
            FileChooserMode::ImportImage2Pixel => "Import",
            FileChooserMode::ChooseFolder => "Choose",
        }
    }
}

/// One row of the directory listing.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// The in-progress chooser state (dialog draft, not model data).
#[derive(Clone, Debug)]
pub struct FileChooserView {
    pub mode: FileChooserMode,
    /// Directory currently listed.
    pub cwd: PathBuf,
    /// Filtered + sorted listing of `cwd` (rebuilt on navigation only).
    pub entries: Vec<Entry>,
    /// Currently highlighted entry (single click), `None` = nothing selected.
    pub selected: Option<PathBuf>,
    /// Editable path row; Enter / the ancestor dropdown navigate to it.
    pub path_edit: String,
    /// Bottom "File name" text: the selected entry's name, or a typed name.
    pub name_edit: String,
    /// Navigation history, oldest first; `history[history_pos] == cwd`.
    pub history: Vec<PathBuf>,
    /// Index of `cwd` within [`Self::history`].
    pub history_pos: usize,
    /// Inline error (bad directory, unreadable listing, …).
    pub error: Option<String>,
    /// Set by the opener so the panel focuses the path row once.
    pub focus_requested: bool,
}

impl Default for FileChooserView {
    fn default() -> Self {
        Self {
            mode: FileChooserMode::OpenProject,
            cwd: PathBuf::new(),
            entries: Vec::new(),
            selected: None,
            path_edit: String::new(),
            name_edit: String::new(),
            history: Vec::new(),
            history_pos: 0,
            error: None,
            focus_requested: false,
        }
    }
}

/// A chooser gesture emitted by the panel.
///
/// `Chosen` carries the RAW selected path — the App shell applies the D80
/// dir↔file resolution (`load_project` handles both shapes, including the
/// `manifest.json`→parent case). The panel never touches the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileChooserEvent {
    Chosen(PathBuf),
    Cancelled,
}

/// Shared cells for the file chooser panel.
#[derive(Clone, Default)]
pub struct FileChooserHost {
    pub view: Rc<RefCell<FileChooserView>>,
    pub events: Rc<RefCell<Vec<FileChooserEvent>>>,
}

/// Whether a path is listable for `mode`. Directories are shown in every
/// mode (they are navigable); file filters are case-insensitive.
pub fn matches_filter(mode: FileChooserMode, path: &Path) -> bool {
    if path.is_dir() {
        return true;
    }
    match mode {
        FileChooserMode::ChooseFolder => false,
        FileChooserMode::OpenProject => {
            path.file_name().is_some_and(|name| name == "manifest.json")
                || path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pyxross"))
        }
        FileChooserMode::ImportPng => path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png")),
        FileChooserMode::ImportGif => path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("gif")),
        FileChooserMode::ImportPyxel => path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pyxel")),
        FileChooserMode::ImportAseprite => path.extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("ase") || ext.eq_ignore_ascii_case("aseprite")
        }),
        FileChooserMode::ImportImage2Pixel => path.extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("png")
                || ext.eq_ignore_ascii_case("jpg")
                || ext.eq_ignore_ascii_case("jpeg")
                || ext.eq_ignore_ascii_case("bmp")
                || ext.eq_ignore_ascii_case("webp")
                || ext.eq_ignore_ascii_case("tga")
                || ext.eq_ignore_ascii_case("gif")
        }),
    }
}

/// Sorts a listing: directories first, then names case-insensitively.
pub fn sort_entries(mut entries: Vec<Entry>) -> Vec<Entry> {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

/// Lists `dir` filtered for `mode` and sorted. A missing/unreadable directory
/// is a `Err` with a user-facing message.
pub fn read_dir_entries(mode: FileChooserMode, dir: &Path) -> Result<Vec<Entry>, String> {
    let entries =
        std::fs::read_dir(dir).map_err(|err| format!("cannot read {}: {err}", dir.display()))?;
    let mut out = Vec::new();
    for item in entries.flatten() {
        let path = item.path();
        if !matches_filter(mode, &path) {
            continue;
        }
        out.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            is_dir: path.is_dir(),
            path,
        });
    }
    Ok(sort_entries(out))
}

/// The parent directory of `path` (the chooser's "Up" target).
pub fn parent_dir(path: &Path) -> Option<PathBuf> {
    path.parent().map(Path::to_path_buf)
}

/// The file name of `path` as a display string (empty when there is none).
pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Every non-empty ancestor of `path`, shallowest first (a breadcrumb root →
/// current). Used by the path dropdown.
pub fn ancestors(path: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = path
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect();
    out.reverse();
    out
}

/// Navigates to `dir`, reloading the listing and updating the history.
pub fn navigate(view: &mut FileChooserView, dir: PathBuf) -> Result<(), String> {
    let outcome = set_directory(view, dir.clone());
    if view.cwd == dir {
        push_history(view, dir);
    }
    outcome
}

/// Sets the current directory without touching the history (back/forward use
/// this). Clears the selection; a non-directory or unreadable target sets
/// `error`.
fn set_directory(view: &mut FileChooserView, dir: PathBuf) -> Result<(), String> {
    if !dir.is_dir() {
        let message = format!("'{}' is not a directory", dir.display());
        view.error = Some(message.clone());
        return Err(message);
    }
    view.cwd = dir.clone();
    view.path_edit = dir.display().to_string();
    view.selected = None;
    view.name_edit.clear();
    match read_dir_entries(view.mode, &dir) {
        Ok(entries) => {
            view.entries = entries;
            view.error = None;
            Ok(())
        }
        Err(message) => {
            view.entries = Vec::new();
            view.error = Some(message.clone());
            Err(message)
        }
    }
}

/// Appends `dir` to the history, truncating any forward branch.
fn push_history(view: &mut FileChooserView, dir: PathBuf) {
    if view.history.get(view.history_pos) == Some(&dir) {
        return;
    }
    view.history.truncate(view.history_pos + 1);
    view.history.push(dir);
    view.history_pos = view.history.len().saturating_sub(1);
}

/// Navigates one step back in the history (a no-op at the oldest entry).
pub fn go_back(view: &mut FileChooserView) -> Result<(), String> {
    let Some(target) = view
        .history_pos
        .checked_sub(1)
        .and_then(|index| view.history.get(index))
        .cloned()
    else {
        return Ok(());
    };
    set_directory(view, target)?;
    view.history_pos = view.history_pos.saturating_sub(1);
    Ok(())
}

/// Navigates one step forward in the history (a no-op at the newest entry).
pub fn go_forward(view: &mut FileChooserView) -> Result<(), String> {
    let next = view.history_pos + 1;
    let Some(target) = view.history.get(next).cloned() else {
        return Ok(());
    };
    set_directory(view, target)?;
    view.history_pos = next;
    Ok(())
}

/// Navigates to the parent directory ("Up").
pub fn go_up(view: &mut FileChooserView) -> Result<(), String> {
    let Some(parent) = parent_dir(&view.cwd) else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent == view.cwd {
        return Ok(());
    }
    navigate(view, parent)
}

/// Navigates to the (editable) path row's target.
pub fn go_to_edit(view: &mut FileChooserView) -> Result<(), String> {
    let target = PathBuf::from(view.path_edit.trim());
    navigate(view, target)
}

/// Creates a uniquely named folder in `cwd`, refreshes the listing and selects
/// it. Filesystem work is done here (the panel already reads directories).
pub fn create_new_folder(view: &mut FileChooserView) -> Result<PathBuf, String> {
    let base = view.cwd.clone();
    if !base.is_dir() {
        return Err("cannot create a folder in the current location".to_owned());
    }
    let target = unique_child_dir(&base);
    std::fs::create_dir(&target)
        .map_err(|err| format!("cannot create {}: {err}", target.display()))?;
    set_directory(view, base)?;
    view.selected = Some(target.clone());
    view.name_edit = file_name_of(&target);
    Ok(target)
}

/// "New Folder", "New Folder (2)", … — the first name that does not exist.
fn unique_child_dir(base: &Path) -> PathBuf {
    let mut candidate = base.join("New Folder");
    let mut counter = 2u32;
    while candidate.exists() && counter < 10_000 {
        candidate = base.join(format!("New Folder ({counter})"));
        counter += 1;
    }
    candidate
}

/// The confirm action: the selected path, or a typed "File name" that
/// resolves inside `cwd`. The result is always validated against the mode
/// filter, so `Chosen` never carries a path the shell would not accept.
pub fn confirm_selection(view: &FileChooserView) -> Result<PathBuf, String> {
    if let Some(path) = &view.selected {
        if !matches_filter(view.mode, path) {
            return Err(format!("'{}' is not valid for this dialog", path.display()));
        }
        return Ok(path.clone());
    }
    let typed = view.name_edit.trim();
    if !typed.is_empty() {
        let candidate = view.cwd.join(typed);
        if !candidate.exists() && !candidate.is_dir() {
            return Err(format!("'{}' does not exist", candidate.display()));
        }
        if !matches_filter(view.mode, &candidate) {
            return Err(format!("'{}' is not valid for this dialog", candidate.display()));
        }
        return Ok(candidate);
    }
    if view.mode == FileChooserMode::ChooseFolder && view.cwd.is_dir() {
        return Ok(view.cwd.clone());
    }
    Err("select an entry first".to_owned())
}

/// Gestures collected while painting one frame, applied after the view borrow
/// is released.
#[derive(Default)]
struct ChooserActions {
    select: Option<PathBuf>,
    navigate: Option<PathBuf>,
    new_folder: bool,
    back: bool,
    forward: bool,
    up: bool,
    confirm: bool,
    cancel: bool,
}

impl ChooserActions {
    /// True when this frame performed a navigation (so Enter must not also
    /// confirm).
    fn is_navigation(&self) -> bool {
        self.navigate.is_some() || self.new_folder || self.back || self.forward || self.up
    }
}

/// A sidebar shortcut entry (standard OS folder or native OS/DE bookmark).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaceItem {
    pub label: String,
    pub path: PathBuf,
    pub is_bookmark: bool,
}

enum PreviewState {
    Empty,
    Image {
        texture: egui::TextureHandle,
        width: u32,
        height: u32,
        info: String,
    },
    Unavailable(String),
}

/// The chooser form: pinned toolbar, left Places/Bookmarks sidebar, growing
/// listing well, right Preview pane, and pinned footer.
struct FileChooserComponent {
    host: FileChooserHost,
    places: Vec<PlaceItem>,
    preview_path: Option<PathBuf>,
    preview_state: PreviewState,
}

impl FileChooserComponent {
    fn new(host: FileChooserHost) -> Self {
        Self {
            host,
            places: discover_places(),
            preview_path: None,
            preview_state: PreviewState::Empty,
        }
    }

    fn sync_preview(&mut self, ctx: &egui::Context, selected: Option<&Path>) {
        let target = selected.map(Path::to_path_buf);
        if target == self.preview_path {
            return;
        }
        self.preview_path = target.clone();
        let Some(path) = target else {
            self.preview_state = PreviewState::Empty;
            return;
        };
        match load_file_preview(&path) {
            Some(Ok((w, h, rgba, info))) => {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [w as usize, h as usize],
                    &rgba,
                );
                let texture = ctx.load_texture(
                    "file_chooser_preview",
                    image,
                    egui::TextureOptions::NEAREST,
                );
                self.preview_state = PreviewState::Image {
                    texture,
                    width: w,
                    height: h,
                    info,
                };
            }
            Some(Err(reason)) => {
                self.preview_state = PreviewState::Unavailable(reason);
            }
            None => {
                self.preview_state = PreviewState::Empty;
            }
        }
    }
}

impl Component for FileChooserComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        // Copy the draft out of the host cell so the borrow is released
        // before the widgets run; state changes are written back at the end.
        let (
            mode,
            cwd,
            entries,
            selected,
            mut path_edit,
            mut name_edit,
            history_pos,
            history_len,
            error,
            focus_requested,
        ) = {
            let v = self.host.view.borrow();
            (
                v.mode,
                v.cwd.clone(),
                v.entries.clone(),
                v.selected.clone(),
                v.path_edit.clone(),
                v.name_edit.clone(),
                v.history_pos,
                v.history.len(),
                v.error.clone(),
                v.focus_requested,
            )
        };
        let mut actions = ChooserActions::default();

        // Keyboard: Enter confirms, Esc cancels (kept from the old panel).
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        // Toolbar (pinned to the top).
        egui::Panel::top(ui.id().with("chooser-toolbar"))
            .frame(egui::Frame::NONE)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                toolbar_ui(
                    ui,
                    chrome,
                    &cwd,
                    history_pos,
                    history_len,
                    &mut path_edit,
                    focus_requested,
                    enter,
                    &mut actions,
                );
            });

        // Footer (pinned to the bottom).
        egui::Panel::bottom(ui.id().with("chooser-footer"))
            .frame(egui::Frame::NONE)
            .resizable(false)
            .show_separator_line(false)
            .show(ui, |ui| {
                footer_ui(
                    ui,
                    chrome,
                    mode,
                    &entries,
                    selected.as_deref(),
                    &mut name_edit,
                    error.as_deref(),
                    &mut actions,
                );
            });

        // Left sidebar: OS standard folders + native OS/DE bookmarks.
        if !self.places.is_empty() && ui.available_width() >= 420.0 {
            egui::Panel::left(ui.id().with("chooser-places"))
                .exact_size(134.0)
                .resizable(false)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                    left: 0,
                    right: 6,
                    top: 4,
                    bottom: 4,
                }))
                .show_separator_line(true)
                .show(ui, |ui| {
                    places_ui(ui, chrome, &self.places, &cwd, &mut actions);
                });
        }

        // Right pane: live preview of selected file (images, GIF, .ase, .pyxel, .pyxross).
        if mode != FileChooserMode::ChooseFolder && ui.available_width() >= 340.0 {
            self.sync_preview(ui.ctx(), selected.as_deref());
            egui::Panel::right(ui.id().with("chooser-preview"))
                .exact_size(176.0)
                .resizable(false)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                    left: 6,
                    right: 0,
                    top: 4,
                    bottom: 4,
                }))
                .show_separator_line(true)
                .show(ui, |ui| {
                    preview_ui(
                        ui,
                        chrome,
                        selected.as_deref(),
                        &self.preview_state,
                    );
                });
        }

        // The listing well grows/shrinks with the panel and scrolls.
        list_ui(ui, chrome, &entries, selected.as_deref(), &mut actions);

        // A navigation this frame consumes Enter (the path field / a folder
        // double-click must not also confirm).
        if escape {
            actions.cancel = true;
        }
        if enter && !actions.is_navigation() && !actions.confirm && actions.select.is_none() {
            actions.confirm = true;
        }

        // Write the draft back.
        {
            let mut view = self.host.view.borrow_mut();
            let name_edited = view.name_edit != name_edit;
            view.path_edit = path_edit.clone();
            view.name_edit = name_edit.clone();
            view.focus_requested = false;
            if let Some(path) = actions.select.clone() {
                view.selected = Some(path.clone());
                view.name_edit = file_name_of(&path);
            } else if name_edited {
                view.selected = None;
            }
        }

        // Apply navigation (at most one branch runs).
        if actions.new_folder {
            let mut view = self.host.view.borrow_mut();
            if let Err(message) = create_new_folder(&mut view) {
                view.error = Some(message);
            }
        }
        if let Some(dir) = actions.navigate.clone() {
            let mut view = self.host.view.borrow_mut();
            if let Err(message) = navigate(&mut view, dir) {
                view.error = Some(message);
            }
        } else if actions.back {
            let mut view = self.host.view.borrow_mut();
            if let Err(message) = go_back(&mut view) {
                view.error = Some(message);
            }
        } else if actions.forward {
            let mut view = self.host.view.borrow_mut();
            if let Err(message) = go_forward(&mut view) {
                view.error = Some(message);
            }
        } else if actions.up {
            let mut view = self.host.view.borrow_mut();
            if let Err(message) = go_up(&mut view) {
                view.error = Some(message);
            }
        }

        // Confirm / cancel.
        if actions.confirm {
            let mut view = self.host.view.borrow_mut();
            match confirm_selection(&view) {
                Ok(path) => {
                    view.error = None;
                    drop(view);
                    self.host
                        .events
                        .borrow_mut()
                        .push(FileChooserEvent::Chosen(path));
                }
                Err(message) => view.error = Some(message),
            }
        } else if actions.cancel {
            self.host
                .events
                .borrow_mut()
                .push(FileChooserEvent::Cancelled);
        }
    }
}

/// The pinned toolbar: back / forward / Up / new-folder, the editable path and
/// an ancestor-jump dropdown.
fn toolbar_ui(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    cwd: &Path,
    history_pos: usize,
    history_len: usize,
    path_edit: &mut String,
    focus_requested: bool,
    enter: bool,
    actions: &mut ChooserActions,
) {
    ui.horizontal(|ui| {
        let back_style = ButtonStyle {
            enabled: history_pos > 0,
            ..ButtonStyle::plain()
        };
        if chrome
            .button(ui, "◀", back_style)
            .on_hover_text("Back")
            .clicked()
        {
            actions.back = true;
        }
        let forward_style = ButtonStyle {
            enabled: history_pos + 1 < history_len,
            ..ButtonStyle::plain()
        };
        if chrome
            .button(ui, "▶", forward_style)
            .on_hover_text("Forward")
            .clicked()
        {
            actions.forward = true;
        }
        let up_target = parent_dir(cwd).filter(|parent| !parent.as_os_str().is_empty());
        let up_style = ButtonStyle {
            enabled: up_target.is_some(),
            ..ButtonStyle::plain()
        };
        if chrome
            .button(ui, "Up", up_style)
            .on_hover_text("Go to the parent directory")
            .clicked()
        {
            actions.up = true;
        }
        if chrome
            .button(ui, "+", ButtonStyle::plain())
            .on_hover_text("New folder")
            .clicked()
        {
            actions.new_folder = true;
        }

        let path_response = ui.add(
            egui::TextEdit::singleline(path_edit)
                .hint_text("current folder")
                .desired_width((ui.available_width() - 34.0).max(60.0)),
        );
        if focus_requested {
            path_response.request_focus();
        }
        // Enter inside the path field navigates to the typed folder.
        if path_response.lost_focus() && enter {
            actions.navigate = Some(PathBuf::from(path_edit.trim()));
        }

        // Ancestor jump: a dropdown of the current path's components.
        egui::ComboBox::from_id_salt("chooser-path-jump")
            .selected_text("▾")
            .width(220.0)
            .show_ui(ui, |ui| {
                for ancestor in ancestors(cwd) {
                    let label = ancestor.display().to_string();
                    if ui.selectable_label(false, label).clicked() {
                        actions.navigate = Some(ancestor.clone());
                    }
                }
            });
    });
}

/// The pinned footer: "File name" + quick-pick, inline error and OK / Cancel.
fn footer_ui(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    mode: FileChooserMode,
    entries: &[Entry],
    selected: Option<&Path>,
    name_edit: &mut String,
    error: Option<&str>,
    actions: &mut ChooserActions,
) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.label("File name:");
        ui.add(
            egui::TextEdit::singleline(name_edit)
                .hint_text(if mode == FileChooserMode::ChooseFolder {
                    "current folder (or enter subfolder)"
                } else {
                    "selected file"
                })
                .desired_width((ui.available_width() - 40.0).max(60.0)),
        );
        egui::ComboBox::from_id_salt("chooser-name-pick")
            .selected_text("▾")
            .width(220.0)
            .show_ui(ui, |ui| {
                if entries.is_empty() {
                    ui.weak("(empty)");
                }
                for entry in entries {
                    let is_selected = selected == Some(entry.path.as_path());
                    if ui.selectable_label(is_selected, &entry.name).clicked() {
                        actions.select = Some(entry.path.clone());
                    }
                }
            });
    });

    if let Some(message) = error {
        ui.label(egui::RichText::new(message).color(ui.visuals().error_fg_color));
    }

    ui.horizontal(|ui| {
        if chrome
            .button(ui, "OK", ButtonStyle::plain())
            .on_hover_text(mode.confirm_label())
            .clicked()
        {
            actions.confirm = true;
        }
        if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
            actions.cancel = true;
        }
    });
}

/// The darker, zebra-striped listing well. Takes the available height and
/// scrolls. Directories are listed first (see [`sort_entries`]).
fn list_ui(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    entries: &[Entry],
    selected: Option<&Path>,
    actions: &mut ChooserActions,
) {
    let list_rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(list_rect, 2.0, chrome.colors.list_bg32());

    egui::ScrollArea::vertical()
        .id_salt(ui.id().with("chooser-list"))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if entries.is_empty() {
                ui.add_space(4.0);
                ui.weak("(no matching entries)");
                return;
            }
            for (index, entry) in entries.iter().enumerate() {
                row_ui(ui, chrome, entry, index, selected, actions);
            }
        });
}

/// One listing row: a full-width Layers-style band (selection / hover / zebra
/// fill) with a left-aligned, click-sensing name.
fn row_ui(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    entry: &Entry,
    index: usize,
    selected: Option<&Path>,
    actions: &mut ChooserActions,
) {
    let is_selected = selected == Some(entry.path.as_path());
    let row_rect = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), ROW_HEIGHT),
    );
    let hovered = !is_selected && ui.rect_contains_pointer(row_rect);
    let background = if is_selected {
        ui.visuals().selection.bg_fill
    } else if hovered {
        ui.visuals().widgets.hovered.weak_bg_fill
    } else if index % 2 == 0 {
        chrome.colors.list_stripe_bg32()
    } else {
        chrome.colors.list_bg32()
    };

    egui::Frame::NONE
        .fill(background)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                // Left-aligned name: a left-to-right layout starts the text at
                // the row's normal padding (NOT centered, unlike `add_sized`).
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), ROW_HEIGHT - 6.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(&entry.name)
                                .truncate()
                                .selectable(false),
                        );
                    },
                );
            });
        });

    // The whole row is the click target, so a click anywhere on it selects
    // (single) or navigates/confirms (double).
    let row_response = ui.interact(
        row_rect,
        ui.id().with(("chooser-row", index)),
        egui::Sense::click(),
    );
    if row_response.clicked() {
        actions.select = Some(entry.path.clone());
    }
    if row_response.double_clicked() {
        if entry.is_dir {
            actions.navigate = Some(entry.path.clone());
        } else {
            actions.select = Some(entry.path.clone());
            actions.confirm = true;
        }
    }
}

/// Panel content: the static nest holding the file chooser form.
struct FileChooserPanelContent {
    nest: NestContent,
}

impl PanelContent for FileChooserPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the file chooser form.
pub(crate) fn build_file_chooser_nest(host: &FileChooserHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(FileChooserComponent::new(host.clone()));
    nest
}

fn places_ui(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    places: &[PlaceItem],
    cwd: &Path,
    actions: &mut ChooserActions,
) {
    egui::ScrollArea::vertical()
        .id_salt("chooser-places-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut showed_bookmarks_header = false;
            ui.weak("Places");
            ui.add_space(2.0);
            for item in places {
                if item.is_bookmark && !showed_bookmarks_header {
                    showed_bookmarks_header = true;
                    ui.add_space(6.0);
                    ui.weak("Bookmarks");
                    ui.add_space(2.0);
                }
                let active = cwd == item.path;
                let row_w = ui.available_width();
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(row_w, 22.0), egui::Sense::click());
                if active {
                    ui.painter()
                        .rect_filled(rect, 2.0, chrome.colors.selection_bg_fill32());
                } else if resp.hovered() {
                    ui.painter()
                        .rect_filled(rect, 2.0, chrome.colors.list_stripe_bg32());
                }
                let text_rect = rect.shrink2(vec2(6.0, 0.0));
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(text_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                child.add(
                    egui::Label::new(&item.label)
                        .truncate()
                        .selectable(false),
                );
                if resp.clicked() {
                    actions.navigate = Some(item.path.clone());
                }
            }
        });
}

fn preview_ui(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    selected: Option<&Path>,
    state: &PreviewState,
) {
    ui.weak("Preview");
    ui.add_space(4.0);
    let avail = ui.available_size();
    let well_h = (avail.y - 46.0).max(96.0);
    let well_rect = ui.allocate_space(vec2(avail.x, well_h)).1;
    ui.painter()
        .rect_filled(well_rect, 2.0, chrome.colors.list_bg32());
    ui.painter().rect_stroke(
        well_rect,
        2.0,
        egui::Stroke::new(1.0, chrome.colors.panel_border32()),
        egui::StrokeKind::Inside,
    );

    match state {
        PreviewState::Image {
            texture,
            width,
            height,
            info,
        } => {
            let pad = 6.0;
            let aw = (well_rect.width() - pad * 2.0).max(1.0);
            let ah = (well_rect.height() - pad * 2.0).max(1.0);
            let tw = (*width).max(1) as f32;
            let th = (*height).max(1) as f32;
            let scale = (aw / tw).min(ah / th);
            let draw_rect =
                egui::Rect::from_center_size(well_rect.center(), vec2(tw * scale, th * scale));
            ui.painter().image(
                texture.id(),
                draw_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            ui.add_space(4.0);
            if let Some(p) = selected {
                ui.add(egui::Label::new(file_name_of(p)).truncate());
            }
            ui.weak(info);
        }
        PreviewState::Unavailable(msg) => {
            ui.painter().text(
                well_rect.center(),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }
        PreviewState::Empty => {
            ui.painter().text(
                well_rect.center(),
                egui::Align2::CENTER_CENTER,
                "No preview",
                egui::FontId::proportional(11.0),
                ui.visuals().weak_text_color(),
            );
        }
    }
}

/// Discovers standard OS user folders and native OS/DE bookmarks (GTK3/GTK2
/// and KDE/Dolphin XBEL) without failing on any OS or desktop environment.
pub fn discover_places() -> Vec<PlaceItem> {
    let mut out: Vec<PlaceItem> = Vec::new();
    let mut push_unique = |label: String, path: PathBuf, is_bookmark: bool| {
        if !path.is_dir() {
            return;
        }
        if out.iter().any(|existing| existing.path == path) {
            return;
        }
        out.push(PlaceItem {
            label,
            path,
            is_bookmark,
        });
    };

    if let Some(p) = dirs::home_dir() {
        push_unique("Home".to_owned(), p, false);
    }
    if let Some(p) = dirs::desktop_dir() {
        push_unique("Desktop".to_owned(), p, false);
    }
    if let Some(p) = dirs::document_dir() {
        push_unique("Documents".to_owned(), p, false);
    }
    if let Some(p) = dirs::picture_dir() {
        push_unique("Pictures".to_owned(), p, false);
    }
    if let Some(p) = dirs::download_dir() {
        push_unique("Downloads".to_owned(), p, false);
    }
    #[cfg(windows)]
    {
        for letter in b'A'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", letter as char));
            if root.is_dir() {
                push_unique(format!("{}:", letter as char), root, false);
            }
        }
    }
    #[cfg(not(windows))]
    {
        push_unique("Root (/)".to_owned(), PathBuf::from("/"), false);
    }

    // Native GTK3 / GTK2 bookmarks (GNOME, XFCE, Cinnamon, MATE, Thunar, Nemo, Nautilus).
    if let Some(cfg) = dirs::config_dir() {
        for (label, path) in parse_gtk_bookmarks(&cfg.join("gtk-3.0").join("bookmarks")) {
            push_unique(label, path, true);
        }
    }
    if let Some(home) = dirs::home_dir() {
        for (label, path) in parse_gtk_bookmarks(&home.join(".gtk-bookmarks")) {
            push_unique(label, path, true);
        }
    }
    // Native KDE Plasma / Dolphin bookmarks (XBEL).
    if let Some(data) = dirs::data_local_dir() {
        for (label, path) in parse_kde_xbel_bookmarks(&data.join("user-places.xbel")) {
            push_unique(label, path, true);
        }
    }

    out
}

fn percent_decode_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(val) = u8::from_str_radix(hex, 16) {
                decoded.push(val);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(decoded).ok()?;
    #[cfg(windows)]
    let s = s.strip_prefix('/').unwrap_or(&s).to_owned();
    Some(PathBuf::from(s))
}

fn parse_gtk_bookmarks(path: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (uri, custom_label) = match trimmed.split_once(' ') {
            Some((u, l)) => (u, Some(l.trim())),
            None => (trimmed, None),
        };
        let Some(p) = percent_decode_file_uri(uri) else {
            continue;
        };
        let label = custom_label
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| file_name_of(&p));
        out.push((label, p));
    }
    out
}

fn parse_kde_xbel_bookmarks(path: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<bookmark ") {
        rest = &rest[start..];
        let Some(end) = rest.find("</bookmark>") else {
            break;
        };
        let block = &rest[..end];
        rest = &rest[end + "</bookmark>".len()..];
        if block.contains("<IsHidden>true</IsHidden>") {
            continue;
        }
        let Some(href_pos) = block.find("href=\"") else {
            continue;
        };
        let href_rest = &block[href_pos + 6..];
        let Some(href_end) = href_rest.find('"') else {
            continue;
        };
        let uri = &href_rest[..href_end];
        let Some(p) = percent_decode_file_uri(uri) else {
            continue;
        };
        let label = block
            .find("<title>")
            .and_then(|ts| {
                let t = &block[ts + 7..];
                t.find("</title>").map(|te| t[..te].trim().to_owned())
            })
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| file_name_of(&p));
        out.push((label, p));
    }
    out
}

fn downscale_preview_if_needed(w: u32, h: u32, rgba: Vec<u8>) -> (u32, u32, Vec<u8>) {
    const MAX_PREVIEW_SIDE: u32 = 256;
    if w <= MAX_PREVIEW_SIDE && h <= MAX_PREVIEW_SIDE {
        return (w, h, rgba);
    }
    let scale = (MAX_PREVIEW_SIDE as f32 / w.max(h) as f32).min(1.0);
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        let sy = ((y as u64 * h as u64) / nh as u64).min(h as u64 - 1) as usize;
        for x in 0..nw {
            let sx = ((x as u64 * w as u64) / nw as u64).min(w as u64 - 1) as usize;
            let si = (sy * w as usize + sx) * 4;
            let di = (y as usize * nw as usize + x as usize) * 4;
            out[di..di + 4].copy_from_slice(&rgba[si..si + 4]);
        }
    }
    (nw, nh, out)
}

/// Decodes a preview thumbnail for any Pyxross-supported document/image:
/// `.pyxross` (archive or folder/`manifest.json`), `.pyxel`, `.ase`/`.aseprite`
/// (first frame), `.gif` (first frame), and raster images (`.png`/`.jpg`/`.bmp`/`.webp`/`.tga`).
fn load_file_preview(path: &Path) -> Option<Result<(u32, u32, Vec<u8>, String), String>> {
    let is_dir_project = path.is_dir() && path.join("manifest.json").is_file();
    if path.is_dir() && !is_dir_project {
        return None;
    }
    let is_manifest = path.file_name().is_some_and(|n| n == "manifest.json");
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    if is_dir_project || is_manifest || ext == "pyxross" {
        let target = if is_manifest {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        return Some(match crate::io::load_document(target) {
            Ok(doc) => {
                let w = doc.canvas_width.max(1);
                let h = doc.canvas_height.max(1);
                let mut session = crate::ui::project::ProjectSession::new(
                    crate::ui::project::ProjectId::new(0),
                    "preview",
                    w as usize,
                    h as usize,
                );
                session.load_document(&doc);
                let comp = session.layers.composite_layers(&session.tile_palette);
                let (pw, ph, rgba) =
                    downscale_preview_if_needed(w, h, comp.as_bytes().to_vec());
                Ok((pw, ph, rgba, format!("{w}x{h} • Pyxross")))
            }
            Err(_) => Err("Preview error".to_owned()),
        });
    }

    if ext == "pyxel" {
        return Some(match std::fs::read(path) {
            Ok(bytes) => match crate::core::pyxel::import_pyxel_bytes(&bytes, "preview") {
                Ok(imp) => {
                    let comp = imp.layers.composite_layers(&imp.tile_palette);
                    let w = imp.width as u32;
                    let h = imp.height as u32;
                    let (pw, ph, rgba) =
                        downscale_preview_if_needed(w, h, comp.as_bytes().to_vec());
                    Ok((pw, ph, rgba, format!("{w}x{h} • Pyxel")))
                }
                Err(_) => Err("Preview error".to_owned()),
            },
            Err(_) => Err("Unreadable".to_owned()),
        });
    }

    if ext == "ase" || ext == "aseprite" {
        return Some(match std::fs::read(path) {
            Ok(bytes) => match crate::core::aseprite::parse_aseprite_bytes(&bytes) {
                Ok(parsed) => {
                    let frame_count = parsed.frames.len();
                    match crate::core::aseprite::build_aseprite_import(
                        &parsed,
                        "preview",
                        crate::core::aseprite::AsepriteLayoutMode::SingleRow,
                    ) {
                        Ok(imp) => {
                            let comp = imp
                                .layers
                                .composite_layers(&crate::core::tilemap::TilePalette::default());
                            let w = imp.frame_w.min(imp.width as u32).max(1);
                            let h = imp.frame_h.min(imp.height as u32).max(1);
                            let full_w = imp.width;
                            let bytes = comp.as_bytes();
                            let mut first_rgba = vec![0u8; (w * h * 4) as usize];
                            for y in 0..h as usize {
                                let src_off = y * full_w * 4;
                                let dst_off = y * w as usize * 4;
                                let row_len = w as usize * 4;
                                first_rgba[dst_off..dst_off + row_len]
                                    .copy_from_slice(&bytes[src_off..src_off + row_len]);
                            }
                            let (pw, ph, rgba) = downscale_preview_if_needed(w, h, first_rgba);
                            Ok((pw, ph, rgba, format!("{w}x{h} • {frame_count}f Aseprite")))
                        }
                        Err(_) => Err("Preview error".to_owned()),
                    }
                }
                Err(_) => Err("Preview error".to_owned()),
            },
            Err(_) => Err("Unreadable".to_owned()),
        });
    }

    if matches!(
        ext.as_str(),
        "gif" | "png" | "jpg" | "jpeg" | "bmp" | "webp" | "tga"
    ) {
        return Some(match image::open(path) {
            Ok(img) => {
                let rgba_img = img.to_rgba8();
                let (w, h) = rgba_img.dimensions();
                let (pw, ph, rgba) = downscale_preview_if_needed(w, h, rgba_img.into_raw());
                let kind = if ext == "gif" { "GIF (Frame 1)" } else { "Image" };
                Ok((pw, ph, rgba, format!("{w}x{h} • {kind}")))
            }
            Err(_) => Err("Preview error".to_owned()),
        });
    }

    None
}

/// Dock spec for the file chooser floating panel (dock id 208).
///
/// **Float-only:** `can_pop_out == false`; **resizable** so the listing can
/// grow. The window is titled [`FILE_CHOOSER_TITLE`] ("File Manager") and its
/// floating header carries the Close action.
pub fn file_chooser_panel_spec(host: &FileChooserHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(FILE_CHOOSER_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(FILE_CHOOSER_TITLE, false, FILE_CHOOSER_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(FileChooserPanelContent {
            nest: build_file_chooser_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pyx_chooser_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn matches_filter_follows_the_mode_contract() {
        let base = scratch_dir("filter");
        std::fs::write(base.join("a.pyxross"), b"x").unwrap();
        std::fs::write(base.join("b.PNG"), b"x").unwrap();
        std::fs::write(base.join("c.gif"), b"x").unwrap();
        std::fs::write(base.join("manifest.json"), b"{}").unwrap();
        std::fs::write(base.join("notes.txt"), b"x").unwrap();
        std::fs::create_dir_all(base.join("somedir")).unwrap();

        use FileChooserMode::*;
        // OpenProject: dirs + *.pyxross + manifest.json only.
        assert!(matches_filter(OpenProject, &base.join("a.pyxross")));
        assert!(matches_filter(OpenProject, &base.join("manifest.json")));
        assert!(!matches_filter(OpenProject, &base.join("b.PNG")));
        assert!(!matches_filter(OpenProject, &base.join("notes.txt")));
        // ImportPng: dirs + *.png (case-insensitive).
        assert!(matches_filter(ImportPng, &base.join("b.PNG")));
        assert!(!matches_filter(ImportPng, &base.join("a.pyxross")));
        assert!(!matches_filter(ImportPng, &base.join("notes.txt")));
        // ImportGif: dirs + *.gif (case-insensitive); unlike ImportPng it must
        // NOT list PNGs.
        assert!(matches_filter(ImportGif, &base.join("c.gif")));
        assert!(matches_filter(ImportGif, &base.join("somedir")));
        assert!(!matches_filter(ImportGif, &base.join("b.PNG")));
        assert!(!matches_filter(ImportGif, &base.join("a.pyxross")));
        assert!(!matches_filter(ImportGif, &base.join("notes.txt")));
        // ChooseFolder: directories only.
        assert!(matches_filter(ChooseFolder, &base.join("somedir")));
        assert!(!matches_filter(ChooseFolder, &base.join("a.pyxross")));
        assert!(!matches_filter(ChooseFolder, &base.join("b.PNG")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn import_gif_mode_title_confirm_and_case_insensitive_filter() {
        // Title/confirm label per a dedicated assertion (the 20 existing tests
        // cover the other modes).
        assert_eq!(FileChooserMode::ImportGif.title(), "Import GIF");
        assert_eq!(FileChooserMode::ImportGif.confirm_label(), "Import");
        // The `.gif` filter is case-insensitive, mirroring the `.png` one.
        assert!(matches_filter(
            FileChooserMode::ImportGif,
            Path::new("/x/sprite.GIF")
        ));
        assert!(matches_filter(
            FileChooserMode::ImportGif,
            Path::new("/x/sprite.gif")
        ));
        assert!(!matches_filter(
            FileChooserMode::ImportGif,
            Path::new("/x/sprite.gif.bak")
        ));
    }

    #[test]
    fn sort_entries_puts_directories_first_then_case_insensitive_names() {
        let entries = vec![
            Entry {
                name: "zeta.txt".to_owned(),
                path: PathBuf::from("/z"),
                is_dir: false,
            },
            Entry {
                name: "Beta".to_owned(),
                path: PathBuf::from("/b"),
                is_dir: true,
            },
            Entry {
                name: "alpha".to_owned(),
                path: PathBuf::from("/a"),
                is_dir: false,
            },
            Entry {
                name: "Gamma".to_owned(),
                path: PathBuf::from("/g"),
                is_dir: true,
            },
        ];
        let sorted = sort_entries(entries);
        let names: Vec<&str> = sorted.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Beta", "Gamma", "alpha", "zeta.txt"],
            "directories first, then case-insensitive names"
        );
    }

    #[test]
    fn read_dir_entries_filters_and_sorts() {
        let base = scratch_dir("list");
        std::fs::write(base.join("b.png"), b"x").unwrap();
        std::fs::write(base.join("a.pyxross"), b"x").unwrap();
        std::fs::write(base.join("c.txt"), b"x").unwrap();
        std::fs::create_dir_all(base.join("dir1")).unwrap();

        let open = read_dir_entries(FileChooserMode::OpenProject, &base).unwrap();
        let names: Vec<&str> = open.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["dir1", "a.pyxross"], "dir first, then file");

        let png = read_dir_entries(FileChooserMode::ImportPng, &base).unwrap();
        assert_eq!(png.len(), 2, "dir + one png");

        let folder = read_dir_entries(FileChooserMode::ChooseFolder, &base).unwrap();
        assert_eq!(folder.len(), 1);
        assert!(folder[0].is_dir);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn read_dir_entries_missing_directory_is_an_error() {
        let err = read_dir_entries(FileChooserMode::OpenProject, Path::new("/definitely/missing/nowhere")).unwrap_err();
        assert!(err.contains("cannot read"), "{err}");
    }

    #[test]
    fn parent_dir_returns_the_parent() {
        assert_eq!(
            parent_dir(Path::new("/a/b/c")),
            Some(PathBuf::from("/a/b"))
        );
        assert_eq!(parent_dir(Path::new("only_name")), Some(PathBuf::from("")));
    }

    #[test]
    fn navigate_rebuilds_entries_and_syncs_path_edit() {
        let base = scratch_dir("nav");
        std::fs::write(base.join("doc.pyxross"), b"x").unwrap();
        let mut view = FileChooserView {
            mode: FileChooserMode::OpenProject,
            ..FileChooserView::default()
        };
        navigate(&mut view, base.clone()).unwrap();
        assert_eq!(view.cwd, base);
        assert_eq!(view.path_edit, base.display().to_string());
        assert_eq!(view.entries.len(), 1);
        assert!(view.entries[0].name.ends_with("doc.pyxross"));
        assert!(view.selected.is_none());
        // Non-directory target sets the inline error.
        let file = base.join("doc.pyxross");
        let err = navigate(&mut view, file.clone()).unwrap_err();
        assert!(err.contains("is not a directory"));
        assert!(view.error.is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn listing_is_flat_and_directories_first() {
        let base = scratch_dir("flat");
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("inner.pyxross"), b"x").unwrap();
        std::fs::write(base.join("top.pyxross"), b"x").unwrap();
        let mut view = FileChooserView {
            mode: FileChooserMode::OpenProject,
            ..FileChooserView::default()
        };
        navigate(&mut view, base.clone()).unwrap();
        let names: Vec<&str> = view.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["sub", "top.pyxross"],
            "flat listing: directories first, then files"
        );
        assert!(view.entries[0].is_dir);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn go_to_edit_navigates_to_the_path_row_target() {
        let base = scratch_dir("goto");
        let sub = base.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        let mut view = FileChooserView {
            mode: FileChooserMode::ChooseFolder,
            ..FileChooserView::default()
        };
        view.path_edit = sub.display().to_string();
        go_to_edit(&mut view).unwrap();
        assert_eq!(view.cwd, sub);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_selection_requires_a_selection() {
        let mut view = FileChooserView::default();
        assert!(confirm_selection(&view).is_err());
        view.selected = Some(PathBuf::from("/tmp/thing.pyxross"));
        assert_eq!(
            confirm_selection(&view).unwrap(),
            PathBuf::from("/tmp/thing.pyxross")
        );
    }

    #[test]
    fn confirm_selection_in_choose_folder_uses_cwd_when_name_is_empty() {
        let base = scratch_dir("choose_cwd");
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let mut view = FileChooserView {
            mode: FileChooserMode::ChooseFolder,
            ..FileChooserView::default()
        };
        navigate(&mut view, base.clone()).unwrap();
        // Empty File name -> confirms the current directory `cwd`.
        assert_eq!(confirm_selection(&view).unwrap(), base);
        // Filled File name -> confirms that target folder.
        view.name_edit = "sub".to_owned();
        assert_eq!(confirm_selection(&view).unwrap(), sub);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn create_new_folder_makes_and_selects_a_unique_directory() {
        let base = scratch_dir("newfolder");
        let mut view = FileChooserView {
            mode: FileChooserMode::ChooseFolder,
            ..FileChooserView::default()
        };
        navigate(&mut view, base.clone()).unwrap();
        let first = create_new_folder(&mut view).unwrap();
        assert!(first.is_dir());
        assert_eq!(file_name_of(&first), "New Folder");
        assert_eq!(view.selected.as_deref(), Some(first.as_path()));
        let second = create_new_folder(&mut view).unwrap();
        assert_ne!(first, second);
        assert_eq!(file_name_of(&second), "New Folder (2)");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn history_back_and_forward_walk_the_visited_directories() {
        let base = scratch_dir("history");
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut view = FileChooserView {
            mode: FileChooserMode::ChooseFolder,
            ..FileChooserView::default()
        };
        navigate(&mut view, a.clone()).unwrap();
        navigate(&mut view, b.clone()).unwrap();
        assert_eq!(view.history_pos, 1);
        assert_eq!(view.history.len(), 2);

        go_back(&mut view).unwrap();
        assert_eq!(view.cwd, a);
        assert_eq!(view.history_pos, 0);
        go_forward(&mut view).unwrap();
        assert_eq!(view.cwd, b);
        assert_eq!(view.history_pos, 1);

        // Navigating after going back truncates the forward branch.
        go_back(&mut view).unwrap();
        navigate(&mut view, base.clone()).unwrap();
        assert_eq!(view.history.len(), 2);
        assert_eq!(view.history_pos, 1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn go_up_navigates_to_the_parent_directory() {
        let base = scratch_dir("goup");
        let sub = base.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let mut view = FileChooserView {
            mode: FileChooserMode::ChooseFolder,
            ..FileChooserView::default()
        };
        navigate(&mut view, sub.clone()).unwrap();
        go_up(&mut view).unwrap();
        assert_eq!(view.cwd, base);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn confirm_selection_rejects_a_file_outside_the_mode_filter() {
        let mut view = FileChooserView {
            mode: FileChooserMode::ImportPng,
            ..FileChooserView::default()
        };
        view.selected = Some(PathBuf::from("/tmp/notes.txt"));
        assert!(confirm_selection(&view).is_err());
    }

    #[test]
    fn confirm_selection_accepts_a_typed_name_that_exists() {
        let base = scratch_dir("typed");
        std::fs::write(base.join("typed.pyxross"), b"x").unwrap();
        let mut view = FileChooserView {
            mode: FileChooserMode::OpenProject,
            ..FileChooserView::default()
        };
        navigate(&mut view, base.clone()).unwrap();
        assert!(view.name_edit.is_empty(), "navigation clears the typed name");
        view.name_edit = "typed.pyxross".to_owned();
        assert_eq!(
            confirm_selection(&view).unwrap(),
            base.join("typed.pyxross")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ancestors_are_shallowest_first() {
        assert_eq!(
            ancestors(Path::new("/a/b/c")),
            vec![
                PathBuf::from("/"),
                PathBuf::from("/a"),
                PathBuf::from("/a/b"),
                PathBuf::from("/a/b/c"),
            ]
        );
    }

    #[test]
    fn spec_is_float_only_resizable_and_uses_a_static_nest() {
        let host = FileChooserHost::default();
        {
            let mut v = host.view.borrow_mut();
            v.mode = FileChooserMode::ImportPng;
            v.path_edit = "/tmp".to_owned();
        }
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), vec2(420.0, 360.0));
        let spec = file_chooser_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), FILE_CHOOSER_PANEL_ID);
        assert_eq!(spec.metadata.title, "File Manager");
        assert!(!spec.metadata.can_pop_out);
        assert!(spec.metadata.resizable, "the chooser listing grows");
        assert_eq!(spec.placement, PanelPlacement::Floating);
        let nest = build_file_chooser_nest(&host);
        assert_eq!(nest.mode(), NestMode::Static);
    }

    #[test]
    fn mode_titles_and_confirm_labels_cover_every_mode() {
        use FileChooserMode::*;
        let cases = [
            (OpenProject, "Open Project", "Open"),
            (ImportPng, "Import PNG", "Import"),
            (ChooseFolder, "Choose Folder", "Choose"),
        ];
        for (mode, title, label) in cases {
            assert_eq!(mode.title(), title);
            assert_eq!(mode.confirm_label(), label);
        }
    }

    /// Drive one headless frame of the chooser component.
    fn run_component_frame(
        ctx: &egui::Context,
        host: &FileChooserHost,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(520.0, 420.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome =
            PanelChrome::flat(&theme, &capture, crate::ui::input_capture::SurfaceId::Canvas);
        let mut component = FileChooserComponent::new(host.clone());
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
    fn component_paints_a_frame_without_panicking() {
        let ctx = egui::Context::default();
        let base = scratch_dir("frame");
        std::fs::write(base.join("a.pyxross"), b"x").unwrap();
        let host = FileChooserHost::default();
        {
            let mut view = host.view.borrow_mut();
            view.mode = FileChooserMode::OpenProject;
            navigate(&mut view, base.clone()).unwrap();
        }
        let output = run_component_frame(&ctx, &host, Vec::new());
        assert!(!output.shapes.is_empty(), "the chooser paints content");
        assert!(
            !text_positions(&output, "a.pyxross").is_empty(),
            "the listing row name is painted"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn listed_names_are_left_aligned_at_the_same_x() {
        let ctx = egui::Context::default();
        let base = scratch_dir("align");
        std::fs::create_dir_all(base.join("a_folder")).unwrap();
        std::fs::write(base.join("b_file.pyxross"), b"x").unwrap();
        let host = FileChooserHost::default();
        {
            let mut view = host.view.borrow_mut();
            view.mode = FileChooserMode::OpenProject;
            navigate(&mut view, base.clone()).unwrap();
        }
        let output = run_component_frame(&ctx, &host, Vec::new());
        let folder_x = text_positions(&output, "a_folder")
            .into_iter()
            .next()
            .expect("the folder row renders its name")
            .x;
        let file_x = text_positions(&output, "b_file.pyxross")
            .into_iter()
            .next()
            .expect("the file row renders its name")
            .x;
        assert_eq!(
            folder_x, file_x,
            "folder and file names must start at the same left edge"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn clicking_a_file_row_selects_it_and_fills_the_name_field() {
        let ctx = egui::Context::default();
        let base = scratch_dir("click");
        std::fs::write(base.join("pick_me.pyxross"), b"x").unwrap();
        let host = FileChooserHost::default();
        {
            let mut view = host.view.borrow_mut();
            view.mode = FileChooserMode::OpenProject;
            navigate(&mut view, base.clone()).unwrap();
        }
        let output = run_component_frame(&ctx, &host, Vec::new());
        let pos = text_positions(&output, "pick_me.pyxross")
            .into_iter()
            .next()
            .expect("the file row renders its name");
        let at = pos + egui::vec2(8.0, 8.0);
        run_component_frame(&ctx, &host, vec![egui::Event::PointerMoved(at)]);
        run_component_frame(
            &ctx,
            &host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        run_component_frame(
            &ctx,
            &host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let view = host.view.borrow();
        let expected = base.join("pick_me.pyxross");
        assert_eq!(view.selected.as_deref(), Some(expected.as_path()));
        assert_eq!(view.name_edit, "pick_me.pyxross");
        assert!(host.events.borrow().is_empty(), "selection alone does not confirm");
        drop(view);
        let _ = std::fs::remove_dir_all(&base);
    }
}
