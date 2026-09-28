//! Nest-based panel content: a scene/container layer between panel chrome and
//! components.
//!
//! A nest hosts an ordered list of [`Component`]s plus its own session-only
//! state. Two modes exist:
//!
//! - [`NestMode::Dynamic`]: content-space components are transformed by a
//!   cursor-anchored camera (Ctrl+scroll zoom, middle-drag pan) and clipped to
//!   the nest viewport; overlay components stay fixed in screen space.
//! - [`NestMode::Static`]: content is laid out at fixed scale, top-left
//!   aligned, measured through egui's scroll machinery; skinned scrollbars
//!   appear on overflow and share one offset with wheel and pan; overlay
//!   components stay fixed in screen space, pinned to the viewport.
//!
//! Both modes inset the nest viewport by [`NEST_PADDING`] so content never
//! touches the panel frame or resize edges.

pub mod camera;
pub mod dynamic;
pub mod scrollbar;
pub mod statik;

use std::cell::Cell;
use std::rc::Rc;

use egui::Vec2;

use super::skin::PanelChrome;
use super::state::PanelContent;
use camera::NestCamera;

/// Inset applied to the nest viewport on every side, in both modes, so
/// transformed/scrolled content never touches the panel frame or resize edges.
///
/// Sized to clear the dock's 6pt resize-handle band: with the 2pt skinned
/// panel-frame padding the content starts 7pt inside the panel rect.
pub const NEST_PADDING: f32 = 5.0;

/// The nest viewport: `full` shrunk by [`NEST_PADDING`] on every side.
///
/// The inset is clamped to at most a quarter of each dimension so tiny panels
/// keep a valid (non-inverted) rect.
pub fn nest_viewport(full: egui::Rect) -> egui::Rect {
    let inset = NEST_PADDING
        .min(full.width() / 4.0)
        .min(full.height() / 4.0)
        .max(0.0);
    full.shrink(inset)
}

/// Smallest nest viewport that can still show content; below this the panel
/// is considered collapsed and every component is hidden.
pub const NEST_MIN_USABLE_EXTENT: f32 = 24.0;

/// True when `viewport` is big enough to render nest components.
pub fn nest_is_usable(viewport: egui::Rect) -> bool {
    viewport.width() >= NEST_MIN_USABLE_EXTENT && viewport.height() >= NEST_MIN_USABLE_EXTENT
}

/// Where a component lives in the nest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentPlacement {
    /// Painted in content space; transformed by the dynamic camera.
    Content,
    /// Painted in screen space, fixed relative to the nest viewport.
    Overlay,
}

/// World-to-screen mapping for a nest's components.
///
/// Content components receive the dynamic camera's mapping and convert their
/// own world coordinates; overlays (and static nests) receive
/// [`NestView::screen`], where the mapping is the identity. Everything paints
/// in the host panel's own layer, so content always stays below the panel's
/// overlays and inside its z band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NestView {
    origin: egui::Pos2,
    scale: f32,
    offset: Vec2,
    clip: egui::Rect,
}

impl NestView {
    /// A mapping for world-space content: `origin` is the screen position of
    /// world `(0, 0)` before the camera offset, `clip` is the nest viewport.
    pub fn new(origin: egui::Pos2, scale: f32, offset: Vec2, clip: egui::Rect) -> Self {
        Self {
            origin,
            scale,
            offset,
            clip,
        }
    }

    /// The identity mapping for screen-space content (overlays, static nests).
    pub fn screen(clip: egui::Rect) -> Self {
        Self {
            origin: egui::Pos2::ZERO,
            scale: 1.0,
            offset: Vec2::ZERO,
            clip,
        }
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    pub fn offset(&self) -> Vec2 {
        self.offset
    }

    /// The nest viewport in screen space; content is clipped to it.
    pub fn clip(&self) -> egui::Rect {
        self.clip
    }

    /// Maps a world point to screen space.
    pub fn world_to_screen(&self, world: egui::Pos2) -> egui::Pos2 {
        self.origin + self.offset + world.to_vec2() * self.scale
    }

    /// Maps a world rect to screen space.
    pub fn world_rect_to_screen(&self, rect: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_max(
            self.world_to_screen(rect.min),
            self.world_to_screen(rect.max),
        )
    }

    /// The world rect currently visible inside [`Self::clip`].
    pub fn visible_world_rect(&self) -> egui::Rect {
        let scale = if self.scale.abs() < f32::EPSILON {
            1.0
        } else {
            self.scale
        };
        let min = (self.clip.min - self.origin - self.offset) / scale;
        let max = (self.clip.max - self.origin - self.offset) / scale;
        egui::Rect::from_min_max(min.to_pos2(), max.to_pos2())
    }

    /// A painter clipped to the nest viewport.
    pub fn painter<'a>(&self, ui: &'a egui::Ui) -> egui::Painter {
        ui.painter().with_clip_rect(self.clip)
    }
}

