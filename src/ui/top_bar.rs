//! Top bar: a fixed three-slice chrome band across the very top of the frame.
//!
//! Slices (top → bottom), totalling [`TOP_BAR_HEIGHT`], never resizable:
//! 1. **menu bar** — 1/4 of the height. The fixed **File** / **Canvas** menu
//!    bar ([`menu_bar`]): each title opens a dropdown that emits a
//!    [`TopBarEvent::MenuAction`]. The legacy [`TopBarHost::set_menus`] label
//!    seam is retained for API compatibility.
//! 2. **middle bar** — 2/4 of the height, holding the square "create new
//!    document" button (emits [`TopBarEvent::OpenCreateDocument`]).
//! 3. **project tab bar** — 1/4 of the height. Lists every open project
//!    (`ProjectStore::tabs()`), highlights the active one, shows a dirty
//!    marker, and emits [`TopBarEvent::SelectProject`] on click.
//!
//! ## Z-order
//! The bar is painted in the central-panel (background) layer AFTER the canvas
//! and the docked panels, so it sits ABOVE them. Floating dock panels are
//! `egui::Window`s in `Order::Middle` and therefore render ABOVE the bar; the
//! dock viewport (`show_inside`) receives the body rect *below* the bar, so
//! docked panels and the canvas never overlap it in the first place.
//!
//! ## Testability
//! The geometry ([`slice_heights`], [`split_frame`]) is pure and separated from
//! the painter ([`TopBarHost::ui`]), so the slice sizes and the exact
//! top-bar/body tiling are unit-tested without an egui frame.

use egui::{pos2, vec2, Align2, FontId, Id, Rect, Sense, Stroke, StrokeKind};

use crate::ui::project::{ProjectId, ProjectTab};
use crate::ui::theme::ThemeColors;

/// Total fixed top-bar height, in points. `21 + 56.7 + 25.2 = 102.9`.
pub const TOP_BAR_HEIGHT: f32 = 102.9;

/// Fixed heights of the menu (top) and project-tab (bottom) slices. These are
/// unchanged from the original 84pt bar; only the MIDDLE slice grew (by 50%,
/// from 37.8 to 56.7), which is what makes the bar thicker overall.
const MENU_SLICE_HEIGHT: f32 = 21.0;
const TABS_SLICE_HEIGHT: f32 = 25.2;
const MIDDLE_SLICE_HEIGHT: f32 = 56.7;

/// Padding inside a slice, and horizontal tab metrics.
const SLICE_INSET: f32 = 4.0;
const TAB_WIDTH: f32 = 140.0;
const TAB_GAP: f32 = 4.0;
/// Side of a tab's trailing "×" close button, and its inset from the tab edge.
const TAB_CLOSE_SIZE: f32 = 16.0;

/// The three slice heights `(menu, middle, tabs)`, top → bottom: `21 / 56.7 /
/// 25.2`. The menu and tab slices are unchanged from the 84pt bar; the middle
/// slice is 50% larger, so the whole bar is thicker ([`TOP_BAR_HEIGHT`]). They
/// sum back to the total.
pub fn slice_heights() -> (f32, f32, f32) {
    (MENU_SLICE_HEIGHT, MIDDLE_SLICE_HEIGHT, TABS_SLICE_HEIGHT)
}

/// One label in the (currently empty) menu-bar slice. The documented seam for
/// adding real menu items later: fill [`TopBarHost::set_menus`] and the top
/// slice draws one entry per label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuLabel {
    pub label: String,
    pub enabled: bool,
}

impl MenuLabel {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
        }
    }
}

/// An event the top bar emits for the App shell to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopBarEvent {
    /// Switch the active project to `ProjectId`.
    SelectProject(ProjectId),
    /// Close the project `ProjectId` (the tab's "×" button).
    CloseProject(ProjectId),
    /// Open (or raise) the "create new document" floating panel.
    OpenCreateDocument,
    /// Toggle the floating Color Picker (clicking the FG swatch in the top bar).
    ToggleColorPicker,
    /// Swap foreground and background colors (clicking the BG swatch in the top bar).
    SwapColors,
    /// A menu-bar item was activated.
    MenuAction(MenuAction),
}

/// An action produced by a menu-bar dropdown item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewProject,
    SaveProject,
    SaveProjectAs,
    OpenProject,
    ImportImages,
    ImportGif,
    ImportPyxel,
    ImportAseprite,
    ImportImage2Pixel,
    ExportImage,
    ExportSpriteSheet,
    ExportGif,
    ExportPyxel,
    ExportAseprite,
    ExportSelected,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    DeleteSelection,
    SelectAll,
    Deselect,
    InvertSelection,
    FlipSelectionHorizontal,
    FlipSelectionVertical,
    RotateSelection,
    ResizeCanvas,
    LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind),
    Style,
    KeyBindings,
    EditorOptions,
    Credits,
}

/// One selectable entry in a menu dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuItem {
    /// Optional section header rendered above this item (with a divider line
    /// when not the first item in the dropdown).
    pub group: Option<&'static str>,
    pub label: &'static str,
    pub action: MenuAction,
}

const fn item(label: &'static str, action: MenuAction) -> MenuItem {
    MenuItem {
        group: None,
        label,
        action,
    }
}

const fn grouped_item(
    group: &'static str,
    label: &'static str,
    action: MenuAction,
) -> MenuItem {
    MenuItem {
        group: Some(group),
        label,
        action,
    }
}

/// A menu-bar title plus its dropdown. `dropdown_title` is an optional centered
/// header line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuDef {
    pub title: &'static str,
    pub dropdown_title: Option<&'static str>,
    pub items: &'static [MenuItem],
}

static FILE_ITEMS: [MenuItem; 15] = [
    grouped_item("Document", "New Project...", MenuAction::NewProject),
    item("Save Project", MenuAction::SaveProject),
    item("Save Project As...", MenuAction::SaveProjectAs),
    item("Open Project...", MenuAction::OpenProject),
    grouped_item("Import", "Import Images...", MenuAction::ImportImages),
    item("Import GIF...", MenuAction::ImportGif),
    item("Import Pyxel...", MenuAction::ImportPyxel),
    item("Import Aseprite...", MenuAction::ImportAseprite),
    item("Image to Pixel...", MenuAction::ImportImage2Pixel),
    grouped_item("Export", "Export Image...", MenuAction::ExportImage),
    item("Export Sprite Sheet...", MenuAction::ExportSpriteSheet),
    item("Export GIF...", MenuAction::ExportGif),
    item("Export .pyxel...", MenuAction::ExportPyxel),
    item("Export .ase...", MenuAction::ExportAseprite),
    item("Export Selection...", MenuAction::ExportSelected),
];

static EDIT_ITEMS: [MenuItem; 12] = [
    grouped_item("History", "Undo", MenuAction::Undo),
    item("Redo", MenuAction::Redo),
    grouped_item("Clipboard", "Cut", MenuAction::Cut),
    item("Copy", MenuAction::Copy),
    item("Paste", MenuAction::Paste),
    item("Delete", MenuAction::DeleteSelection),
    grouped_item("Selection", "Select All", MenuAction::SelectAll),
    item("Deselect", MenuAction::Deselect),
    item("Invert Selection", MenuAction::InvertSelection),
    grouped_item("Transform", "Flip Horizontal", MenuAction::FlipSelectionHorizontal),
    item("Flip Vertical", MenuAction::FlipSelectionVertical),
    item("Rotate 90°", MenuAction::RotateSelection),
];

