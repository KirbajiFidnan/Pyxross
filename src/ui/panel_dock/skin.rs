//! Nine-slice skin rendering for dock-panel chrome.
//!
//! Skins are atlas textures declared by [`Theme`]; this module turns them into
//! egui meshes. Every paint path is skinned-first with a flat-color fallback:
//! [`PanelChrome::paint_slice`] returns `false` when the theme has no such
//! skin or its atlas is not loaded, and the caller paints the flat
//! [`ThemeColors`] fill instead.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{pos2, vec2, Align2, Color32, Mesh, Rect, Response, TextureId};

use crate::ui::input_capture::{InputCapture, SurfaceId};
use crate::ui::theme::{NineSlice, Skin, SkinState, Theme, ThemeColors, SKIN_TINT};

/// Presentation flags for [`PanelChrome::button`].
///
/// The interaction state (normal/hover/pressed/disabled) is derived from the
/// button's own [`Response`]; this only carries the flags the caller decides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ButtonStyle {
    /// Paints the selected (toggled-on) look: a translucent selection wash
    /// over the skin, or egui's selectable-button look without a skin.
    pub selected: bool,
    /// When false the button renders disabled and ignores clicks.
    pub enabled: bool,
}

impl ButtonStyle {
    /// An enabled, unselected button.
    pub const fn plain() -> Self {
        Self {
            selected: false,
            enabled: true,
        }
    }

    /// An enabled button with the selected look when `selected`.
    pub const fn toggled(selected: bool) -> Self {
        Self {
            selected,
            enabled: true,
        }
    }

    /// A disabled, unselected button.
    pub const fn disabled() -> Self {
        Self {
            selected: false,
            enabled: false,
        }
    }
}

/// The skin state for a button response: disabled > pressed > hover > normal.
fn button_skin_state(response: &Response) -> SkinState {
    if !response.enabled() {
        SkinState::Disabled
    } else if response.is_pointer_button_down_on() {
        SkinState::Pressed
    } else if response.hovered() {
        SkinState::Hover
    } else {
        SkinState::Normal
    }
}

/// Splits a destination rect into 9 regions in points space.
///
/// The corner is clamped to half the rect's width/height so the center regions
/// never invert; a zero-size center is allowed (degenerate dest).
pub fn split_dest(dest: Rect, corner: f32) -> NineSlice {
    let c = corner
        .max(0.0)
        .min(dest.width() * 0.5)
        .min(dest.height() * 0.5);
    let (x, y) = (dest.min.x, dest.min.y);
    let (w, h) = (dest.width(), dest.height());
    let rect = |x0: f32, y0: f32, w0: f32, h0: f32| Rect::from_min_size(pos2(x0, y0), vec2(w0, h0));
    NineSlice {
        top_left: rect(x, y, c, c),
        top_center: rect(x + c, y, w - 2.0 * c, c),
        top_right: rect(x + w - c, y, c, c),
        middle_left: rect(x, y + c, c, h - 2.0 * c),
        center: rect(x + c, y + c, w - 2.0 * c, h - 2.0 * c),
        middle_right: rect(x + w - c, y + c, c, h - 2.0 * c),
        bottom_left: rect(x, y + h - c, c, c),
        bottom_center: rect(x + c, y + h - c, w - 2.0 * c, c),
        bottom_right: rect(x + w - c, y + h - c, c, c),
    }
}

/// Converts pixel-space regions to normalized UV coordinates in [0, 1].
///
/// egui UVs are top-left origin with no Y flip, so this is a plain division.
pub fn to_uv(src: &NineSlice, atlas_size: (u32, u32)) -> NineSlice {
    let (w, h) = (atlas_size.0 as f32, atlas_size.1 as f32);
    let map = |r: &Rect| {
        Rect::from_min_max(
            pos2(r.min.x / w, r.min.y / h),
            pos2(r.max.x / w, r.max.y / h),
        )
    };
    NineSlice {
        top_left: map(&src.top_left),
        top_center: map(&src.top_center),
        top_right: map(&src.top_right),
        middle_left: map(&src.middle_left),
        center: map(&src.center),
        middle_right: map(&src.middle_right),
        bottom_left: map(&src.bottom_left),
        bottom_center: map(&src.bottom_center),
        bottom_right: map(&src.bottom_right),
    }
}

