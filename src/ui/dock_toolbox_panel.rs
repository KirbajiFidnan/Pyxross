//! Toolbox dock panel: categorized static nest listing the application tools.
//!
//! Tools are grouped into three labeled sections separated by subtle hairline
//! dividers:
//! - **Draw**: Pencil, Eraser, Fill, Eyedropper
//! - **Selection**: Rectangle, Wand, Lasso, Brush Selector (all Fieldier child
//!   tools exposed directly; the parent Fieldier tool has no standalone button)
//! - **Misc**: Tile, Animation, Text

use egui::vec2;

use crate::input::{FieldierChild, Tool};
use crate::ui::dock_hosts::{ToolboxHost, ToolboxShortcuts, ToolboxView};
use crate::ui::icons;
use crate::ui::panel_dock::nest::NestView;
use crate::ui::panel_dock::{
    ButtonStyle, Component, NestContent, NestMode, PanelChrome, PanelId, PanelMetadata,
    PanelPlacement, PanelSpec,
};
use crate::ui::toolbar::{tool_label, ToolbarEvent};

/// The dock id for the Toolbox panel.
pub const TOOLBOX_PANEL_ID: u64 = 100;

/// Button side and spacing for the multi-column tool grid.
const TOOL_BUTTON_SIZE: f32 = 32.0;
const TOOL_GRID_GAP: f32 = 4.0;
const CATEGORY_HEADER_HEIGHT: f32 = 16.0;
const CATEGORY_HEADER_GAP: f32 = 4.0;
const CATEGORY_DIVIDER_GAP: f32 = 6.0;

/// One clickable button in the categorized Toolbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolboxEntry {
    Tool(Tool),
    Selection(FieldierChild),
}

/// One labeled section in the Toolbox.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ToolboxCategory {
    pub title: &'static str,
    pub entries: &'static [ToolboxEntry],
}

const DRAW_ENTRIES: &[ToolboxEntry] = &[
    ToolboxEntry::Tool(Tool::Pencil),
    ToolboxEntry::Tool(Tool::Eraser),
    ToolboxEntry::Tool(Tool::Fill),
    ToolboxEntry::Tool(Tool::Eyedropper),
];

const SELECTION_ENTRIES: &[ToolboxEntry] = &[
    ToolboxEntry::Selection(FieldierChild::Rectangle),
    ToolboxEntry::Selection(FieldierChild::Wand),
    ToolboxEntry::Selection(FieldierChild::Lasso),
    ToolboxEntry::Selection(FieldierChild::BrushSelector),
];

const MISC_ENTRIES: &[ToolboxEntry] = &[
    ToolboxEntry::Tool(Tool::Tile),
    ToolboxEntry::Tool(Tool::AnimationPlacer),
    ToolboxEntry::Tool(Tool::Text),
];

/// Ordered Toolbox sections rendered from top to bottom.
pub(crate) const TOOLBOX_CATEGORIES: &[ToolboxCategory] = &[
    ToolboxCategory {
        title: "Draw",
        entries: DRAW_ENTRIES,
    },
    ToolboxCategory {
        title: "Selection",
        entries: SELECTION_ENTRIES,
    },
    ToolboxCategory {
        title: "Misc",
        entries: MISC_ENTRIES,
    },
];

fn entry_icon(entry: ToolboxEntry) -> &'static str {
    match entry {
        ToolboxEntry::Tool(Tool::Pencil | Tool::Draw) => icons::PENCIL,
        ToolboxEntry::Tool(Tool::Eraser) => icons::ERASER,
        ToolboxEntry::Tool(Tool::Fill) => icons::FILL,
        ToolboxEntry::Tool(Tool::Eyedropper) => icons::EYEDROPPER,
        ToolboxEntry::Tool(Tool::Fieldier) => icons::SELECTION,
        ToolboxEntry::Tool(Tool::Tile) => icons::TILE,
        ToolboxEntry::Tool(Tool::AnimationPlacer) => icons::ANIMATION,
        ToolboxEntry::Tool(Tool::Text) => icons::TEXT,
        ToolboxEntry::Selection(FieldierChild::Rectangle) => icons::SELECT_RECT,
        ToolboxEntry::Selection(FieldierChild::Wand) => icons::SELECT_WAND,
        ToolboxEntry::Selection(FieldierChild::Lasso) => icons::SELECT_LASSO,
        ToolboxEntry::Selection(FieldierChild::BrushSelector) => icons::SELECT_BRUSH,
    }
}