static CANVAS_ITEMS: [MenuItem; 1] = [grouped_item(
    "Canvas",
    "Resize Canvas...",
    MenuAction::ResizeCanvas,
)];

static PALETTE_ITEMS: [MenuItem; 7] = [
    grouped_item(
        "Builtin Presets",
        "Ross (32)",
        MenuAction::LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind::Ross),
    ),
    item(
        "PICO-8 (16)",
        MenuAction::LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind::Pico8),
    ),
    item(
        "Sweetie 16",
        MenuAction::LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind::Sweetie16),
    ),
    item(
        "DB16",
        MenuAction::LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind::Db16),
    ),
    item(
        "DB32",
        MenuAction::LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind::Db32),
    ),
    item(
        "Endesga 32",
        MenuAction::LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind::Endesga32),
    ),
    item(
        "AAP-64",
        MenuAction::LoadBuiltinPalette(crate::core::palette::BuiltinPaletteKind::Aap64),
    ),
];

static OPTIONS_ITEMS: [MenuItem; 4] = [
    grouped_item("Preferences", "Style...", MenuAction::Style),
    item("Key Bindings...", MenuAction::KeyBindings),
    item("Editor Options...", MenuAction::EditorOptions),
    grouped_item("About", "Credits", MenuAction::Credits),
];

static MENUS: [MenuDef; 5] = [
    MenuDef {
        title: "File",
        dropdown_title: None,
        items: &FILE_ITEMS,
    },
    MenuDef {
        title: "Edit",
        dropdown_title: None,
        items: &EDIT_ITEMS,
    },
    MenuDef {
        title: "Canvas",
        dropdown_title: None,
        items: &CANVAS_ITEMS,
    },
    MenuDef {
        title: "Palette",
        dropdown_title: None,
        items: &PALETTE_ITEMS,
    },
    MenuDef {
        title: "Options",
        dropdown_title: None,
        items: &OPTIONS_ITEMS,
    },
];

/// The fixed menu-bar definitions (File, then Canvas).
pub fn menu_bar() -> &'static [MenuDef] {
    &MENUS
}

/// The frame split into the fixed top bar and the remaining body. `top_rect`
/// and `body_rect` tile `frame` exactly: no gap, no overlap, and
/// `body_rect.top() == frame.top() + TOP_BAR_HEIGHT`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TopBarLayout {
    /// The whole fixed top bar (menus + middle + tabs).
    pub top_rect: Rect,
    /// Everything below the top bar: the canvas + dock viewport.
    pub body_rect: Rect,
    /// The menu-bar slice.
    pub menu_rect: Rect,
    /// The multi-purpose middle slice (empty placeholder today).
    pub middle_rect: Rect,
    /// The project tab-bar slice.
    pub tabs_rect: Rect,
}

/// Split `frame` into the fixed top bar and the rest of the frame. Pure.
pub fn split_frame(frame: Rect) -> TopBarLayout {
    let (menu_h, middle_h, _tabs_h) = slice_heights();
    let top_rect = Rect::from_min_max(frame.min, pos2(frame.max.x, frame.min.y + TOP_BAR_HEIGHT));
    let body_rect = Rect::from_min_max(pos2(frame.min.x, frame.min.y + TOP_BAR_HEIGHT), frame.max);
    let menu_rect = Rect::from_min_max(top_rect.min, pos2(top_rect.max.x, top_rect.min.y + menu_h));
    let middle_rect = Rect::from_min_max(
        pos2(top_rect.min.x, menu_rect.max.y),
        pos2(top_rect.max.x, menu_rect.max.y + middle_h),
    );
    let tabs_rect = Rect::from_min_max(pos2(top_rect.min.x, middle_rect.max.y), top_rect.max);
    TopBarLayout {
        top_rect,
        body_rect,
        menu_rect,
        middle_rect,
        tabs_rect,
    }
}

/// A small, FIXED left inset for the create button: close to the left edge but
/// never glued to it. Independent of the (square) button size and the slice
/// width.
const CREATE_BUTTON_LEFT_INSET: f32 = 20.0;

/// The square "create new document" button in the middle slice: pinned to the
/// LEFT edge of `middle_rect`. Pure, so tests can target it.
pub fn create_document_button_rect(layout: &TopBarLayout) -> Rect {
    let side = (layout.middle_rect.height() - 2.0 * SLICE_INSET).max(8.0);
    let inset = CREATE_BUTTON_LEFT_INSET.max(SLICE_INSET);
    let center = pos2(
        layout.middle_rect.left() + inset + side * 0.5,
        layout.middle_rect.center().y,
    );
    Rect::from_center_size(center, vec2(side, side))
}

/// The Foreground/Background color swatch cluster in the middle slice, placed
/// to the RIGHT of the create-document button with a divider gap.
pub fn fg_bg_swatch_rects(layout: &TopBarLayout) -> (Rect, Rect) {
    let create_rect = create_document_button_rect(layout);
    let side = ((layout.middle_rect.height() - 2.0 * SLICE_INSET) * 0.62).clamp(16.0, 32.0);
    let offset = (side * 0.42).round();
    let total = side + offset;
    let x0 = create_rect.right() + 20.0;
    let y0 = layout.middle_rect.center().y - total * 0.5;
    let fg = Rect::from_min_size(pos2(x0, y0), vec2(side, side));
    let bg = Rect::from_min_size(pos2(x0 + offset, y0 + offset), vec2(side, side));
    (fg, bg)
}

/// The trailing "×" close button inside a project tab: a small square pinned to
/// the tab's RIGHT edge, vertically centered. Pure, so tests can target it.
pub fn tab_close_button_rect(tab_rect: Rect) -> Rect {
    let side = TAB_CLOSE_SIZE.min(tab_rect.height() - 2.0 * SLICE_INSET).max(8.0);
    let center = pos2(
        tab_rect.right() - SLICE_INSET - side * 0.5,
        tab_rect.center().y,
    );
    Rect::from_center_size(center, vec2(side, side))
}

/// Menu-bar metrics: title width, dropdown width, item height and padding.
const MENU_TITLE_WIDTH: f32 = 72.0;
const MENU_DROPDOWN_WIDTH: f32 = 236.0;
const MENU_ITEM_HEIGHT: f32 = 22.0;
const MENU_DROPDOWN_PAD: f32 = 6.0;
const MENU_HEADER_HEIGHT: f32 = 20.0;
const MENU_GROUP_HEIGHT: f32 = 18.0;
const MENU_SEPARATOR_HEIGHT: f32 = 7.0;
const MENU_ITEM_TEXT_INSET: f32 = 10.0;

/// Layout rows computed for a menu dropdown: section dividers/headers and item rects.
#[derive(Clone, Debug)]
struct DropdownRowLayout {
    separator_y: Option<f32>,
    group_rect: Option<Rect>,
    item: MenuItem,
    item_rect: Rect,
}