/// Builds a 9-slice mesh: 36 vertices and 54 indices.
///
/// `src` holds the UV-space source regions (see [`to_uv`]); `dest` is split by
/// `corner` (points) and each source region is stretched onto its destination
/// region. A `corner` of 0 center-stretches the whole source.
pub fn build_slice_mesh(
    texture: TextureId,
    src: &NineSlice,
    dest: Rect,
    corner: f32,
    tint: Color32,
) -> Mesh {
    let mut mesh = Mesh::with_texture(texture);
    let dest_slices = split_dest(dest, corner);
    let regions = [
        (&src.top_left, &dest_slices.top_left),
        (&src.top_center, &dest_slices.top_center),
        (&src.top_right, &dest_slices.top_right),
        (&src.middle_left, &dest_slices.middle_left),
        (&src.center, &dest_slices.center),
        (&src.middle_right, &dest_slices.middle_right),
        (&src.bottom_left, &dest_slices.bottom_left),
        (&src.bottom_center, &dest_slices.bottom_center),
        (&src.bottom_right, &dest_slices.bottom_right),
    ];
    for (uv, rect) in regions {
        mesh.add_rect_with_uv(*rect, *uv, tint);
    }
    mesh
}

/// A loaded atlas texture plus its pixel dimensions.
///
/// Deliberately no `Debug`: [`egui::TextureHandle`] does not implement it.
pub struct SkinAtlas {
    texture: egui::TextureHandle,
    size: (u32, u32),
}

impl SkinAtlas {
    pub fn new(texture: egui::TextureHandle, size: (u32, u32)) -> Self {
        Self { texture, size }
    }

    /// The uploaded texture.
    pub fn texture(&self) -> &egui::TextureHandle {
        &self.texture
    }

    /// Atlas dimensions in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

/// Loaded atlases keyed by the atlas path each skin declares.
///
/// The keys are the `atlas_path` strings from the theme's skins; the loader
/// resolves them against the theme dir when reading the files.
#[derive(Default)]
pub struct AtlasCache(HashMap<PathBuf, SkinAtlas>);

impl AtlasCache {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    pub fn insert(&mut self, path: PathBuf, atlas: SkinAtlas) {
        self.0.insert(path, atlas);
    }

    pub fn get(&self, path: &Path) -> Option<&SkinAtlas> {
        self.0.get(path)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The chrome context for one frame of dock-panel painting.
///
/// Skinned-first: [`Self::paint_slice`] paints the theme's nine-slice skin
/// when both the skin and its atlas are available, and returns `false` so the
/// caller paints the flat [`ThemeColors`] fallback.
///
/// `capture` routes zoom/pan/scroll gestures; `surface` is the surface the
/// panel currently being painted owns. Both are per-panel, so the dock calls
/// [`Self::for_surface`] before painting each panel's content.
#[derive(Clone, Copy)]
pub struct PanelChrome<'a> {
    pub colors: &'a ThemeColors,
    pub theme: &'a Theme,
    pub atlases: Option<&'a AtlasCache>,
    pub capture: &'a InputCapture,
    pub surface: SurfaceId,
}

impl<'a> PanelChrome<'a> {
    /// Flat-only chrome (no atlases) for `surface`: every paint falls back to
    /// colors, gestures route through `capture`.
    pub fn flat(theme: &'a Theme, capture: &'a InputCapture, surface: SurfaceId) -> Self {
        Self {
            colors: &theme.colors,
            theme,
            atlases: None,
            capture,
            surface,
        }
    }

    /// The same chrome bound to another surface.
    pub fn for_surface(self, surface: SurfaceId) -> Self {
        Self { surface, ..self }
    }

    /// The named skin plus its loaded atlas, if both exist.
    pub fn skin(&self, name: &str) -> Option<(&'a Skin, &'a SkinAtlas)> {
        let skin = self.theme.skin(name)?;
        let atlas = self.atlases?.get(Path::new(&skin.atlas_path))?;
        Some((skin, atlas))
    }