fn entry_tooltip(entry: ToolboxEntry, shortcuts: &ToolboxShortcuts) -> String {
    match entry {
        ToolboxEntry::Tool(tool) => {
            let name = tool_label(tool);
            match tool {
                Tool::Pencil => format!("{name} ({})", shortcuts.pencil),
                Tool::Eraser => format!("{name} ({})", shortcuts.eraser),
                Tool::Fill => format!("{name} ({})", shortcuts.fill),
                Tool::Eyedropper => format!("{name} (Right-Click)"),
                Tool::Tile => format!("{name} ({})", shortcuts.tile),
                Tool::AnimationPlacer => format!("{name} ({})", shortcuts.animation_placer),
                Tool::Text => format!("{name} ({})", shortcuts.text),
                Tool::Fieldier | Tool::Draw => name.to_string(),
            }
        }
        ToolboxEntry::Selection(FieldierChild::Rectangle) => {
            format!("Rectangle Select ({})", shortcuts.rectangle)
        }
        ToolboxEntry::Selection(FieldierChild::Wand) => {
            format!("Magic Wand ({})", shortcuts.wand)
        }
        ToolboxEntry::Selection(FieldierChild::Lasso) => {
            format!("Lasso Select ({})", shortcuts.lasso)
        }
        ToolboxEntry::Selection(FieldierChild::BrushSelector) => {
            format!("Brush Selector ({})", shortcuts.brush_selector)
        }
    }
}

fn entry_is_active(entry: ToolboxEntry, view: &ToolboxView) -> bool {
    match entry {
        ToolboxEntry::Tool(tool) => view.tool == tool,
        ToolboxEntry::Selection(child) => view.tool == Tool::Fieldier && view.child == child,
    }
}

fn entry_click_event(entry: ToolboxEntry) -> ToolbarEvent {
    match entry {
        ToolboxEntry::Tool(tool) => ToolbarEvent::ToolSelected(tool),
        ToolboxEntry::Selection(child) => ToolbarEvent::FieldierChildChanged(child),
    }
}

/// Categorized tool grid: draws subtle section headers, hairline dividers and
/// square icon buttons; emits [`ToolbarEvent::ToolSelected`] or
/// [`ToolbarEvent::FieldierChildChanged`] on click.
struct ToolboxComponent {
    host: ToolboxHost,
}