fn compute_dropdown_rows(
    left: f32,
    top: f32,
    def: &MenuDef,
) -> (f32, Vec<DropdownRowLayout>) {
    let mut y = top + MENU_DROPDOWN_PAD;
    if def.dropdown_title.is_some() {
        y += MENU_HEADER_HEIGHT;
    }
    let row_left = left + MENU_DROPDOWN_PAD;
    let row_right = left + MENU_DROPDOWN_WIDTH - MENU_DROPDOWN_PAD;
    let mut rows = Vec::with_capacity(def.items.len());
    for (i, &item) in def.items.iter().enumerate() {
        let mut separator_y = None;
        let mut group_rect = None;
        if item.group.is_some() {
            if i > 0 {
                separator_y = Some(y + MENU_SEPARATOR_HEIGHT * 0.5);
                y += MENU_SEPARATOR_HEIGHT;
            }
            group_rect = Some(Rect::from_min_max(
                pos2(row_left, y),
                pos2(row_right, y + MENU_GROUP_HEIGHT),
            ));
            y += MENU_GROUP_HEIGHT;
        }
        let item_rect = Rect::from_min_max(
            pos2(row_left, y),
            pos2(row_right, y + MENU_ITEM_HEIGHT),
        );
        y += MENU_ITEM_HEIGHT;
        rows.push(DropdownRowLayout {
            separator_y,
            group_rect,
            item,
            item_rect,
        });
    }
    y += MENU_DROPDOWN_PAD;
    (y - top, rows)
}

/// The clickable title rects of the menu bar, in order (File, Canvas, …).
pub fn menu_title_rects(layout: &TopBarLayout) -> Vec<(usize, Rect)> {
    menu_bar()
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let x = layout.menu_rect.left() + SLICE_INSET + index as f32 * MENU_TITLE_WIDTH;
            let rect = Rect::from_min_max(
                pos2(x, layout.menu_rect.top() + 1.0),
                pos2(x + MENU_TITLE_WIDTH, layout.menu_rect.bottom() - 1.0),
            );
            (index, rect)
        })
        .collect()
}

/// The dropdown panel rect for the menu at `index`: below the clicked title,
/// extending BELOW the top bar (into the body) as needed. Pure.
pub fn menu_dropdown_rect(layout: &TopBarLayout, index: usize) -> Option<Rect> {
    let def = menu_bar().get(index)?;
    let title = menu_title_rects(layout)
        .into_iter()
        .find(|(i, _)| *i == index)
        .map(|(_, r)| r)?;
    let top = layout.menu_rect.bottom() + 1.0;
    let (height, _) = compute_dropdown_rows(title.left(), top, def);
    Some(Rect::from_min_size(
        pos2(title.left(), top),
        vec2(MENU_DROPDOWN_WIDTH, height),
    ))
}

/// The clickable rects of a dropdown's items, in order. Pure.
pub fn menu_item_rects(layout: &TopBarLayout, index: usize) -> Vec<(MenuItem, Rect)> {
    let Some(def) = menu_bar().get(index) else {
        return Vec::new();
    };
    let Some(dropdown) = menu_dropdown_rect(layout, index) else {
        return Vec::new();
    };
    let (_, rows) = compute_dropdown_rows(dropdown.left(), dropdown.top(), def);
    rows.into_iter()
        .map(|row| (row.item, row.item_rect))
        .collect()
}

/// Dynamic shortcut strings displayed alongside Edit menu entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditMenuShortcuts {
    pub undo: String,
    pub redo: String,
    pub cut: String,
    pub copy: String,
    pub paste: String,
    pub delete: String,
    pub deselect: String,
    pub invert_selection: String,
    pub flip_h: String,
    pub flip_v: String,
    pub rotate: String,
}

impl EditMenuShortcuts {
    pub fn from_keymap(keymap: &crate::input::Keymap) -> Self {
        use crate::input::Action;
        Self {
            undo: keymap.display_label(Action::Undo),
            redo: keymap.display_label(Action::Redo),
            cut: keymap.display_label(Action::Cut),
            copy: keymap.display_label(Action::Copy),
            paste: keymap.display_label(Action::Paste),
            delete: keymap.display_label(Action::SelectDelete),
            deselect: "Esc".to_string(),
            invert_selection: keymap.display_label(Action::InvertSelection),
            flip_h: keymap.display_label(Action::FlipSelectionHorizontal),
            flip_v: keymap.display_label(Action::FlipSelectionVertical),
            rotate: keymap.display_label(Action::RotateSelection),
        }
    }
}

impl Default for EditMenuShortcuts {
    fn default() -> Self {
        Self::from_keymap(&crate::input::Keymap::defaults())
    }
}

/// Per-frame top-bar host: the App writes the menu labels and the open-project
/// snapshot, paints the bar, then drains the emitted events.
#[derive(Default)]
pub struct TopBarHost {
    /// Legacy label seam (kept for API compatibility; the fixed File/Canvas
    /// menu bar is what actually paints).
    menus: Vec<MenuLabel>,
    /// The open projects for this frame (from `ProjectStore::tabs()`).
    tabs: Vec<ProjectTab>,
    /// Events emitted while painting, drained by the App.
    events: Vec<TopBarEvent>,
    /// Index into [`menu_bar`] of the currently open dropdown, if any.
    open_menu: Option<usize>,
    /// Dynamic shortcut labels for the Edit menu.
    edit_shortcuts: EditMenuShortcuts,
    /// Optional right-aligned pixel-art banner rendered in the middle slice.
    banner: Option<(egui::TextureId, [u32; 2])>,
    /// Global foreground / background colors displayed in the middle slice.
    fg_color: crate::core::color::Color,
    bg_color: crate::core::color::Color,
}

impl TopBarHost {
    pub fn new() -> Self {
        Self {
            fg_color: crate::core::color::Color::BLACK,
            bg_color: crate::core::color::Color::WHITE,
            ..Self::default()
        }
    }

    pub fn set_colors(
        &mut self,
        fg: crate::core::color::Color,
        bg: crate::core::color::Color,
    ) {
        self.fg_color = fg;
        self.bg_color = bg;
    }

    pub fn set_edit_shortcuts(&mut self, shortcuts: EditMenuShortcuts) {
        self.edit_shortcuts = shortcuts;
    }

    pub fn set_banner(&mut self, banner: Option<(egui::TextureId, [u32; 2])>) {
        self.banner = banner;
    }

    fn item_label_and_shortcut(&self, item: MenuItem) -> (&'static str, Option<&str>) {
        let shortcut = match item.action {
            MenuAction::Undo => Some(self.edit_shortcuts.undo.as_str()),
            MenuAction::Redo => Some(self.edit_shortcuts.redo.as_str()),
            MenuAction::Cut => Some(self.edit_shortcuts.cut.as_str()),
            MenuAction::Copy => Some(self.edit_shortcuts.copy.as_str()),
            MenuAction::Paste => Some(self.edit_shortcuts.paste.as_str()),
            MenuAction::DeleteSelection => Some(self.edit_shortcuts.delete.as_str()),
            MenuAction::Deselect => Some(self.edit_shortcuts.deselect.as_str()),
            MenuAction::InvertSelection => Some(self.edit_shortcuts.invert_selection.as_str()),
            MenuAction::FlipSelectionHorizontal => Some(self.edit_shortcuts.flip_h.as_str()),
            MenuAction::FlipSelectionVertical => Some(self.edit_shortcuts.flip_v.as_str()),
            MenuAction::RotateSelection => Some(self.edit_shortcuts.rotate.as_str()),
            _ => None,
        };
        (item.label, shortcut.filter(|s| !s.is_empty()))
    }

    /// Legacy extension seam for extra menu labels, retained for API
    /// compatibility. The actual menu bar is the fixed [`menu_bar`] (File /
    /// Canvas); these labels are no longer painted. Passing an empty vec (the
    /// default) is a no-op.
    pub fn set_menus(&mut self, menus: Vec<MenuLabel>) {
        self.menus = menus;
    }

