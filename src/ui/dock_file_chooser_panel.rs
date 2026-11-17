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
const FILE_CHOOSER_MIN_SIZE: egui::Vec2 = vec2(520.0, 420.0);

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
            FileChooserMode::ChooseFolder => "Choose Folder",
        }
    }

    /// Label of the mode's confirm action (shown as the OK button's tooltip).
    pub fn confirm_label(self) -> &'static str {
        match self {
            FileChooserMode::OpenProject => "Open",
            FileChooserMode::ImportPng => "Import",
            FileChooserMode::ImportGif => "Import",
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

/// The chooser form: pinned toolbar, growing listing well, pinned footer.
struct FileChooserComponent {
    host: FileChooserHost,
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
            view.path_edit = path_edit.clone();
            view.name_edit = name_edit.clone();
            view.focus_requested = false;
            if let Some(path) = actions.select.clone() {
                view.selected = Some(path.clone());
                view.name_edit = file_name_of(&path);
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
                .hint_text("selected file")
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
    nest.push(FileChooserComponent { host: host.clone() });
    nest
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
        let mut component = FileChooserComponent { host: host.clone() };
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