/// A nest component: owns its own state and paints immediately.
///
/// Components declare whether they live in content space (mapped by the
/// dynamic camera's [`NestView`]) or as a screen-space overlay. Both nest modes
/// honour the placement: overlays paint after content, stay fixed relative to
/// the nest viewport, and are clipped to it.
pub trait Component {
    /// Paints the component into `ui`, converting world coordinates through
    /// `view` when the component lives in content space. `chrome` is the host
    /// panel's chrome, so components can paint with the theme's nine-slice
    /// skins and color tokens.
    fn ui(&mut self, ui: &mut egui::Ui, view: &NestView, chrome: &PanelChrome);

    /// Where the component lives in the nest.
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Content
    }
}

/// The nest kind: dynamic (zoomable/panable camera) or static (fixed scale).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NestMode {
    Dynamic,
    Static,
}

/// A camera reset a component asks the nest to perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NestReset {
    Pan,
    Zoom,
}

/// Channel a component uses to request a camera reset from its nest.
///
/// Components cannot reach the nest's camera directly; they publish a request
/// through this handle and the dynamic nest applies it on the next frame.
#[derive(Clone, Default)]
pub struct NestResetHandle(Rc<Cell<Option<NestReset>>>);

impl NestResetHandle {
    /// Requests a reset; the last request in a frame wins.
    pub fn request(&self, reset: NestReset) {
        self.0.set(Some(reset));
    }

    /// Takes the pending request, if any.
    pub fn take(&self) -> Option<NestReset> {
        self.0.take()
    }
}

/// One nest per panel: an ordered component list plus the nest state.
///
/// Nest state is session-only — it lives in the panel entry and is never
/// persisted.
pub struct NestContent {
    pub(crate) mode: NestMode,
    pub(crate) components: Vec<Box<dyn Component>>,
    /// Dynamic only: the camera.
    pub(crate) camera: NestCamera,
    /// Static only: the shared scroll offset, mirrored into the scroll area.
    pub(crate) offset: Vec2,
    reset: NestResetHandle,
}

impl NestContent {
    /// A new empty nest in the given mode.
    pub fn new(mode: NestMode) -> Self {
        Self {
            mode,
            components: Vec::new(),
            camera: NestCamera::new(),
            offset: Vec2::ZERO,
            reset: NestResetHandle::default(),
        }
    }

    /// Appends a component to the nest.
    pub fn push(&mut self, component: impl Component + 'static) -> &mut Self {
        self.components.push(Box::new(component));
        self
    }

    /// A handle components can use to request camera resets.
    pub fn reset_handle(&self) -> NestResetHandle {
        self.reset.clone()
    }

    /// Takes the pending reset request, if any.
    pub(crate) fn take_reset(&self) -> Option<NestReset> {
        self.reset.take()
    }

    /// The nest mode.
    pub fn mode(&self) -> NestMode {
        self.mode
    }

    /// The dynamic camera (dynamic mode only).
    pub fn camera(&self) -> &NestCamera {
        &self.camera
    }

    /// Mutable access to the dynamic camera (dynamic mode only).
    pub fn camera_mut(&mut self) -> &mut NestCamera {
        &mut self.camera
    }

    /// The shared scroll offset (static mode only).
    pub fn offset(&self) -> Vec2 {
        self.offset
    }
}

impl PanelContent for NestContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        match self.mode {
            NestMode::Dynamic => dynamic::show_dynamic(self, ui, chrome),
            NestMode::Static => statik::show_static(self, ui, chrome),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Rect};

    #[test]
    fn nest_viewport_insets_by_nest_padding() {
        let full = Rect::from_min_size(pos2(10.0, 20.0), vec2(200.0, 100.0));
        assert_eq!(nest_viewport(full), full.shrink(NEST_PADDING));
        assert_eq!(nest_viewport(full).min, pos2(15.0, 25.0));
        assert_eq!(nest_viewport(full).max, pos2(205.0, 115.0));
    }

    #[test]
    fn nest_viewport_clamps_the_inset_for_tiny_panels() {
        // An 8x8 panel: the full inset would leave nothing; the quarter clamp
        // keeps a 4x4 viewport.
        let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(8.0, 8.0));
        assert_eq!(
            nest_viewport(full),
            Rect::from_min_size(pos2(2.0, 2.0), vec2(4.0, 4.0))
        );
    }

    #[test]
    fn nest_viewport_never_inverts() {
        // A 2x2 panel: the inset clamps to 0.5, leaving a valid 1x1 viewport.
        let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(2.0, 2.0));
        let viewport = nest_viewport(full);
        assert!(viewport.width() > 0.0 && viewport.height() > 0.0);
        assert!(viewport.min.x <= viewport.max.x && viewport.min.y <= viewport.max.y);
    }

    #[test]
    fn nest_is_usable_rejects_collapsed_viewports() {
        let at = |w: f32, h: f32| Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
        assert!(nest_is_usable(at(
            NEST_MIN_USABLE_EXTENT,
            NEST_MIN_USABLE_EXTENT
        )));
        assert!(!nest_is_usable(at(
            NEST_MIN_USABLE_EXTENT - 1.0,
            NEST_MIN_USABLE_EXTENT
        )));
        assert!(!nest_is_usable(at(
            NEST_MIN_USABLE_EXTENT,
            NEST_MIN_USABLE_EXTENT - 1.0
        )));
        assert!(!nest_is_usable(at(8.0, 8.0)));
        assert!(!nest_is_usable(Rect::NOTHING));
    }
}
