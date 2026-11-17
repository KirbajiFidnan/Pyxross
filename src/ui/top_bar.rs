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
    ExportSpriteSheet,
    ExportGif,
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
}

/// One selectable entry in a menu dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: &'static str,
    pub action: MenuAction,
}

/// A menu-bar title plus its dropdown. `dropdown_title` is an optional centered
/// header line (the File menu shows "document").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuDef {
    pub title: &'static str,
    pub dropdown_title: Option<&'static str>,
    pub items: &'static [MenuItem],
}

/// The centered header shown at the top of the File dropdown.
pub const FILE_MENU_TITLE: &str = "document";

static FILE_ITEMS: [MenuItem; 9] = [
    MenuItem {
        label: "new project",
        action: MenuAction::NewProject,
    },
    MenuItem {
        label: "save project",
        action: MenuAction::SaveProject,
    },
    MenuItem {
        label: "save project as",
        action: MenuAction::SaveProjectAs,
    },
    MenuItem {
        label: "open project",
        action: MenuAction::OpenProject,
    },
    MenuItem {
        label: "import images",
        action: MenuAction::ImportImages,
    },
    MenuItem {
        label: "import gif",
        action: MenuAction::ImportGif,
    },
    MenuItem {
        label: "export sprite sheet",
        action: MenuAction::ExportSpriteSheet,
    },
    MenuItem {
        label: "export gif",
        action: MenuAction::ExportGif,
    },
    MenuItem {
        label: "export selection",
        action: MenuAction::ExportSelected,
    },
];

static EDIT_ITEMS: [MenuItem; 12] = [
    MenuItem {
        label: "undo (Ctrl+Z)",
        action: MenuAction::Undo,
    },
    MenuItem {
        label: "redo (Ctrl+Y)",
        action: MenuAction::Redo,
    },
    MenuItem {
        label: "cut (Ctrl+X)",
        action: MenuAction::Cut,
    },
    MenuItem {
        label: "copy (Ctrl+C)",
        action: MenuAction::Copy,
    },
    MenuItem {
        label: "paste (Ctrl+V)",
        action: MenuAction::Paste,
    },
    MenuItem {
        label: "delete (Del)",
        action: MenuAction::DeleteSelection,
    },
    MenuItem {
        label: "select all",
        action: MenuAction::SelectAll,
    },
    MenuItem {
        label: "deselect (Esc)",
        action: MenuAction::Deselect,
    },
    MenuItem {
        label: "invert selection (Ctrl+I)",
        action: MenuAction::InvertSelection,
    },
    MenuItem {
        label: "flip horizontal (Ctrl+F)",
        action: MenuAction::FlipSelectionHorizontal,
    },
    MenuItem {
        label: "flip vertical (Shift+F)",
        action: MenuAction::FlipSelectionVertical,
    },
    MenuItem {
        label: "rotate 90° (Alt+F)",
        action: MenuAction::RotateSelection,
    },
];

static CANVAS_ITEMS: [MenuItem; 1] = [MenuItem {
    label: "resize canvas",
    action: MenuAction::ResizeCanvas,
}];

static MENUS: [MenuDef; 3] = [
    MenuDef {
        title: "File",
        dropdown_title: Some(FILE_MENU_TITLE),
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

/// The square "create new document" button in the middle slice: a square whose
/// side length approximates the (grown) middle slice height, set a small fixed inset
/// from the slice's LEFT edge. Pure, so tests can target it.
pub fn create_document_button_rect(layout: &TopBarLayout) -> Rect {
    let side = (layout.middle_rect.height() - 2.0 * SLICE_INSET).max(8.0);
    let inset = CREATE_BUTTON_LEFT_INSET.max(SLICE_INSET);
    let center = pos2(
        layout.middle_rect.left() + inset + side * 0.5,
        layout.middle_rect.center().y,
    );
    Rect::from_center_size(center, vec2(side, side))
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
const MENU_DROPDOWN_WIDTH: f32 = 204.0;
const MENU_ITEM_HEIGHT: f32 = 22.0;
const MENU_DROPDOWN_PAD: f32 = 4.0;
const MENU_HEADER_HEIGHT: f32 = 20.0;

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
    let header = if def.dropdown_title.is_some() {
        MENU_HEADER_HEIGHT
    } else {
        0.0
    };
    let height = header + def.items.len() as f32 * MENU_ITEM_HEIGHT + 2.0 * MENU_DROPDOWN_PAD;
    Some(Rect::from_min_size(
        pos2(title.left(), layout.menu_rect.bottom() + 1.0),
        vec2(MENU_DROPDOWN_WIDTH, height),
    ))
}

/// The clickable rects of a dropdown's items, in order. Pure.
pub fn menu_item_rects(layout: &TopBarLayout, index: usize) -> Vec<(MenuItem, Rect)> {
    let def = match menu_bar().get(index) {
        Some(def) => def,
        None => return Vec::new(),
    };
    let dropdown = match menu_dropdown_rect(layout, index) {
        Some(rect) => rect,
        None => return Vec::new(),
    };
    let header = if def.dropdown_title.is_some() {
        MENU_HEADER_HEIGHT
    } else {
        0.0
    };
    def.items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let top = dropdown.top() + MENU_DROPDOWN_PAD + header + i as f32 * MENU_ITEM_HEIGHT;
            let rect = Rect::from_min_max(
                pos2(dropdown.left() + MENU_DROPDOWN_PAD, top),
                pos2(dropdown.right() - MENU_DROPDOWN_PAD, top + MENU_ITEM_HEIGHT),
            );
            (*item, rect)
        })
        .collect()
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
}