    /// The current menu labels (test/introspection seam).
    pub fn menus(&self) -> &[MenuLabel] {
        &self.menus
    }

    /// Snapshot the open projects for this frame. Call once per frame before
    /// [`Self::ui`] with `ProjectStore::tabs()`.
    pub fn set_tabs(&mut self, tabs: Vec<ProjectTab>) {
        self.tabs = tabs;
    }

    /// The tab snapshot painted last frame (test/introspection seam).
    pub fn tabs(&self) -> &[ProjectTab] {
        &self.tabs
    }

    /// Drain the events emitted since the last call.
    pub fn drain_events(&mut self) -> Vec<TopBarEvent> {
        std::mem::take(&mut self.events)
    }

    /// The index into [`menu_bar`] of the open dropdown, if any
    /// (test/introspection seam).
    pub fn open_menu(&self) -> Option<usize> {
        self.open_menu
    }

    /// Paint the fixed three-slice bar inside `layout` and record interactions
    /// into [`Self::events`]. Colors come exclusively from the theme tokens.
    pub fn ui(&mut self, ui: &mut egui::Ui, layout: &TopBarLayout, colors: &ThemeColors) {
        let painter = ui.painter().clone();

        // Backgrounds for the three fixed bands.
        painter.rect_filled(layout.menu_rect, 0.0, colors.panel_header_bg32());
        painter.rect_filled(layout.middle_rect, 0.0, colors.panel_bg32());
        painter.rect_filled(layout.tabs_rect, 0.0, colors.panel_header_bg32());
        if let Some((tex_id, [bw, bh])) = self.banner {
            if bw > 0 && bh > 0 && layout.middle_rect.height() > 0.0 {
                let target_h = layout.middle_rect.height();
                let draw_w = target_h * (bw as f32 / bh as f32);
                let banner_rect = Rect::from_min_max(
                    pos2(layout.middle_rect.right() - draw_w, layout.middle_rect.top()),
                    layout.middle_rect.right_bottom(),
                );
                let fade_frac = 0.38_f32;
                let fade_x = banner_rect.left() + banner_rect.width() * fade_frac;
                let mut mesh = egui::Mesh::with_texture(tex_id);
                let transparent = egui::Color32::TRANSPARENT;
                let opaque = crate::ui::theme::SKIN_TINT;
                // Left fade strip: transparent -> opaque.
                let v0 = mesh.vertices.len() as u32;
                let push_v = |m: &mut egui::Mesh, pos: egui::Pos2, uv: egui::Pos2, color: egui::Color32| {
                    m.vertices.push(egui::epaint::Vertex { pos, uv, color });
                };
                push_v(&mut mesh, banner_rect.left_top(), pos2(0.0, 0.0), transparent);
                push_v(&mut mesh, pos2(fade_x, banner_rect.top()), pos2(fade_frac, 0.0), opaque);
                push_v(&mut mesh, pos2(fade_x, banner_rect.bottom()), pos2(fade_frac, 1.0), opaque);
                push_v(&mut mesh, banner_rect.left_bottom(), pos2(0.0, 1.0), transparent);
                mesh.add_triangle(v0, v0 + 1, v0 + 2);
                mesh.add_triangle(v0, v0 + 2, v0 + 3);
                // Right solid strip: opaque -> opaque.
                let v1 = mesh.vertices.len() as u32;
                push_v(&mut mesh, pos2(fade_x, banner_rect.top()), pos2(fade_frac, 0.0), opaque);
                push_v(&mut mesh, banner_rect.right_top(), pos2(1.0, 0.0), opaque);
                push_v(&mut mesh, banner_rect.right_bottom(), pos2(1.0, 1.0), opaque);
                push_v(&mut mesh, pos2(fade_x, banner_rect.bottom()), pos2(fade_frac, 1.0), opaque);
                mesh.add_triangle(v1, v1 + 1, v1 + 2);
                mesh.add_triangle(v1, v1 + 2, v1 + 3);
                painter
                    .with_clip_rect(layout.middle_rect)
                    .add(egui::Shape::Mesh(mesh.into()));
            }
        }
        // Separators under the menu slice, under the middle slice, and at the
        // very bottom of the bar (so the bar reads as a distinct frame band).
        let border = Stroke::new(1.0, colors.panel_border32());
        painter.line_segment(
            [layout.menu_rect.left_bottom(), layout.menu_rect.right_bottom()],
            border,
        );
        painter.line_segment(
            [layout.middle_rect.left_bottom(), layout.middle_rect.right_bottom()],
            border,
        );
        painter.line_segment(
            [layout.top_rect.left_bottom(), layout.top_rect.right_bottom()],
            border,
        );

        // The bar claims its own rect so a click on empty bar space can never
        // reach the canvas. Registered BEFORE the tab widgets so a tab, laid
        // out later, wins the hit test where they overlap.
        let _ = ui.interact(layout.top_rect, Id::new("top-bar-capture"), Sense::click());

        let text_color = ui.visuals().text_color();

        // Close the open dropdown when a press lands outside its title and
        // panel (raw pointer input, so it is independent of egui widget
        // resolution order). Done BEFORE the title/item clicks are processed.
        if let Some(open) = self.open_menu {
            if ui.input(|i| i.pointer.any_pressed()) {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    let on_title = menu_title_rects(layout)
                        .iter()
                        .any(|(_, rect)| rect.contains(pos));
                    let on_dropdown =
                        menu_dropdown_rect(layout, open).is_some_and(|rect| rect.contains(pos));
                    if !on_title && !on_dropdown {
                        self.open_menu = None;
                    }
                }
            }
        }

        // While a dropdown is open, a full-frame scrim (registered BEFORE the
        // titles and items, so they win the hit test) swallows every other
        // click — a dropdown interaction can never reach the canvas.
        if self.open_menu.is_some() {
            let frame = layout.top_rect.union(layout.body_rect);
            let _ = ui.interact(frame, Id::new("top-bar-menu-scrim"), Sense::click());
        }

        // Menu bar: fixed File / Canvas titles. A click toggles the dropdown;
        // the open title is highlighted.
        for (index, rect) in menu_title_rects(layout) {
            let response = ui.interact(
                rect,
                Id::new(("top-bar-menu-title", index)),
                Sense::click(),
            );
            let open = self.open_menu == Some(index);
            if response.hovered() || open {
                painter.rect_filled(rect, 2.0, colors.selection_bg_fill32());
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                menu_bar()[index].title,
                FontId::proportional(13.0),
                text_color,
            );
            if response.clicked() {
                self.open_menu = if open { None } else { Some(index) };
            }
        }

