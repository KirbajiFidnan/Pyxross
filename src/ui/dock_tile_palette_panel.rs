//! Tile Palette dock panel (id 106): the project's tile palette with a
//! click-to-select thumbnail grid and pinned actions (add empty, delete,
//! reorder).
//!
//! **DOCK-ONLY.** The panel is pinned in the right dock and never floats: its
//! metadata is `can_pop_out == false` (and `can_native_pop_out == false`, which
//! `PanelMetadata::new` already defaults), so its header exposes neither Float
//! nor Pop-out.
//!
//! A static nest holds two components: the wrapping thumbnail grid (content)
//! and the pinned action header (overlay). The header never scrolls; the grid
//! scrolls under it. Every gesture is emitted as a [`TilePalettePanelEvent`] for
//! the App shell to apply — the panel is a pure view over the host cells and
//! never mutates the model.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use egui::vec2;

use crate::core::tilemap::{Tile, TileId, TilePixelOverrides};
use crate::ui::panel_dock::nest::{Component, ComponentPlacement, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// The dock id for the Tile Palette panel.
pub const TILE_PALETTE_PANEL_ID: u64 = 106;

/// Height of the pinned header band (matches the dock header height).
const HEADER_HEIGHT: f32 = 28.0;

/// Gap between thumbnails, in points.
const THUMB_GAP: f32 = 6.0;

/// Maximum thumbnail dimension (width or height) in points.
const THUMB_MAX: f32 = 72.0;

/// Maximum pixel-scale factor when upscaling a thumbnail.
const THUMB_MAX_SCALE: f32 = 8.0;

/// Corner radius of a thumbnail.
const THUMB_CORNER: f32 = 2.0;

/// Width of the selected-thumbnail highlight border, in points.
const SELECTED_MARKER_WIDTH: f32 = 2.0;

/// A tile-palette gesture emitted by the Tile Palette panel.
///
/// The App shell owns the project's `tile_palette` and tool state; this pure
/// view only ever pushes these events into the host cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TilePalettePanelEvent {
    /// Make the tile with this id the selected placement source and switch to
    /// the Tile tool.
    Select(TileId),
    /// Append a new EMPTY tile (one tile cell, `tile_size.w` × `tile_size.h`,
    /// transparent) and select it.
    AddEmpty,
    /// Remove the tile with this id.
    Delete(TileId),
    /// Move the selected tile one slot: `+1` up, `-1` down (deterministic
    /// reorder; the palette keeps insertion order).
    MoveSelected(i32),
}

/// Snapshot of the Tile Palette panel state for one frame.
///
/// `tiles` is the ACTIVE project's tile palette (in insertion order);
/// `selected` is the chosen tile id, or `None`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TilePaletteView {
    pub tiles: Vec<Tile>,
    pub selected: Option<TileId>,
    /// Read-only per-frame tile pixels derived from an in-flight canvas stroke.
    /// The committed palette tiles above are never changed for this preview.
    pub pixel_overrides: TilePixelOverrides,
}

/// Shared cells for the Tile Palette panel.
#[derive(Clone, Default)]
pub struct TilePaletteHost {
    pub view: Rc<RefCell<TilePaletteView>>,
    pub events: Rc<RefCell<Vec<TilePalettePanelEvent>>>,
}

/// Pinned action header: Add (empty), Delete and reorder buttons over the
/// scrolling thumbnail grid.
///
/// An overlay component, so it stays fixed while the grid scrolls.
struct TilePaletteHeaderComponent {
    host: TilePaletteHost,
}