    /// Paints one skin state stretched over `dest` with the given corner size
    /// (points). A `corner` of 0 center-stretches the whole region.
    ///
    /// Low-level primitive: callers resolve the skin/atlas pair via
    /// [`Self::skin`] first (e.g. to override the corner for thin strips).
    pub fn paint_nine_slice(
        &self,
        painter: &egui::Painter,
        skin: &Skin,
        atlas: &SkinAtlas,
        state: SkinState,
        dest: Rect,
        corner: f32,
    ) {
        let src = NineSlice::from_source(skin.source_for(state), atlas.size());
        let uv = to_uv(&src, atlas.size());
        let mesh = build_slice_mesh(atlas.texture().id(), &uv, dest, corner, SKIN_TINT);
        painter.add(egui::Shape::Mesh(mesh.into()));
    }

    /// Paints the named skin in `state` over `dest` using the skin's own
    /// corner size. Returns `false` when the theme has no such skin or its
    /// atlas is not loaded — the caller then paints the flat fallback.
    pub fn paint_slice(
        &self,
        painter: &egui::Painter,
        name: &str,
        state: SkinState,
        dest: Rect,
    ) -> bool {
        let Some((skin, atlas)) = self.skin(name) else {
            return false;
        };
        let corner = NineSlice::from_source(skin.source_for(state), atlas.size())
            .top_left
            .width();
        self.paint_nine_slice(painter, skin, atlas, state, dest, corner);
        true
    }

