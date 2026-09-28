//! Palette dock panel (id 105): the active palette's swatches with pinned
//! add/remove actions, used to pick the primary and secondary drawing colors.
//!
//! A static nest holds two components: the wrapping swatch grid (content) and
//! the pinned action header (overlay). The header never scrolls; the swatches
//! scroll under it. Every gesture is emitted as a [`PalettePanelEvent`] for the
//! App shell to apply — the panel never mutates the model.

use egui::vec2;

use crate::core::color::Color;
use crate::ui::dock_hosts::{PalettePanelHost, PaletteView};
use crate::ui::panel_dock::nest::{Component, ComponentPlacement, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// The dock id for the Palette panel.
pub const PALETTE_PANEL_ID: u64 = 105;

/// Edge length of one palette swatch, in points.
const SWATCH_SIZE: f32 = 20.0;

/// Gap between swatches, in points.
const SWATCH_GAP: f32 = 4.0;

/// Corner radius of a swatch, in points.
const SWATCH_CORNER: f32 = 1.0;

/// Height of the pinned header band (matches the dock header height).
const HEADER_HEIGHT: f32 = 28.0;

/// Width of the marker border around the primary-matching swatch, in points.
const PRIMARY_MARKER_WIDTH: f32 = 2.0;

/// Inset of the inner marker border around the secondary-matching swatch.
const SECONDARY_MARKER_INSET: f32 = 2.5;

/// Width of the secondary marker border, in points.
const SECONDARY_MARKER_WIDTH: f32 = 1.5;

/// A palette gesture emitted by the Palette panel.
///
/// The App shell owns the palette and the two drawing colors; this pure view
/// only ever pushes these events into the host cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PalettePanelEvent {
    /// Append the current primary color to the active palette.
    Add,
    /// Remove the selected entry from the active palette.
    Remove(usize),
    /// Set the primary drawing color.
    Primary(Color),
    /// Set the secondary drawing color.
    Secondary(Color),
}

/// The number of swatch columns the grid fits into `width` points.
fn columns_for_width(width: f32) -> usize {
    (((width + SWATCH_GAP) / (SWATCH_SIZE + SWATCH_GAP)).floor() as usize).max(1)
}

/// Converts a core color to an egui color.
fn to_color32(color: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// Pinned action header: Add/Remove over the scrolling swatch grid.
///
/// An overlay component, so it stays fixed while the swatch grid scrolls.
struct PaletteHeaderComponent {
    host: PalettePanelHost,
}

impl Component for PaletteHeaderComponent {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let entry_count = self.host.view.borrow().entries.len();
        let selected = *self.host.selected.borrow();
        ui.horizontal(|ui| {
            if chrome
                .button(ui, "Add color", ButtonStyle::plain())
                .on_hover_text("Append the primary color to the active palette")
                .clicked()
            {
                self.host.events.borrow_mut().push(PalettePanelEvent::Add);
            }
            let remove_enabled =
                entry_count > 1 && selected.is_some_and(|index| index < entry_count);
            let remove = chrome
                .button(
                    ui,
                    "Remove color",
                    ButtonStyle {
                        enabled: remove_enabled,
                        ..ButtonStyle::plain()
                    },
                )
                .on_hover_text("Remove the selected palette entry");
            if remove.clicked() {
                if let Some(index) = selected {
                    self.host
                        .events
                        .borrow_mut()
                        .push(PalettePanelEvent::Remove(index));
                }
            }
        });
    }
}

/// One swatch: filled with its palette color and marked when it matches the
/// primary (outer border) or the secondary (inner border) drawing color.
fn swatch_ui(
    ui: &mut egui::Ui,
    host: &PalettePanelHost,
    index: usize,
    view: &PaletteView,
    chrome: &PanelChrome,
) {
    let color = view.entries[index];
    let (rect, _) = ui.allocate_exact_size(vec2(SWATCH_SIZE, SWATCH_SIZE), egui::Sense::hover());
    let response = ui.interact(
        rect,
        egui::Id::new(("palette-swatch", index)),
        egui::Sense::click(),
    );
    ui.painter()
        .rect_filled(rect, SWATCH_CORNER, to_color32(color));
    ui.painter().rect_stroke(
        rect,
        SWATCH_CORNER,
        egui::Stroke::new(1.0, chrome.colors.panel_border32()),
        egui::StrokeKind::Inside,
    );
    if color == view.secondary {
        ui.painter().rect_stroke(
            rect.shrink(SECONDARY_MARKER_INSET),
            SWATCH_CORNER,
            egui::Stroke::new(
                SECONDARY_MARKER_WIDTH,
                chrome.colors.selection_stroke_color32(),
            ),
            egui::StrokeKind::Inside,
        );
    }
    if color == view.primary {
        ui.painter().rect_stroke(
            rect,
            SWATCH_CORNER,
            egui::Stroke::new(
                PRIMARY_MARKER_WIDTH,
                chrome.colors.selection_stroke_color32(),
            ),
            egui::StrokeKind::Inside,
        );
    }
    if response.clicked() {
        *host.selected.borrow_mut() = Some(index);
        host.events
            .borrow_mut()
            .push(PalettePanelEvent::Primary(color));
    }
    if response.secondary_clicked() {
        *host.selected.borrow_mut() = Some(index);
        host.events
            .borrow_mut()
            .push(PalettePanelEvent::Secondary(color));
    }
}

/// Scrollable palette grid: wrapping swatch rows that follow the panel width.
struct PaletteSwatchesComponent {
    host: PalettePanelHost,
}

impl Component for PaletteSwatchesComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow();
        let width = ui.available_width();
        let columns = columns_for_width(width);
        let rows = view.entries.len().div_ceil(columns);
        let content_height = HEADER_HEIGHT + rows as f32 * (SWATCH_SIZE + SWATCH_GAP);
        ui.set_min_size(vec2(width, content_height));
        ui.add_space(HEADER_HEIGHT);
        for (row_index, row) in view.entries.chunks(columns).enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = SWATCH_GAP;
                for column in 0..row.len() {
                    swatch_ui(ui, &self.host, row_index * columns + column, &view, chrome);
                }
            });
        }
    }
}

/// Panel content: the static nest holding the swatches and the pinned header.
struct PalettePanelContent {
    nest: NestContent,
}

impl PanelContent for PalettePanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the swatch grid (content) and the pinned header (overlay).
pub(crate) fn build_palette_nest(host: &PalettePanelHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(PaletteSwatchesComponent { host: host.clone() })
        .push(PaletteHeaderComponent { host: host.clone() });
    nest
}

/// Dock spec for the Palette panel (dock id `PanelId::new(105)`).
///
/// Dockable and floatable like the other dockable panels; the App docks it
/// right by default.
pub fn palette_panel_spec(host: &PalettePanelHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(PALETTE_PANEL_ID),
        metadata: PanelMetadata::new("Palette", true, vec2(200.0, 140.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(120.0, 120.0), vec2(240.0, 300.0)),
        content: Box::new(PalettePanelContent {
            nest: build_palette_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_for_width_fits_at_least_one_swatch() {
        assert_eq!(columns_for_width(0.0), 1);
        assert_eq!(columns_for_width(SWATCH_SIZE), 1);
        assert_eq!(columns_for_width(SWATCH_SIZE + SWATCH_GAP), 1);
        assert_eq!(columns_for_width(SWATCH_SIZE + SWATCH_GAP + SWATCH_SIZE), 2);
        assert_eq!(columns_for_width(2.0 * SWATCH_SIZE + 3.0 * SWATCH_GAP), 2);
    }
}