impl TopBarHost {
    pub fn new() -> Self {
        Self::default()
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

        // Middle slice: mostly an empty placeholder for future multi-purpose
        // controls, with the square "create new document" button pinned to the
        // right edge. Pressing it asks the App to open the create-document panel.
        let button_rect = create_document_button_rect(layout);
        let button = ui.interact(
            button_rect,
            Id::new("top-bar-create-document"),
            Sense::click(),
        );
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
        painter.text(
            button_rect.center(),
            Align2::CENTER_CENTER,
            "+",
            FontId::proportional((button_rect.height() * 0.7).clamp(12.0, 30.0)),
            text_color,
        );
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
        // Panel background + border.
        painter.rect_filled(dropdown, 3.0, colors.panel_header_bg32());
        painter.rect_stroke(
            dropdown,
            3.0,
            Stroke::new(1.0, colors.panel_border32()),
            StrokeKind::Inside,
        );
        // Optional centered header (File -> "document").
        if let Some(header) = def.dropdown_title {
            let header_rect = Rect::from_min_max(
                dropdown.min,
                pos2(dropdown.right(), dropdown.top() + MENU_DROPDOWN_PAD + MENU_HEADER_HEIGHT),
            );
            painter.text(
                header_rect.center(),
                Align2::CENTER_CENTER,
                header,
                FontId::proportional(12.0),
                colors.panel_border32(),
            );
        }
        let mut chosen: Option<MenuAction> = None;
        for (item, rect) in menu_item_rects(layout, index) {
            let response = ui.interact(
                rect,
                Id::new(("top-bar-menu-item", index, item.label)),
                Sense::click(),
            );
            if response.hovered() {
                painter.rect_filled(rect, 2.0, colors.selection_bg_fill32());
            }
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                item.label,
                FontId::proportional(13.0),
                text_color,
            );
            if response.clicked() {
                chosen = Some(item.action);
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
        let y = layout.tabs_rect.top() + SLICE_INSET;
        let height = (layout.tabs_rect.height() - 2.0 * SLICE_INSET).max(1.0);
        let text_color = ui.visuals().text_color();
        let mut x = layout.tabs_rect.left() + SLICE_INSET;
        for tab in &tabs {
            let rect = Rect::from_min_size(pos2(x, y), vec2(TAB_WIDTH, height));
            if rect.right() > layout.tabs_rect.right() - SLICE_INSET {
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
            // Dirty marker: a trailing bullet (no hard-coded color; text only).
            let label = if tab.dirty {
                format!("{} •", tab.name)
            } else {
                tab.name.clone()
            };
            // The label is centred in the space LEFT of the close button so the
            // "×" never overlaps it.
            let close_rect = tab_close_button_rect(rect);
            let label_center = pos2(
                (rect.left() + close_rect.left()) * 0.5,
                rect.center().y,
            );
            painter.text(
                label_center,
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(13.0),
                text_color,
            );
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
            "button should sit near the left edge, not a third over: {button:?}"
        );
        assert!(
            layout.middle_rect.right() - button.right() > 0.0,
            "button must be clear of the right edge: {button:?}"
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
        assert_eq!(menus.len(), 3, "three menu titles");
        assert_eq!(menus[0].title, "File");
        assert_eq!(menus[1].title, "Edit");
        assert_eq!(menus[2].title, "Canvas");
        assert_eq!(
            menus[0].dropdown_title,
            Some("document"),
            "the File dropdown carries the centered 'document' header"
        );
        assert_eq!(menus[0].items.len(), 9);
        assert_eq!(menus[1].items.len(), 12);
        assert_eq!(menus[2].items.len(), 1);
        assert_eq!(menus[2].items[0].label, "resize canvas");
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
            MenuAction::ExportSpriteSheet,
            MenuAction::ExportGif,
            MenuAction::ExportSelected,
        ];
        let items = menu_item_rects(&layout, 0);
        assert_eq!(items.len(), 9);
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
        assert_eq!(items[0].0.label, "resize canvas");

        click_host(&ctx, &mut host, &layout, &colors, canvas_title);
        click_host(&ctx, &mut host, &layout, &colors, items[0].1.center());
        assert_eq!(
            host.drain_events(),
            vec![TopBarEvent::MenuAction(MenuAction::ResizeCanvas)]
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