        // Middle slice: New Document button on the LEFT, vertical divider, then FG/BG swatches.
        let button_rect = create_document_button_rect(layout);
        let divider_x = button_rect.right() + 10.0;
        painter.vline(
            divider_x,
            (layout.middle_rect.top() + 8.0)..=(layout.middle_rect.bottom() - 8.0),
            Stroke::new(1.0, colors.panel_border32()),
        );
        let (fg_rect, bg_rect) = fg_bg_swatch_rects(layout);
        let bg_resp = ui
            .interact(bg_rect, Id::new("top-bar-bg-swatch"), Sense::click())
            .on_hover_text("Background color (Click to swap FG/BG)");
        let fg_resp = ui
            .interact(fg_rect, Id::new("top-bar-fg-swatch"), Sense::click())
            .on_hover_text("Foreground color (Click to open Color Picker)");
        let bg32 = egui::Color32::from_rgba_unmultiplied(
            self.bg_color.r,
            self.bg_color.g,
            self.bg_color.b,
            self.bg_color.a,
        );
        let fg32 = egui::Color32::from_rgba_unmultiplied(
            self.fg_color.r,
            self.fg_color.g,
            self.fg_color.b,
            self.fg_color.a,
        );
        painter.rect_filled(bg_rect, 2.0, bg32);
        painter.rect_stroke(
            bg_rect,
            2.0,
            Stroke::new(1.0, colors.panel_border32()),
            StrokeKind::Inside,
        );
        painter.rect_filled(fg_rect, 2.0, fg32);
        painter.rect_stroke(
            fg_rect,
            2.0,
            Stroke::new(1.5, colors.selection_stroke_color32()),
            StrokeKind::Inside,
        );
        if fg_resp.clicked() {
            self.events.push(TopBarEvent::ToggleColorPicker);
        } else if bg_resp.clicked() {
            self.events.push(TopBarEvent::SwapColors);
        }
        if fg_resp.hovered() || bg_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let button = ui
            .interact(
                button_rect,
                Id::new("top-bar-create-document"),
                Sense::click(),
            )
            .on_hover_text("Create new document");
        let button_fill = if button.is_pointer_button_down_on() {
            colors.selection_stroke_color32()
        } else if button.hovered() {
            colors.selection_bg_fill32()
        } else {
            colors.panel_bg32()
        };
        painter.rect_filled(button_rect, 3.0, button_fill);
        painter.rect_stroke(
            button_rect,
            3.0,
            Stroke::new(1.0, colors.panel_border32()),
            StrokeKind::Inside,
        );
        paint_new_document_pixel_icon(&painter, button_rect, colors);
        if button.clicked() {
            self.events.push(TopBarEvent::OpenCreateDocument);
        }
        if button.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // Tab slice: one tab per open project.
        self.paint_tabs(ui, layout, colors);

        // Dropdown: painted LAST so it sits ABOVE every other top-bar element;
        // it may extend below the bar (into the body). Item clicks close the
        // menu and emit the action.
        self.paint_dropdown(ui, layout, colors);
    }

    /// Paint the currently open menu dropdown (if any) and handle its items.
    fn paint_dropdown(&mut self, ui: &mut egui::Ui, layout: &TopBarLayout, colors: &ThemeColors) {
        let Some(index) = self.open_menu else {
            return;
        };
        let Some(def) = menu_bar().get(index) else {
            self.open_menu = None;
            return;
        };
        let Some(dropdown) = menu_dropdown_rect(layout, index) else {
            return;
        };
        let painter = ui.painter().clone();
        let text_color = ui.visuals().text_color();
        let weak_color = ui.visuals().weak_text_color();
        // Panel background + border.
        painter.rect_filled(dropdown, 3.0, colors.panel_header_bg32());
        painter.rect_stroke(
            dropdown,
            3.0,
            Stroke::new(1.0, colors.panel_border32()),
            StrokeKind::Inside,
        );
        // Optional centered header.
        if let Some(header) = def.dropdown_title {
            let header_rect = Rect::from_min_max(
                dropdown.min,
                pos2(dropdown.right(), dropdown.top() + MENU_DROPDOWN_PAD + MENU_HEADER_HEIGHT),
            );
            painter.text(
                header_rect.center(),
                Align2::CENTER_CENTER,
                header,
                FontId::proportional(11.0),
                weak_color,
            );
        }
        let (_, rows) = compute_dropdown_rows(dropdown.left(), dropdown.top(), def);
        let mut chosen: Option<MenuAction> = None;
        for row in rows {
            if let Some(sep_y) = row.separator_y {
                painter.hline(
                    (dropdown.left() + MENU_DROPDOWN_PAD)..=(dropdown.right() - MENU_DROPDOWN_PAD),
                    sep_y,
                    Stroke::new(1.0, colors.panel_border32()),
                );
            }
            if let (Some(group_name), Some(group_rect)) = (row.item.group, row.group_rect) {
                painter.text(
                    pos2(group_rect.left() + MENU_ITEM_TEXT_INSET, group_rect.center().y),
                    Align2::LEFT_CENTER,
                    group_name,
                    FontId::proportional(10.5),
                    weak_color,
                );
            }
            let rect = row.item_rect;
            let response = ui.interact(
                rect,
                Id::new(("top-bar-menu-item", index, row.item.label)),
                Sense::click(),
            );
            if response.hovered() {
                painter.rect_filled(rect, 2.0, colors.selection_bg_fill32());
            }
            let (label, shortcut) = self.item_label_and_shortcut(row.item);
            painter.text(
                pos2(rect.left() + MENU_ITEM_TEXT_INSET, rect.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(12.5),
                text_color,
            );
            if let Some(shortcut) = shortcut {
                painter.text(
                    pos2(rect.right() - MENU_ITEM_TEXT_INSET, rect.center().y),
                    Align2::RIGHT_CENTER,
                    shortcut,
                    FontId::proportional(11.5),
                    weak_color,
                );
            }
            if response.clicked() {
                chosen = Some(row.item.action);
            }
        }
        if let Some(action) = chosen {
            self.events.push(TopBarEvent::MenuAction(action));
            self.open_menu = None;
        }
    }

    fn paint_tabs(&mut self, ui: &mut egui::Ui, layout: &TopBarLayout, colors: &ThemeColors) {
        let painter = ui.painter().clone();
        let tabs = self.tabs.clone();
        let version_text = format!("Pyxross Workshop {}", env!("CARGO_PKG_VERSION"));
        let version_font = FontId::proportional(11.5);
        let version_w = painter
            .layout_no_wrap(
                version_text.clone(),
                version_font.clone(),
                ui.visuals().weak_text_color(),
            )
            .size()
            .x;
        let tabs_right_limit = (layout.tabs_rect.right() - version_w - 18.0).max(layout.tabs_rect.left());
        painter.text(
            pos2(layout.tabs_rect.right() - 10.0, layout.tabs_rect.center().y),
            Align2::RIGHT_CENTER,
            version_text,
            version_font,
            ui.visuals().weak_text_color(),
        );

        let y = layout.tabs_rect.top() + SLICE_INSET;
        let height = (layout.tabs_rect.height() - 2.0 * SLICE_INSET).max(1.0);
        let text_color = ui.visuals().text_color();
        let mut x = layout.tabs_rect.left() + SLICE_INSET;
        for tab in &tabs {
            let rect = Rect::from_min_size(pos2(x, y), vec2(TAB_WIDTH, height));
            if rect.right() > tabs_right_limit {
                break;
            }
            let response = ui.interact(
                rect,
                Id::new(("top-bar-tab", tab.id.raw())),
                Sense::click(),
            );
            let fill = if tab.active {
                colors.selection_bg_fill32()
            } else {
                colors.panel_bg32()
            };
            painter.rect_filled(rect, 3.0, fill);
            let stroke_color = if tab.active {
                colors.selection_stroke_color32()
            } else {
                colors.panel_border32()
            };
            painter.rect_stroke(rect, 3.0, Stroke::new(1.0, stroke_color), StrokeKind::Inside);
            // The label sits in the space LEFT of the close button so the
            // "×" never overlaps it; long names are truncated with "...".
            let close_rect = tab_close_button_rect(rect);
            let label_rect = Rect::from_min_max(
                pos2(rect.left() + 6.0, rect.top()),
                pos2((close_rect.left() - 4.0).max(rect.left() + 6.0), rect.bottom()),
            );
            let font = FontId::proportional(13.0);
            let suffix = if tab.dirty { " •" } else { "" };
            let full_label = format!("{}{}", tab.name, suffix);
            let (display_label, was_truncated) =
                truncate_label_to_width(&painter, &tab.name, suffix, &font, label_rect.width());
            let label_painter = painter.with_clip_rect(label_rect.intersect(painter.clip_rect()));
            label_painter.text(
                label_rect.center(),
                Align2::CENTER_CENTER,
                display_label,
                font,
                text_color,
            );
            if was_truncated {
                response.clone().on_hover_text(full_label);
            }
            // Select: the whole tab, but a click on the "×" is claimed by the
            // close button registered AFTER this one (egui gives the later
            // widget the hit test), so it must not also select.
            if response.clicked() && !tab.active {
                self.events.push(TopBarEvent::SelectProject(tab.id));
            }
            let close = ui.interact(
                close_rect,
                Id::new(("top-bar-tab-close", tab.id.raw())),
                Sense::click(),
            );
            let close_fill = if close.is_pointer_button_down_on() {
                colors.selection_stroke_color32()
            } else if close.hovered() {
                colors.selection_bg_fill32()
            } else {
                colors.panel_bg32()
            };
            painter.rect_filled(close_rect, 2.0, close_fill);
            painter.text(
                close_rect.center(),
                Align2::CENTER_CENTER,
                "×",
                FontId::proportional(13.0),
                text_color,
            );
            if close.clicked() {
                self.events.push(TopBarEvent::CloseProject(tab.id));
            }
            if close.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            x += TAB_WIDTH + TAB_GAP;
        }
    }
}