impl Component for TilePaletteHeaderComponent {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow();
        let selected = view.selected;
        let selected_index = selected.and_then(|id| view.tiles.iter().position(|t| t.id == id));
        let count = view.tiles.len();
        ui.horizontal(|ui| {
            if chrome
                .button(ui, "+ Empty", ButtonStyle::plain())
                .on_hover_text("Add a new empty tile")
                .clicked()
            {
                self.host
                    .events
                    .borrow_mut()
                    .push(TilePalettePanelEvent::AddEmpty);
            }
            let delete = chrome
                .button(
                    ui,
                    "− Delete",
                    ButtonStyle {
                        enabled: selected.is_some(),
                        ..ButtonStyle::plain()
                    },
                )
                .on_hover_text("Delete the selected tile");
            if delete.clicked() {
                if let Some(id) = selected {
                    self.host
                        .events
                        .borrow_mut()
                        .push(TilePalettePanelEvent::Delete(id));
                }
            }
            let move_up = chrome
                .button(
                    ui,
                    "↑",
                    ButtonStyle {
                        enabled: selected_index.is_some_and(|index| index > 0),
                        ..ButtonStyle::plain()
                    },
                )
                .on_hover_text("Move the selected tile up");
            if move_up.clicked() {
                self.host
                    .events
                    .borrow_mut()
                    .push(TilePalettePanelEvent::MoveSelected(1));
            }
            let move_down = chrome
                .button(
                    ui,
                    "↓",
                    ButtonStyle {
                        enabled: selected_index.is_some_and(|index| index + 1 < count),
                        ..ButtonStyle::plain()
                    },
                )
                .on_hover_text("Move the selected tile down");
            if move_down.clicked() {
                self.host
                    .events
                    .borrow_mut()
                    .push(TilePalettePanelEvent::MoveSelected(-1));
            }
        });
    }
}

/// The pixel scale for a thumbnail so it fits within [`THUMB_MAX`] points.
fn thumb_scale(tile: &Tile) -> f32 {
    let w = tile.w.max(1) as f32;
    let h = tile.h.max(1) as f32;
    (THUMB_MAX / w)
        .min(THUMB_MAX / h)
        .min(THUMB_MAX_SCALE)
        .max(1.0)
}

/// The thumbnail pixel is the transient stroke override when one exists;
/// otherwise it remains the committed palette byte. Keeping this lookup in one
/// helper makes the live-preview fallback explicit and easy to test.
fn thumbnail_pixel_rgba(
    tile: &Tile,
    pixel_overrides: &TilePixelOverrides,
    x: u32,
    y: u32,
) -> [u8; 4] {
    let i = (y as usize * tile.w as usize + x as usize) * 4;
    pixel_overrides.get(tile.id, x, y).unwrap_or([
        tile.pixels[i],
        tile.pixels[i + 1],
        tile.pixels[i + 2],
        tile.pixels[i + 3],
    ])
}

const INLINE_RECT_PIXEL_LIMIT: usize = 64;

fn hash_tile_pixels(tile: &Tile, pixel_overrides: &TilePixelOverrides) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mix = |h: &mut u64, byte: u8| {
        *h ^= byte as u64;
        *h = h.wrapping_mul(0x100000001b3);
    };
    for b in tile.w.to_le_bytes() {
        mix(&mut h, b);
    }
    for b in tile.h.to_le_bytes() {
        mix(&mut h, b);
    }
    if pixel_overrides.is_empty() {
        for &b in &tile.pixels {
            mix(&mut h, b);
        }
    } else {
        for y in 0..tile.h as u32 {
            for x in 0..tile.w as u32 {
                let rgba = thumbnail_pixel_rgba(tile, pixel_overrides, x, y);
                for b in rgba {
                    mix(&mut h, b);
                }
            }
        }
    }
    h
}

fn ensure_tile_texture(
    ctx: &egui::Context,
    cache: &mut HashMap<TileId, (u64, egui::TextureHandle)>,
    tile: &Tile,
    pixel_overrides: &TilePixelOverrides,
) -> egui::TextureId {
    let hash = hash_tile_pixels(tile, pixel_overrides);
    if let Some((cached_hash, tex)) = cache.get(&tile.id) {
        if *cached_hash == hash {
            return tex.id();
        }
    }
    let w = tile.w as usize;
    let h = tile.h as usize;
    let rgba = if pixel_overrides.is_empty() {
        tile.pixels.clone()
    } else {
        let mut buf = Vec::with_capacity(w * h * 4);
        for y in 0..tile.h as u32 {
            for x in 0..tile.w as u32 {
                buf.extend_from_slice(&thumbnail_pixel_rgba(tile, pixel_overrides, x, y));
            }
        }
        buf
    };
    let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
    let tex = ctx.load_texture(
        format!("tile-thumb-{}", tile.id.0),
        image,
        egui::TextureOptions::NEAREST,
    );
    let id = tex.id();
    cache.insert(tile.id, (hash, tex));
    id
}