impl Component for ToolboxComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow().clone();
        let avail_w = ui.available_width().max(TOOL_BUTTON_SIZE);
        let cols = (((avail_w + TOOL_GRID_GAP) / (TOOL_BUTTON_SIZE + TOOL_GRID_GAP)).floor()
            as usize)
            .max(1);

        let mut total_h = 0.0f32;
        for (cat_idx, cat) in TOOLBOX_CATEGORIES.iter().enumerate() {
            if cat_idx > 0 {
                total_h += CATEGORY_DIVIDER_GAP * 2.0 + 1.0;
            }
            let rows = (cat.entries.len() + cols - 1) / cols;
            let grid_h = rows as f32 * TOOL_BUTTON_SIZE
                + (rows.saturating_sub(1)) as f32 * TOOL_GRID_GAP;
            total_h += CATEGORY_HEADER_HEIGHT + CATEGORY_HEADER_GAP + grid_h;
        }
        ui.set_min_size(vec2(avail_w, total_h));

        let header_color = ui.visuals().weak_text_color();
        let divider_color = chrome.colors.panel_border32().linear_multiply(0.55);

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(TOOL_GRID_GAP, 0.0);
            for (cat_idx, cat) in TOOLBOX_CATEGORIES.iter().enumerate() {
                if cat_idx > 0 {
                    ui.add_space(CATEGORY_DIVIDER_GAP);
                    let (div_rect, _) =
                        ui.allocate_exact_size(vec2(avail_w, 1.0), egui::Sense::hover());
                    ui.painter().line_segment(
                        [div_rect.left_center(), div_rect.right_center()],
                        egui::Stroke::new(1.0, divider_color),
                    );
                    ui.add_space(CATEGORY_DIVIDER_GAP);
                }

                let (hdr_rect, _) = ui.allocate_exact_size(
                    vec2(avail_w, CATEGORY_HEADER_HEIGHT),
                    egui::Sense::hover(),
                );
                ui.painter().text(
                    hdr_rect.left_center(),
                    egui::Align2::LEFT_CENTER,
                    cat.title,
                    egui::FontId::proportional(11.0),
                    header_color,
                );
                ui.add_space(CATEGORY_HEADER_GAP);

                for (row_idx, chunk) in cat.entries.chunks(cols).enumerate() {
                    if row_idx > 0 {
                        ui.add_space(TOOL_GRID_GAP);
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = vec2(TOOL_GRID_GAP, TOOL_GRID_GAP);
                        for &entry in chunk {
                            let active = entry_is_active(entry, &view);
                            let response = ui
                                .allocate_ui(vec2(TOOL_BUTTON_SIZE, TOOL_BUTTON_SIZE), |ui| {
                                    let icon = entry_icon(entry);
                                    chrome
                                        .button_with_font_size(
                                            ui,
                                            icon,
                                            Some(18.0),
                                            ButtonStyle::toggled(active),
                                        )
                                        .on_hover_text(entry_tooltip(entry, &view.shortcuts))
                                })
                                .inner;
                            if response.clicked() {
                                self.host
                                    .events
                                    .borrow_mut()
                                    .push(entry_click_event(entry));
                            }
                        }
                    });
                }
            }
        });
    }
}

/// Static nest listing the categorized tools; the buttons live in the scrollable content.
pub(crate) fn build_toolbox_nest(host: &ToolboxHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ToolboxComponent { host: host.clone() });
    nest
}

/// Dock spec for the Toolbox panel (dock id `PanelId::new(100)`).
pub fn toolbox_panel_spec(host: &ToolboxHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(TOOLBOX_PANEL_ID),
        metadata: PanelMetadata::new("Toolbox", false, vec2(160.0, 120.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(40.0, 80.0), vec2(200.0, 360.0)),
        content: Box::new(build_toolbox_nest(host)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbox_categories_expose_fielder_children_and_omit_parent_fielder() {
        let titles: Vec<&str> = TOOLBOX_CATEGORIES.iter().map(|c| c.title).collect();
        assert_eq!(titles, vec!["Draw", "Selection", "Misc"]);

        let all_entries: Vec<ToolboxEntry> = TOOLBOX_CATEGORIES
            .iter()
            .flat_map(|c| c.entries.iter().copied())
            .collect();

        // Parent Fieldier must NOT appear as a button.
        assert!(
            !all_entries.contains(&ToolboxEntry::Tool(Tool::Fieldier)),
            "parent Tool::Fieldier must not have a standalone Toolbox button"
        );

        // Every Fieldier child must appear in the Selection category.
        let sel = TOOLBOX_CATEGORIES[1].entries;
        assert!(sel.contains(&ToolboxEntry::Selection(FieldierChild::Rectangle)));
        assert!(sel.contains(&ToolboxEntry::Selection(FieldierChild::Wand)));
        assert!(sel.contains(&ToolboxEntry::Selection(FieldierChild::Lasso)));
        assert!(sel.contains(&ToolboxEntry::Selection(FieldierChild::BrushSelector)));

        // Clicking a Selection entry emits FieldierChildChanged.
        assert_eq!(
            entry_click_event(ToolboxEntry::Selection(FieldierChild::BrushSelector)),
            ToolbarEvent::FieldierChildChanged(FieldierChild::BrushSelector)
        );
    }
}