/// 27x27 pixel-art ornament for the top bar's "New Document" button (trimmed of
/// outer transparent margins so the icon fills the button cleanly).
/// `#` = white (`SKIN_TINT`), `g` = theme accent (`selection_stroke_color32`),
/// `.` = transparent.
const NEW_DOCUMENT_PIXEL_ICON: [&str; 27] = [
    ".............#.............",
    "............###............",
    "..gggggg...#####...gggggg..",
    "..ggggggg.g##.##g.ggggggg..",
    "..ggg.................ggg..",
    "..gg.g####g.....g####g.gg..",
    "..gg.#####g..#..g#####.gg..",
    "..gg.#####..###..#####.gg..",
    "...g.####..g#.#g..####.g...",
    ".....###...........###.....",
    "...g.gg...##.g.##...gg.g...",
    "..##....g.#..#..#.#....##..",
    ".###...##...g#g...##...###.",
    "###...##..g#####g..##...###",
    ".###...##...g#g...##...###.",
    "..##....g.#..#..#.#....##..",
    "...g.gg...##.g.##...gg.g...",
    ".....###...........###.....",
    "...g.####..g#.#g..####.g...",
    "..gg.#####..###..#####.gg..",
    "..gg.#####g..#..g#####.gg..",
    "..gg.g####g.....g####g.gg..",
    "..ggg.................ggg..",
    "..ggggggg.g##.##g.ggggggg..",
    "..gggggg...#####...gggggg..",
    "............###............",
    ".............#.............",
];

fn paint_new_document_pixel_icon(
    painter: &egui::Painter,
    button_rect: Rect,
    colors: &ThemeColors,
) {
    let n = NEW_DOCUMENT_PIXEL_ICON.len() as f32;
    let pad = (button_rect.height() * 0.06).max(1.5);
    let inner = button_rect.shrink(pad);
    let side = inner.width().min(inner.height()).max(n);
    let cell = side / n;
    let origin = pos2(
        button_rect.center().x - side * 0.5,
        button_rect.center().y - side * 0.5,
    );
    let white = crate::ui::theme::SKIN_TINT;
    let accent = colors.selection_stroke_color32();
    for (row_idx, row) in NEW_DOCUMENT_PIXEL_ICON.iter().enumerate() {
        let y0 = origin.y + row_idx as f32 * cell;
        let y1 = origin.y + (row_idx + 1) as f32 * cell;
        for (col_idx, ch) in row.bytes().enumerate() {
            let color = match ch {
                b'#' => white,
                b'g' => accent,
                _ => continue,
            };
            let x0 = origin.x + col_idx as f32 * cell;
            let x1 = origin.x + (col_idx + 1) as f32 * cell;
            painter.rect_filled(Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), 0.0, color);
        }
    }
}