fn paint_coarse_checkerboard(
    painter: &egui::Painter,
    rect: egui::Rect,
    check_a: egui::Color32,
    check_b: egui::Color32,
) {
    let cols = 8usize;
    let rows = 8usize;
    let cw = rect.width() / cols as f32;
    let ch = rect.height() / rows as f32;
    for y in 0..rows {
        for x in 0..cols {
            let checker = if (x + y) % 2 == 0 { check_a } else { check_b };
            let px = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + x as f32 * cw, rect.min.y + y as f32 * ch),
                egui::pos2(
                    rect.min.x + (x + 1) as f32 * cw,
                    rect.min.y + (y + 1) as f32 * ch,
                ),
            );
            painter.rect_filled(px, 0.0, checker);
        }
    }
}

/// One thumbnail: the tile's pixels drawn scaled-up, with a highlight border
/// when selected. A click emits `Select(tile.id)`.
///
/// Fully-transparent tiles (e.g. the EMPTY tile "+ Empty" creates) are drawn
/// over a subtle checkerboard so the user can SEE the tile is empty before
/// placing it — honest feedback for the "no visible tile" workflow.
fn thumbnail_ui(
    ui: &mut egui::Ui,
    host: &TilePaletteHost,
    tile: &Tile,
    selected: bool,
    pixel_overrides: &TilePixelOverrides,
    chrome: &PanelChrome,
    thumb_cache: &mut HashMap<TileId, (u64, egui::TextureHandle)>,
) {
    let scale = thumb_scale(tile);
    let size = vec2(tile.w as f32 * scale, tile.h as f32 * scale);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let response = ui.interact(
        rect,
        egui::Id::new(("tile-thumbnail", tile.id.0)),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    let check_a = chrome.colors.panel_bg32();
    let check_b = chrome.colors.panel_border32();
    let pixel_count = (tile.w as usize).saturating_mul(tile.h as usize);
    if pixel_count <= INLINE_RECT_PIXEL_LIMIT {
        // Small tiles: keep exact per-pixel rect painting.
        for y in 0..tile.h as usize {
            for x in 0..tile.w as usize {
                let checker = if (x + y) % 2 == 0 { check_a } else { check_b };
                let px = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + x as f32 * scale, rect.min.y + y as f32 * scale),
                    vec2(scale, scale),
                );
                painter.rect_filled(px, 0.0, checker);
            }
        }
        for y in 0..tile.h as usize {
            for x in 0..tile.w as usize {
                let rgba = thumbnail_pixel_rgba(tile, pixel_overrides, x as u32, y as u32);
                let color = egui::Color32::from(egui::Rgba::from_rgba_unmultiplied(
                    rgba[0] as f32 / 255.0,
                    rgba[1] as f32 / 255.0,
                    rgba[2] as f32 / 255.0,
                    rgba[3] as f32 / 255.0,
                ));
                let px = egui::Rect::from_min_size(
                    egui::pos2(rect.min.x + x as f32 * scale, rect.min.y + y as f32 * scale),
                    vec2(scale, scale),
                );
                painter.rect_filled(px, 0.0, color);
            }
        }
    } else {
        // Large tiles: draw a coarse 8x8 checkerboard + one cached GPU texture quad
        // so adding many large tiles never tessellates thousands of RectShapes/frame.
        paint_coarse_checkerboard(painter, rect, check_a, check_b);
        let tex_id = ensure_tile_texture(ui.ctx(), thumb_cache, tile, pixel_overrides);
        painter.image(
            tex_id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            crate::ui::theme::SKIN_TINT,
        );
    }
    painter.rect_stroke(
        rect,
        THUMB_CORNER,
        egui::Stroke::new(1.0, chrome.colors.panel_border32()),
        egui::StrokeKind::Inside,
    );
    if selected {
        painter.rect_stroke(
            rect,
            THUMB_CORNER,
            egui::Stroke::new(
                SELECTED_MARKER_WIDTH,
                chrome.colors.selection_stroke_color32(),
            ),
            egui::StrokeKind::Inside,
        );
    }
    if response.clicked() {
        host.events
            .borrow_mut()
            .push(TilePalettePanelEvent::Select(tile.id));
    }
}