    /// Paints a button labelled `label` and returns its response.
    ///
    /// Skinned-first: when the theme has a `button` skin the nine-slice is
    /// painted in the state resolved from the response (Normal/Hover/Pressed/
    /// Disabled) with the label centered over it, plus a translucent selection
    /// wash when `style.selected`. Without a skin the plain egui button (a
    /// selectable one when `selected`) is used instead, so panels keep working
    /// unskinned. The button sizes to its label plus the standard padding and
    /// never exceeds the available width, so a narrow panel is not widened.
    pub fn button(&self, ui: &mut egui::Ui, label: &str, style: ButtonStyle) -> Response {
        if self.skin("button").is_none() {
            return ui.add_enabled(
                style.enabled,
                egui::Button::selectable(style.selected, label),
            );
        }
        let font = egui::TextStyle::Button.resolve(ui.style());
        let text_color = ui.visuals().text_color();
        let galley = ui
            .painter()
            .layout_no_wrap(label.to_owned(), font.clone(), text_color);
        let size = vec2(
            (galley.size().x + 8.0).min(ui.available_width().max(1.0)),
            ui.spacing().interact_size.y.max(galley.size().y + 4.0),
        );
        ui.add_enabled_ui(style.enabled, |ui| {
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
            let state = button_skin_state(&response);
            self.paint_slice(ui.painter(), "button", state, rect);
            if style.selected {
                let wash = self.colors.selection_bg_fill32().gamma_multiply(0.5);
                ui.painter().rect_filled(rect, 2.0, wash);
            }
            ui.painter().with_clip_rect(rect).text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                font,
                text_color,
            );
            response
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    #[test]
    fn split_dest_splits_into_nine_regions() {
        let ns = split_dest(rect(10.0, 20.0, 40.0, 30.0), 5.0);
        assert_eq!(ns.top_left, rect(10.0, 20.0, 5.0, 5.0));
        assert_eq!(ns.top_center, rect(15.0, 20.0, 30.0, 5.0));
        assert_eq!(ns.top_right, rect(45.0, 20.0, 5.0, 5.0));
        assert_eq!(ns.middle_left, rect(10.0, 25.0, 5.0, 20.0));
        assert_eq!(ns.center, rect(15.0, 25.0, 30.0, 20.0));
        assert_eq!(ns.middle_right, rect(45.0, 25.0, 5.0, 20.0));
        assert_eq!(ns.bottom_left, rect(10.0, 45.0, 5.0, 5.0));
        assert_eq!(ns.bottom_center, rect(15.0, 45.0, 30.0, 5.0));
        assert_eq!(ns.bottom_right, rect(45.0, 45.0, 5.0, 5.0));
    }

    #[test]
    fn split_dest_clamps_corner_to_half() {
        let ns = split_dest(rect(0.0, 0.0, 10.0, 10.0), 99.0);
        assert_eq!(ns.top_left, rect(0.0, 0.0, 5.0, 5.0));
        assert_eq!(ns.center, rect(5.0, 5.0, 0.0, 0.0));
        assert_eq!(ns.bottom_right, rect(5.0, 5.0, 5.0, 5.0));
    }

    #[test]
    fn split_dest_degenerate_smaller_than_two_corners() {
        let ns = split_dest(rect(0.0, 0.0, 4.0, 4.0), 8.0);
        assert_eq!(ns.top_left, rect(0.0, 0.0, 2.0, 2.0));
        assert_eq!(ns.center, rect(2.0, 2.0, 0.0, 0.0));
        assert_eq!(ns.bottom_right, rect(2.0, 2.0, 2.0, 2.0));
    }

    #[test]
    fn to_uv_normalizes_pixel_space() {
        let src = NineSlice {
            top_left: rect(0.0, 0.0, 4.0, 4.0),
            top_center: rect(4.0, 0.0, 24.0, 4.0),
            top_right: rect(28.0, 0.0, 4.0, 4.0),
            middle_left: rect(0.0, 4.0, 4.0, 24.0),
            center: rect(4.0, 4.0, 24.0, 24.0),
            middle_right: rect(28.0, 4.0, 4.0, 24.0),
            bottom_left: rect(0.0, 28.0, 4.0, 4.0),
            bottom_center: rect(4.0, 28.0, 24.0, 4.0),
            bottom_right: rect(28.0, 28.0, 4.0, 4.0),
        };
        let uv = to_uv(&src, (32, 32));
        assert_eq!(uv.top_left, rect(0.0, 0.0, 0.125, 0.125));
        assert_eq!(uv.center, rect(0.125, 0.125, 0.75, 0.75));
        assert_eq!(uv.bottom_right, rect(0.875, 0.875, 0.125, 0.125));
    }

    #[test]
    fn build_slice_mesh_emits_36_vertices_and_54_indices() {
        let source = crate::ui::theme::NineSliceSource {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
            corner_size: 4,
        };
        let src = to_uv(&NineSlice::from_source(&source, (32, 32)), (32, 32));
        let mesh = build_slice_mesh(
            TextureId::Managed(7),
            &src,
            rect(0.0, 0.0, 64.0, 64.0),
            4.0,
            SKIN_TINT,
        );
        assert_eq!(mesh.texture_id, TextureId::Managed(7));
        assert_eq!(mesh.vertices.len(), 36);
        assert_eq!(mesh.indices.len(), 54);
        assert!(mesh.is_valid());
        // Top-left corner: dest origin maps the source corner UV.
        assert_eq!(mesh.vertices[0].pos, pos2(0.0, 0.0));
        assert_eq!(mesh.vertices[0].uv, pos2(0.0, 0.0));
        // Center region (5th rect, vertices 16-19): inset by the corner.
        assert_eq!(mesh.vertices[16].pos, pos2(4.0, 4.0));
        assert_eq!(mesh.vertices[16].uv, pos2(4.0 / 32.0, 4.0 / 32.0));
        assert_eq!(mesh.vertices[19].pos, pos2(60.0, 60.0));
        assert_eq!(mesh.vertices[19].uv, pos2(28.0 / 32.0, 28.0 / 32.0));
    }

    #[test]
    fn build_slice_mesh_zero_corner_center_stretches() {
        let source = crate::ui::theme::NineSliceSource {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
            corner_size: 4,
        };
        let src = to_uv(&NineSlice::from_source(&source, (32, 32)), (32, 32));
        let mesh = build_slice_mesh(
            TextureId::Managed(1),
            &src,
            rect(0.0, 0.0, 6.0, 6.0),
            0.0,
            SKIN_TINT,
        );
        // Corner 0: the whole dest is the center region.
        assert_eq!(mesh.vertices[16].pos, pos2(0.0, 0.0));
        assert_eq!(mesh.vertices[19].pos, pos2(6.0, 6.0));
        assert_eq!(mesh.vertices[16].uv, pos2(4.0 / 32.0, 4.0 / 32.0));
    }

    #[test]
    fn paint_slice_returns_false_without_skin_or_atlas() {
        let theme = Theme::default_dark();
        let capture = InputCapture::new();
        let surface = SurfaceId::Canvas;
        capture.set_target(Some(surface));
        let chrome = PanelChrome::flat(&theme, &capture, surface);
        let ctx = egui::Context::default();
        let painter = ctx.layer_painter(egui::LayerId::background());
        assert!(!chrome.paint_slice(
            &painter,
            "panel_bg",
            SkinState::Normal,
            rect(0.0, 0.0, 10.0, 10.0)
        ));
    }
}