/// Truncates `name` with `"..."` (keeping any `suffix` such as `" •"`) so the
/// measured single-line text fits within `max_width`. Returns `(text, truncated)`.
fn truncate_label_to_width(
    painter: &egui::Painter,
    name: &str,
    suffix: &str,
    font: &FontId,
    max_width: f32,
) -> (String, bool) {
    let measure = |s: &str| {
        painter
            .layout_no_wrap(s.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };
    let full = format!("{name}{suffix}");
    if max_width <= 0.0 || measure(&full) <= max_width {
        return (full, false);
    }
    let chars: Vec<char> = name.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let prefix: String = chars[..mid].iter().collect();
        let candidate = format!("{prefix}...{suffix}");
        if measure(&candidate) <= max_width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let prefix: String = chars[..lo].iter().collect();
    (format!("{prefix}...{suffix}"), true)
}

/// Paints the monochrome pixel-art New Document emblem centered in `rect`
/// at an integer pixel scale so it reads cleanly as a workspace watermark.
pub(crate) fn paint_new_document_watermark(
    painter: &egui::Painter,
    rect: Rect,
    color: egui::Color32,
) {
    let n = NEW_DOCUMENT_PIXEL_ICON.len() as f32;
    let side = (rect.width().min(rect.height())).max(n);
    let cell = (side / n).floor().max(1.0);
    let draw_side = cell * n;
    let origin = pos2(
        (rect.center().x - draw_side * 0.5).round(),
        (rect.center().y - draw_side * 0.5).round(),
    );
    for (row_idx, row) in NEW_DOCUMENT_PIXEL_ICON.iter().enumerate() {
        let y0 = origin.y + row_idx as f32 * cell;
        let y1 = origin.y + (row_idx + 1) as f32 * cell;
        for (col_idx, ch) in row.bytes().enumerate() {
            if ch != b'#' && ch != b'g' {
                continue;
            }
            let x0 = origin.x + col_idx as f32 * cell;
            let x1 = origin.x + (col_idx + 1) as f32 * cell;
            painter.rect_filled(Rect::from_min_max(pos2(x0, y0), pos2(x1, y1)), 0.0, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_heights_are_21_56_7_25_2() {
        let (menu, middle, tabs) = slice_heights();
        assert!((menu - 21.0).abs() < 1e-4, "menu = {menu}");
        assert!((middle - 56.7).abs() < 1e-4, "middle = {middle}");
        assert!((tabs - 25.2).abs() < 1e-4, "tabs = {tabs}");
        assert!(
            (menu + middle + tabs - TOP_BAR_HEIGHT).abs() < 1e-3,
            "the slices must sum to TOP_BAR_HEIGHT"
        );
        assert!(
            (TOP_BAR_HEIGHT - 102.9).abs() < 1e-3,
            "the bar must grow to 102.9"
        );
    }

    #[test]
    fn tab_close_button_emits_close_project() {
        let ctx = egui::Context::default();
        let mut host = TopBarHost::new();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let frame = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let layout = split_frame(frame);
        // One tab at the left of the tab slice -> its "×" is targetable.
        let tab_rect = Rect::from_min_size(
            pos2(layout.tabs_rect.left() + SLICE_INSET, layout.tabs_rect.top() + SLICE_INSET),
            vec2(TAB_WIDTH, layout.tabs_rect.height() - 2.0 * SLICE_INSET),
        );
        let id = crate::ui::project::ProjectId::new(7);
        host.set_tabs(vec![crate::ui::project::ProjectTab {
            id,
            name: "Doc".to_string(),
            active: true,
            dirty: false,
        }]);
        let at = tab_close_button_rect(tab_rect).center();

        let run = |host: &mut TopBarHost, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(frame),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| host.ui(ui, &layout, &colors));
            });
            output.textures_delta.clear();
        };
        run(&mut host, vec![egui::Event::PointerMoved(at)]);
        run(
            &mut host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        run(
            &mut host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );

        let events = host.drain_events();
        assert!(
            events.contains(&TopBarEvent::CloseProject(id)),
            "clicking a tab's × must emit CloseProject({id:?}): {events:?}"
        );
        assert!(
            !events.contains(&TopBarEvent::SelectProject(id)),
            "the × click must not also select the tab: {events:?}"
        );
    }

    #[test]
    fn create_document_button_is_square_and_inside_the_middle_slice() {
        let frame = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let layout = split_frame(frame);
        let button = create_document_button_rect(&layout);
        assert!(
            (button.width() - button.height()).abs() < 1e-4,
            "the button must be square: {button:?}"
        );
        // Side length approximates the middle slice height.
        assert!(
            (button.height() - (layout.middle_rect.height() - 2.0 * SLICE_INSET)).abs() < 1e-4
        );
        // Offset a small FIXED inset from the LEFT edge (not glued, but close).
        assert!(layout.middle_rect.contains_rect(button), "button inside middle");
        let left_inset = button.left() - layout.middle_rect.left();
        assert!(
            left_inset > SLICE_INSET + 1.0,
            "button must NOT be glued to the left edge (inset {left_inset}): {button:?}"
        );
        assert!(
            (left_inset - CREATE_BUTTON_LEFT_INSET).abs() < 1e-3,
            "left inset must be the small fixed {CREATE_BUTTON_LEFT_INSET}: {button:?}"
        );
        assert!(
            left_inset < layout.middle_rect.width() / 4.0,
            "button should sit near the left edge: {button:?}"
        );
        let (fg_rect, bg_rect) = fg_bg_swatch_rects(&layout);
        assert!(layout.middle_rect.contains_rect(fg_rect), "fg swatch inside middle");
        assert!(layout.middle_rect.contains_rect(bg_rect), "bg swatch inside middle");
        assert!(
            button.right() < fg_rect.left(),
            "create button sits on the left while fg/bg swatches sit to its right"
        );
    }

    /// Drive one full move/press/release click at `at` through `host.ui`.
    fn click_host(
        ctx: &egui::Context,
        host: &mut TopBarHost,
        layout: &TopBarLayout,
        colors: &crate::ui::theme::ThemeColors,
        at: egui::Pos2,
    ) {
        let run = |host: &mut TopBarHost, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(layout.top_rect.union(layout.body_rect)),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| host.ui(ui, layout, colors));
            });
            output.textures_delta.clear();
        };
        run(host, vec![egui::Event::PointerMoved(at)]);
        run(
            host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        run(
            host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    #[test]
    fn menu_bar_is_file_then_edit_then_canvas_with_document_header() {
        let menus = menu_bar();
        assert_eq!(menus.len(), 5, "five menu titles");
        assert_eq!(menus[0].title, "File");
        assert_eq!(menus[1].title, "Edit");
        assert_eq!(menus[2].title, "Canvas");
        assert_eq!(menus[3].title, "Palette");
        assert_eq!(menus[4].title, "Options");
        assert_eq!(
            menus[0].items[0].group,
            Some("Document"),
            "the File dropdown starts with the 'Document' group"
        );
        assert_eq!(menus[0].items.len(), 15);
        assert_eq!(menus[1].items.len(), 12);
        assert_eq!(menus[2].items.len(), 1);
        assert_eq!(menus[2].items[0].label, "Resize Canvas...");
        assert_eq!(menus[3].items.len(), 7);
        assert_eq!(menus[3].items[0].label, "Ross (32)");
        assert_eq!(menus[4].items.len(), 4);
        assert_eq!(menus[4].items[0].label, "Style...");
        assert_eq!(menus[4].items[1].label, "Key Bindings...");
        assert_eq!(menus[4].items[2].label, "Editor Options...");
        assert_eq!(menus[4].items[3].label, "Credits");
    }

    #[test]
    fn file_menu_each_item_emits_its_action() {
        let ctx = egui::Context::default();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let layout = split_frame(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
        let mut host = TopBarHost::new();
        let file_title = menu_title_rects(&layout)[0].1.center();

        let expected = [
            MenuAction::NewProject,
            MenuAction::SaveProject,
            MenuAction::SaveProjectAs,
            MenuAction::OpenProject,
            MenuAction::ImportImages,
            MenuAction::ImportGif,
            MenuAction::ImportPyxel,
            MenuAction::ImportAseprite,
            MenuAction::ImportImage2Pixel,
            MenuAction::ExportImage,
            MenuAction::ExportSpriteSheet,
            MenuAction::ExportGif,
            MenuAction::ExportPyxel,
            MenuAction::ExportAseprite,
            MenuAction::ExportSelected,
        ];
        let items = menu_item_rects(&layout, 0);
        assert_eq!(items.len(), 15);
        for (i, (item, rect)) in items.iter().enumerate() {
            // Open File, then click this item.
            click_host(&ctx, &mut host, &layout, &colors, file_title);
            click_host(&ctx, &mut host, &layout, &colors, rect.center());
            let events = host.drain_events();
            assert!(
                events.contains(&TopBarEvent::MenuAction(expected[i])),
                "item {:?} must emit {:?}, got {events:?}",
                item.label,
                expected[i]
            );
        }
    }

    #[test]
    fn edit_menu_each_item_emits_its_action() {
        let ctx = egui::Context::default();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let layout = split_frame(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
        let mut host = TopBarHost::new();
        let edit_title = menu_title_rects(&layout)[1].1.center();

        let expected = [
            MenuAction::Undo,
            MenuAction::Redo,
            MenuAction::Cut,
            MenuAction::Copy,
            MenuAction::Paste,
            MenuAction::DeleteSelection,
            MenuAction::SelectAll,
            MenuAction::Deselect,
            MenuAction::InvertSelection,
            MenuAction::FlipSelectionHorizontal,
            MenuAction::FlipSelectionVertical,
            MenuAction::RotateSelection,
        ];
        let items = menu_item_rects(&layout, 1);
        assert_eq!(items.len(), 12);
        for (i, (item, rect)) in items.iter().enumerate() {
            click_host(&ctx, &mut host, &layout, &colors, edit_title);
            click_host(&ctx, &mut host, &layout, &colors, rect.center());
            let events = host.drain_events();
            assert!(
                events.contains(&TopBarEvent::MenuAction(expected[i])),
                "item {:?} must emit {:?}, got {events:?}",
                item.label,
                expected[i]
            );
        }
    }

    #[test]
    fn canvas_menu_resize_item_emits_resize_canvas() {
        let ctx = egui::Context::default();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let layout = split_frame(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
        let mut host = TopBarHost::new();
        let canvas_title = menu_title_rects(&layout)[2].1.center();
        let items = menu_item_rects(&layout, 2);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0.label, "Resize Canvas...");

        click_host(&ctx, &mut host, &layout, &colors, canvas_title);
        click_host(&ctx, &mut host, &layout, &colors, items[0].1.center());
        assert_eq!(
            host.drain_events(),
            vec![TopBarEvent::MenuAction(MenuAction::ResizeCanvas)]
        );
    }

    #[test]
    fn options_menu_style_item_emits_style() {
        let ctx = egui::Context::default();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let layout = split_frame(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
        let mut host = TopBarHost::new();
        let options_title = menu_title_rects(&layout)[4].1.center();
        let items = menu_item_rects(&layout, 4);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].0.label, "Style...");
        assert_eq!(items[1].0.label, "Key Bindings...");
        assert_eq!(items[2].0.label, "Editor Options...");
        assert_eq!(items[3].0.label, "Credits");

        click_host(&ctx, &mut host, &layout, &colors, options_title);
        click_host(&ctx, &mut host, &layout, &colors, items[0].1.center());
        assert_eq!(
            host.drain_events(),
            vec![TopBarEvent::MenuAction(MenuAction::Style)]
        );

        click_host(&ctx, &mut host, &layout, &colors, options_title);
        click_host(&ctx, &mut host, &layout, &colors, items[1].1.center());
        assert_eq!(
            host.drain_events(),
            vec![TopBarEvent::MenuAction(MenuAction::KeyBindings)]
        );
    }

    #[test]
    fn palette_menu_emits_load_builtin_palette() {
        let ctx = egui::Context::default();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let layout = split_frame(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
        let mut host = TopBarHost::new();
        let palette_title = menu_title_rects(&layout)[3].1.center();
        let items = menu_item_rects(&layout, 3);
        assert_eq!(items.len(), 7);
        click_host(&ctx, &mut host, &layout, &colors, palette_title);
        click_host(&ctx, &mut host, &layout, &colors, items[0].1.center());
        assert_eq!(
            host.drain_events(),
            vec![TopBarEvent::MenuAction(MenuAction::LoadBuiltinPalette(
                crate::core::palette::BuiltinPaletteKind::Ross
            ))]
        );
    }

    #[test]
    fn file_dropdown_opens_below_the_bar_and_closes_on_outside_click() {
        let ctx = egui::Context::default();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let layout = split_frame(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0)));
        let mut host = TopBarHost::new();
        let file_title = menu_title_rects(&layout)[0].1.center();
        click_host(&ctx, &mut host, &layout, &colors, file_title);
        assert_eq!(host.open_menu(), Some(0), "clicking File opens its dropdown");
        // Dropdown geometry hangs from the menu slice and, with several items,
        // extends BELOW the whole top bar into the body.
        let dropdown = menu_dropdown_rect(&layout, 0).expect("File dropdown");
        assert!(dropdown.top() >= layout.menu_rect.bottom() - 1.0);
        assert!(
            dropdown.bottom() > layout.top_rect.bottom(),
            "the File dropdown must extend below the bar: {dropdown:?}"
        );
        // Click far outside (canvas area) -> closes; no menu action emitted.
        click_host(&ctx, &mut host, &layout, &colors, egui::pos2(400.0, 400.0));
        assert_eq!(host.open_menu(), None, "an outside click must close the menu");
        assert!(
            !host
                .drain_events()
                .iter()
                .any(|e| matches!(e, TopBarEvent::MenuAction(_))),
            "an outside click must not emit a menu action"
        );
    }

    #[test]
    fn tab_close_button_is_right_aligned_and_inside_the_tab() {
        let tab = Rect::from_min_size(pos2(4.0, 100.0), vec2(TAB_WIDTH, 17.2));
        let close = tab_close_button_rect(tab);
        assert!((close.width() - close.height()).abs() < 1e-4, "square: {close:?}");
        assert!(tab.contains_rect(close), "close button inside tab");
        assert!(
            tab.right() - close.right() > 0.0,
            "close button inset from the tab's right edge: {close:?}"
        );
        assert!(
            close.center().x > tab.center().x,
            "close button sits in the RIGHT half of the tab"
        );
    }

    #[test]
    fn middle_create_button_emits_open_create_document() {
        let ctx = egui::Context::default();
        let mut host = TopBarHost::new();
        let colors = crate::ui::theme::Theme::default_dark().colors;
        let frame = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let layout = split_frame(frame);
        let at = create_document_button_rect(&layout).center();

        let run = |host: &mut TopBarHost, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(frame),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| host.ui(ui, &layout, &colors));
            });
            output.textures_delta.clear();
        };
        run(&mut host, vec![egui::Event::PointerMoved(at)]);
        run(
            &mut host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        run(
            &mut host,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );

        assert!(
            host.drain_events().contains(&TopBarEvent::OpenCreateDocument),
            "clicking the middle square must emit OpenCreateDocument"
        );
    }

    #[test]
    fn top_bar_and_body_tile_the_frame_exactly() {
        let frame = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let layout = split_frame(frame);
        // Top bar spans the full width, fixed height, at the very top.
        assert_eq!(layout.top_rect.min, frame.min);
        assert_eq!(layout.top_rect.width(), frame.width());
        assert_eq!(layout.top_rect.height(), TOP_BAR_HEIGHT);
        // Body starts right below the bar and reaches the frame bottom.
        assert_eq!(layout.body_rect.top(), frame.top() + TOP_BAR_HEIGHT);
        assert_eq!(layout.body_rect.bottom(), frame.bottom());
        assert_eq!(layout.body_rect.width(), frame.width());
        // No gap and no overlap between the two halves (they only touch).
        assert_eq!(layout.top_rect.bottom(), layout.body_rect.top());
        let overlap = layout.top_rect.intersect(layout.body_rect);
        assert!(
            overlap.width() <= 0.0 || overlap.height() <= 0.0,
            "the top bar and body must not overlap in area: {overlap:?}"
        );
        // The three slices tile the top bar exactly.
        assert_eq!(layout.menu_rect.max.y, layout.middle_rect.min.y);
        assert_eq!(layout.middle_rect.max.y, layout.tabs_rect.min.y);
        assert_eq!(layout.menu_rect.min.y, layout.top_rect.min.y);
        assert_eq!(layout.tabs_rect.max.y, layout.top_rect.max.y);
        assert_eq!(layout.menu_rect.height(), 21.0);
        assert!((layout.middle_rect.height() - 56.7).abs() < 1e-4);
        assert!((layout.tabs_rect.height() - 25.2).abs() < 1e-4);
    }

    #[test]
    fn split_frame_respects_an_offset_frame() {
        let frame = Rect::from_min_size(pos2(10.0, 20.0), vec2(320.0, 240.0));
        let layout = split_frame(frame);
        assert_eq!(layout.top_rect.min, pos2(10.0, 20.0));
        assert_eq!(layout.body_rect.top(), 20.0 + TOP_BAR_HEIGHT);
        assert_eq!(layout.body_rect.left(), 10.0);
    }
}