/// True when every pixel of `tile` is fully transparent — the EMPTY tile
/// created by "+ Empty". Placing it writes transparent cells, so the user
/// sees nothing; the panel warns about this.
fn tile_is_empty(tile: &Tile) -> bool {
    tile.pixels.chunks_exact(4).all(|px| px[3] == 0)
}

/// Scrollable thumbnail grid: wrapping rows that follow the panel width.
struct TilePaletteGridComponent {
    host: TilePaletteHost,
    thumb_cache: HashMap<TileId, (u64, egui::TextureHandle)>,
}

impl Component for TilePaletteGridComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.view.borrow();
        ui.add_space(HEADER_HEIGHT);
        if view.tiles.is_empty() {
            self.thumb_cache.clear();
            ui.weak("No tiles yet — add a tile with “+ Empty”.");
            return;
        }
        self.thumb_cache
            .retain(|id, _| view.tiles.iter().any(|t| t.id == *id));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = THUMB_GAP;
            ui.spacing_mut().item_spacing.y = THUMB_GAP;
            for tile in &view.tiles {
                let selected = view.selected == Some(tile.id);
                thumbnail_ui(
                    ui,
                    &self.host,
                    tile,
                    selected,
                    &view.pixel_overrides,
                    chrome,
                    &mut self.thumb_cache,
                );
            }
        });
        // Honest feedback: the SELECTED tile is empty (fully transparent).
        // Placing it writes transparent cells — nothing appears on the canvas.
        if let Some(selected) = view.selected {
            if view
                .tiles
                .iter()
                .find(|tile| tile.id == selected)
                .is_some_and(tile_is_empty)
            {
                ui.add_space(THUMB_GAP);
                ui.weak("The selected tile is empty (fully transparent) — placing it shows nothing on the canvas.");
            }
        }
    }
}

/// Panel content: the static nest holding the thumbnail grid and the pinned
/// header.
struct TilePalettePanelContent {
    nest: NestContent,
}

impl PanelContent for TilePalettePanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the thumbnail grid (content) and the pinned header
/// (overlay).
pub(crate) fn build_tile_palette_nest(host: &TilePaletteHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(TilePaletteGridComponent {
        host: host.clone(),
        thumb_cache: HashMap::new(),
    })
    .push(TilePaletteHeaderComponent { host: host.clone() });
    nest
}

/// Dock spec for the Tile Palette panel (dock id `PanelId::new(106)`).
///
/// **Dock-only:** `can_pop_out == false` (and `can_native_pop_out == false`), so
/// the header offers neither Float nor Pop-out; the App docks it right by
/// default.
pub fn tile_palette_panel_spec(host: &TilePaletteHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(TILE_PALETTE_PANEL_ID),
        metadata: PanelMetadata::new("Tile Palette", false, vec2(200.0, 140.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(160.0, 120.0), vec2(240.0, 320.0)),
        content: Box::new(TilePalettePanelContent {
            nest: build_tile_palette_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tile(id: u64, w: u16, h: u16, pixel: [u8; 4]) -> Tile {
        Tile {
            id: TileId(id),
            w,
            h,
            pixels: pixel.repeat(w as usize * h as usize),
        }
    }

    #[test]
    fn tile_palette_spec_is_dock_only_with_id_106() {
        let host = TilePaletteHost::default();
        let spec = tile_palette_panel_spec(&host, PanelPlacement::DockedRight);
        assert_eq!(spec.id.raw(), TILE_PALETTE_PANEL_ID);
        assert_eq!(spec.metadata.title, "Tile Palette");
        assert!(
            !spec.metadata.can_pop_out,
            "the Tile Palette panel is dock-only"
        );
        assert!(
            !spec.metadata.can_native_pop_out,
            "the Tile Palette panel must not pop out to a native surface"
        );
    }

    #[test]
    fn tile_palette_host_defaults_are_empty() {
        let host = TilePaletteHost::default();
        assert!(host.view.borrow().tiles.is_empty());
        assert_eq!(host.view.borrow().selected, None);
        assert!(host.events.borrow().is_empty());
    }

    #[test]
    fn tile_palette_host_snapshot_and_events_round_trip() {
        let host = TilePaletteHost::default();
        let snapshot = TilePaletteView {
            tiles: vec![
                sample_tile(1, 2, 2, [255, 0, 0, 255]),
                sample_tile(2, 4, 4, [0, 255, 0, 255]),
            ],
            selected: Some(TileId(2)),
            ..Default::default()
        };
        *host.view.borrow_mut() = snapshot.clone();
        assert_eq!(*host.view.borrow(), snapshot);

        host.events
            .borrow_mut()
            .push(TilePalettePanelEvent::Select(TileId(1)));
        host.events
            .borrow_mut()
            .push(TilePalettePanelEvent::AddEmpty);
        host.events
            .borrow_mut()
            .push(TilePalettePanelEvent::Delete(TileId(2)));
        host.events
            .borrow_mut()
            .push(TilePalettePanelEvent::MoveSelected(1));
        let drained: Vec<TilePalettePanelEvent> = host.events.borrow_mut().drain(..).collect();
        assert_eq!(
            drained,
            vec![
                TilePalettePanelEvent::Select(TileId(1)),
                TilePalettePanelEvent::AddEmpty,
                TilePalettePanelEvent::Delete(TileId(2)),
                TilePalettePanelEvent::MoveSelected(1),
            ]
        );
    }

    #[test]
    fn thumbnail_scale_fits_within_max_and_never_zero() {
        let big = sample_tile(1, 32, 16, [0, 0, 0, 255]);
        let scale = thumb_scale(&big);
        assert!(scale > 0.0);
        assert!(32.0 * scale <= THUMB_MAX + 0.5);
        assert!(16.0 * scale <= THUMB_MAX + 0.5);
        let tiny = sample_tile(2, 1, 1, [0, 0, 0, 255]);
        assert_eq!(thumb_scale(&tiny), THUMB_MAX_SCALE.min(THUMB_MAX));
    }

    #[test]
    fn thumbnail_uses_live_pixel_override_then_reverts_to_committed_pixel() {
        let tile = sample_tile(7, 2, 1, [10, 20, 30, 255]);
        let committed = [10, 20, 30, 255];
        let preview = [220, 40, 90, 255];
        let mut overrides = TilePixelOverrides::default();
        overrides.insert(tile.id, 1, 0, preview);

        assert_eq!(
            thumbnail_pixel_rgba(&tile, &overrides, 1, 0),
            preview,
            "a live stroke override is the thumbnail source while present"
        );
        assert_eq!(
            thumbnail_pixel_rgba(&tile, &TilePixelOverrides::default(), 1, 0),
            committed,
            "without an override the thumbnail falls back to committed tile bytes"
        );
        assert_eq!(tile.pixels, committed.repeat(2), "preview never mutates the tile");
    }

    #[test]
    fn large_tile_thumbnail_uses_cached_texture_instead_of_per_pixel_rects() {
        let host = TilePaletteHost::default();
        host.view.borrow_mut().tiles = vec![sample_tile(1, 128, 128, [255, 0, 0, 255])];
        let mut grid = TilePaletteGridComponent {
            host: host.clone(),
            thumb_cache: HashMap::new(),
        };
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome {
            colors: &theme.colors,
            theme: &theme,
            atlases: None,
            capture: &capture,
            surface: crate::ui::SurfaceId::Canvas,
            rounding: 3.0,
        };
        let view = NestView::new(
            egui::pos2(0.0, 0.0),
            1.0,
            egui::Vec2::ZERO,
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(260.0, 260.0)),
        );
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| grid.ui(ui, &view, &chrome));
        assert_eq!(grid.thumb_cache.len(), 1);
        assert!(
            out.shapes.len() < 100,
            "128x128 tile should render via 1 cached texture + coarse backdrop, got {} shapes",
            out.shapes.len()
        );
        out.textures_delta.clear();
    }
}
