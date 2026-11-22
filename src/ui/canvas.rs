//! Canvas widget — wgpu texture → egui. R1 milestone.

use crate::core::brush::{bresenham_line, BrushSpec, DrawMode};
use crate::core::camera::Camera;
use crate::core::clip::PixelClip;
use crate::core::color::Color;
use crate::core::math::Rect2i;
use crate::core::tilemap::TileSize;
use crate::render::gizmo::GizmoHit;
use crate::render::overlay::{
    ant_segments, ant_stroke_color, ant_under_stroke_color, mask_polylines,
    pixel_ant_edges_from_polylines, BoundarySegment, OnionGhost, PIXEL_ANT_SUBDIV,
};
use crate::ui::input_capture::{ctrl_wheel, shift_wheel, InputCapture, SurfaceId};
use crate::ui::theme::ThemeColors;

/// Approximate scroll points per zoom notch.
///
/// egui reports discrete mouse-wheel notches as `line_scroll_speed` points
/// (default 40.0 on native) and smooths them over several frames; smoothed
/// scroll deltas are normalized against this so one wheel notch maps to one
/// integer zoom step.
const SCROLL_POINTS_PER_NOTCH: f32 = 40.0;

/// Continuous zoom factor per full wheel notch (plain scroll, no modifier).
const ZOOM_PER_NOTCH: f32 = 1.15;

/// Tile-grid lines closer than this many points are suppressed.
const MIN_TILE_LINE_SPACING: f32 = 4.0;

/// Alpha applied to the theme's `frame_region_ant` token so the animated
/// frame-region dashes read as a clearly VISIBLE yellow line.
///
/// Deliberately higher than the selection ants'
/// [`crate::render::overlay::ANT_STROKE_ALPHA`] = 120: at the shared 47% alpha
/// the yellow dashes blended with the bordeaux under-baseline into grey, so the
/// frame borders were barely legible against the artwork. This token keeps its
/// RGB channels from the theme (no hard-coded chrome colour) and only raises
/// the alpha.
const FRAME_REGION_ANT_STROKE_ALPHA: u8 = 220;

/// Speed scale for the frame-region marching ants: the phase input fed to
/// [`pixel_ant_edges_from_polylines`] is `time_ms * 0.5`, so the pattern
/// marches HALF as fast as the selection ants. The global
/// [`crate::render::overlay::ANT_SPEED_PX_PER_MS`] is left untouched — only
/// the frame-region dashes are slowed down.
const FRAME_REGION_ANT_SPEED_SCALE: f32 = 0.5;

/// Per-frame interactions reported by [`CanvasWidget::ui`].
///
/// The App shell drives stroke begin/continue/finish and viewport pan/zoom from
/// these; the contract is fixed — do not rename or reshape.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CanvasInteractions {
    /// A primary-button drag began this frame (with a valid first canvas point).
    pub stroke_started: bool,
    /// A primary-button gesture entered (or re-entered) the canvas this frame
    /// (or began on it this frame): the tool must start a NEW segment at
    /// `stroke_point` (no interpolation from the previous point). Set by the
    /// App's global-pointer gesture machine both for the press frame (press
    /// paints immediately) and for presses that armed outside the canvas.
    pub stroke_segment_started: bool,
    /// The canvas pixel under the cursor this frame (during a primary drag).
    pub stroke_point: Option<(i32, i32)>,
    /// The primary-button drag ended this frame.
    pub stroke_ended: bool,
    /// Whether the canvas widget itself holds the primary press this frame
    /// (true even when the press is in the letterbox around the draw rect).
    /// The App uses this to exclude the canvas from the "another UI wants the
    /// pointer" test when deciding whether to arm a tool from outside.
    pub primary_down_on_canvas: bool,
    /// Canvas-pixel pan applied this frame by a middle-button drag.
    pub pan_by: (i32, i32),
    /// Zoom applied this frame: `(factor, pointer_pos)`.
    pub zoom: Option<(f32, egui::Pos2)>,
    /// Shift+scroll delta over the canvas (points); the App maps it to the
    /// Draw tool's brush size. Shift+scroll never zooms.
    pub brush_scroll: f32,
    /// Ctrl+scroll steps over the canvas: the App sets the active Draw tool's
    /// brush shape from their sign (up → round, down → square, net zero → no
    /// change). Ctrl+scroll never zooms and never resizes.
    pub shape_cycle: i32,
    /// Alt+scroll delta over the canvas (points); the App maps it to the
    /// active Draw tool's scatter amount. Alt+scroll never zooms and never
    /// changes the brush size or shape.
    pub scatter_scroll: f32,
    /// Camera after this frame's viewport interactions.
    pub updated_camera: Camera,
    /// The canvas pixel under the cursor when the primary button was clicked
    /// without dragging (press and release within the click threshold).
    pub clicked: Option<(i32, i32)>,
    /// The canvas pixel under the cursor when the primary button was
    /// double-clicked this frame.
    pub double_clicked: Option<(i32, i32)>,
    /// RAW primary-press signal, independent of egui's widget attribution.
    ///
    /// egui 0.36 resolves a press on a DRAG-ONLY widget (a dock panel header
    /// band, a resize handle, a scrollbar) as `click: None, drag: Some`, so the
    /// widget-scoped `clicked`/`stroke_started` never fire for those presses
    /// even when the pointer is geometrically over the canvas draw rect. The
    /// tile placer must stamp from this RAW press point instead: set whenever
    /// the primary button went down this frame AND the press position maps to a
    /// canvas pixel inside the draw rect (the press origin when available,
    /// otherwise the latest pointer position).
    pub raw_pressed_over_canvas: Option<(i32, i32)>,
    /// The PRIMARY-press-frame modifiers latched by the widget (D78), attached
    /// to [`Self::clicked`] / [`Self::double_clicked`] so click-driven tools
    /// (the Wand) observe Alt/Shift as they were at the press, not as they are
    /// at the release. winit batches events per redraw, so the modifier key-up
    /// can land in the same batch as the mouse-up; re-reading the live
    /// modifiers at the click frame would then silently drop them.
    ///
    /// `None` when no press-frame snapshot exists (a synthetic/direct
    /// `CanvasInteractions` built by tests); callers then fall back to the
    /// live held state.
    pub click_modifiers: Option<egui::Modifiers>,
    /// A secondary-button (right) press/drag began this frame over the canvas.
    pub eyedropper_started: bool,
    /// The canvas pixel under the cursor this frame during a secondary-button
    /// press or drag.
    pub eyedropper_point: Option<(i32, i32)>,
    /// The secondary-button press/drag ended this frame.
    pub eyedropper_ended: bool,
    /// Optional out-of-bounds canvas point recorded just before a draw tool
    /// crossed into the canvas, so fast inward swipes paint right from the edge.
    pub stroke_segment_entry_from: Option<(i32, i32)>,
}

/// Readout for the transform drag HUD (item 5), built by the App from the live
/// transform state and painted near the pointer by the canvas widget while a
/// transform handle is grabbed. Display-only.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct HudState {
    /// Transformed output size in pixels (W, H).
    pub size: (u32, u32),
    /// Rotation angle in degrees (clockwise positive).
    pub angle_deg: f32,
    /// Canvas position of the transformed bbox's top-left.
    pub pos: (i32, i32),
    /// Per-axis scale factors.
    pub scale: (f32, f32),
}

/// Decorations painted on top of the canvas texture, in canvas-pixel space.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum CanvasOverlay {
    #[default]
    None,
    /// A pixel-region highlight: the committed selection, drawn as a marching
    /// ants border.
    Selection(Rect2i),
    /// The live selection gesture (marquee, area move, region drag): a plain
    /// solid outline with no fill and no dashes, so an in-progress selection
    /// never covers the pixels it is measuring.
    Marquee(Rect2i),
    /// A raw lasso trace (the pointer path in canvas coordinates): painted as
    /// one closed plain solid polyline, no fill and no dashes, so the traced
    /// shape never covers the pixels it is measuring (D73).
    Lasso(Vec<(i32, i32)>),
    /// The region (tile) cells under a live region-selection drag: each cell
    /// painted as a plain solid unit outline in the live-gesture color, no
    /// fill (D73). `base` is the pre-gesture selection's boundary
    /// ([`crate::core::select::Selection::outline_segments`]), painted as the
    /// committed marching ants under the swept cells so the existing selection
    /// stays visible during the drag; empty when the gesture has no base.
    RegionCells {
        cells: Vec<Rect2i>,
        base: Vec<BoundarySegment>,
    },
    /// Onion-skin ghosts: tinted textured quads for neighboring frames.
    Onion(Vec<OnionGhost>),
    /// Frame Edit onion-skin overlay viewports (Fix 5): the WRAPPED previous /
    /// next frame regions painted as translucent textured quads into the
    /// CURRENT region's screen rect (so they align with the base viewport),
    /// with UVs sampling the neighbour region. Painter-only and
    /// non-interactive — it never captures or alters the pointer.
    FrameOnion {
        /// Wrapped previous-frame region (absolute canvas coords), if shown.
        prev: Option<Rect2i>,
        /// Wrapped next-frame region (absolute canvas coords), if shown.
        next: Option<Rect2i>,
    },
    /// Onion-skin overlay viewports: the wrapped previous / next frame regions
    /// painted as translucent textured quads into the CURRENT region's screen
    /// rect (so the ghost aligns with the base viewport), with UVs sampling the
    /// neighbour region. Painter-only and non-interactive — it never captures or
    /// alters the pointer.
    ///
    /// Same geometry/ordering contract as [`Self::FrameOnion`]; it differs only
    /// in tint source: `prev`/`next` use the OPAQUE `ThemeColors::onion_skin_*`
    /// tokens, and the painter applies the 75/255 preview alpha itself.
    OnionOverlay {
        /// Wrapped previous-frame region (absolute canvas coords), if shown.
        prev: Option<Rect2i>,
        /// Wrapped next-frame region (absolute canvas coords), if shown.
        next: Option<Rect2i>,
    },
    /// Marching-ants border around a pixel region.
    Ants(Rect2i),
    /// Marching-ants border around an arbitrary mask shape: the boundary unit
    /// segments in canvas coordinates (see `Selection::outline_segments`).
    AntsMask(Vec<((i32, i32), (i32, i32))>),
    /// Selected-animation frame-region guide: every region's border drawn as a
    /// pixel-perfect marching-ants outline (sub-unit dashes on the document
    /// grid), so the frame boundaries read on the canvas regardless of the
    /// active tool. The solid under-baseline uses
    /// [`ThemeColors::frame_region_ant_under`] and the animated dashes use
    /// [`ThemeColors::frame_region_ant`]. Painter-only and non-interactive — it
    /// never captures or alters the pointer. Emitted by the App shell for the
    /// active animation's regions. An empty list paints nothing.
    FrameRegions(Vec<Rect2i>),
    /// Tile-placer cursor preview (UX items 1/4/6): the hovered grid cell's
    /// green outline, the REAL flip+rotated preview pixels (at the hovered cell
    /// AND at every cell of the in-flight gesture), and the green tile_id labels
    /// of every assigned cell. Only emitted while the Tile tool is active, so it
    /// never overlaps the gizmo/selection overlays.
    ///
    /// Z-ORDER: the whole variant is painted in a DEDICATED final pass
    /// ([`paint_tile_placer_overlay`], the last thing `ui` paints), so the
    /// preview always lands ON TOP of the baked tile content — nothing the
    /// canvas draws afterwards can bury it.
    TilePlacer {
        /// The hovered grid cell `(cx, cy)`, when the pointer is over the
        /// canvas and a tile is selected. `None` (no outline) otherwise.
        hover_cell: Option<(u32, u32)>,
        /// Every grid cell the in-flight placer gesture has stamped/cleared.
        /// Each gets a green outline of its own, so a drag shows the whole
        /// painted trail rather than only the pointer's cell. Empty when no
        /// gesture is active.
        gesture_cells: Vec<(u32, u32)>,
        /// The oriented tile's OPAQUE pixels in CANVAS coordinates with their
        /// straight-alpha RGBA colour — the REAL tile content to be placed
        /// (flip/rotation applied; transparent pixels omitted). Covers the
        /// hovered cell and every `gesture_cells` entry. Empty when there is
        /// nothing to preview (no tile selected, or an empty tile).
        preview_pixels: Vec<(i32, i32, Color)>,
        /// Assigned cells: `(cell_x, cell_y)` + the placed tile's id, for the
        /// green index labels at each cell's top-left.
        indices: Vec<((u32, u32), u64)>,
    },
    /// Transform gizmo overlay (R4, D57): floating lifted pixels (D32) beneath
    /// the quad outline, then the scale/rotate/centre handles. The quad corners
    /// come from `TransformObject::canvas_corners`, so the gizmo rotates with
    /// the content. `hovered` is the handle hit-tested under the pointer this
    /// frame.
    Gizmo {
        corners: [(f32, f32); 4],
        pivot: (f32, f32),
        hovered: GizmoHit,
        /// The handle currently grabbed, if any (item 6). The App populates it
        /// from the live drag and the widget passes it to `gizmo::paint`, which
        /// gives the active handle visual precedence over hover.
        active: Option<GizmoHit>,
        /// Floating lifted pixels: `(texture, canvas rect)`. `None` hides the
        /// preview quad before the first texture upload.
        preview: Option<(egui::TextureId, Rect2i)>,
        /// Live drag HUD (item 5): size / angle / position readout shown near
        /// the pointer while a transform handle is grabbed.
        hud: Option<HudState>,
    },
    /// Dynamic-line curve transform: the flattened spline (canvas-space float
    /// points), the integer samples a commit rasterizes, and the four
    /// draggable gizmos. `hovered` is the gizmo index under the pointer this
    /// frame, if any.
    Curve {
        points: Vec<(f32, f32)>,
        /// The integer canvas points the committed stroke stamps along the
        /// spline — the same samples
        /// [`crate::core::transform::CurveTransform::samples`] feeds the stroke
        /// machinery at commit. The preview builds its footprint cells from
        /// these, so what is shown is what will land.
        samples: Vec<(i32, i32)>,
        gizmos: [(f32, f32); 4],
        hovered: Option<usize>,
    },
    /// Animation-placer boundary highlights (Phase 3). While the
    /// [`AnimationPlacer`](crate::input::Tool::AnimationPlacer) tool is active:
    /// the FIRST frame index's region boundary (red), the LAST frame index's
    /// region boundary (green), the whole indexed area's boundary (grey), plus
    /// the live in-flight rectangle while a drag is defining a new area.
    ///
    /// Painted in the same dedicated final pass as the tile-placer overlay, so
    /// it always sits on top of the canvas content and every other decoration.
    AnimationPlacer {
        /// Boundary of the FIRST frame (index 0) of the active animation.
        first: Rect2i,
        /// Boundary of the LAST frame of the active animation.
        last: Rect2i,
        /// Boundary of the whole indexed area (the union of every frame region).
        area: Rect2i,
        /// The live rectangle being dragged, in canvas pixels; `None` when no
        /// rectangle gesture is in flight.
        in_flight: Option<Rect2i>,
    },
}

/// Live LINE-mode preview geometry, in canvas pixels: the press anchor and the
/// current (possibly Ctrl-snapped) end point. Preview only — the App commits
/// the real stroke on release.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePreview {
    pub anchor: (i32, i32),
    pub end: (i32, i32),
}

/// Per-frame view inputs the canvas needs besides the camera.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasView {
    /// Canvas dimensions in pixels.
    pub canvas_size: (u32, u32),
    /// Whether the region (tile) grid is painted.
    pub grid_visible: bool,
    /// Region/tile size in pixels, PER AXIS: grid lines land on multiples of
    /// `w` along x and `h` along y.
    pub tile_size: TileSize,
    /// Brush footprint previewed under the cursor (Draw tool); `None` hides it.
    pub brush: Option<BrushSpec>,
    /// Draw mode for the cursor preview style: Pen paints the footprint as
    /// solid cells in [`Self::brush_color`], Eraser paints a hollow outline.
    pub draw_mode: DrawMode,
    /// The primary color the Pen preview is filled with.
    pub brush_color: Color,
    /// Live LINE-mode anchor→end preview; `None` outside a line gesture.
    pub line: Option<LinePreview>,
    /// The committed selection as a pixel clip, or `None` when nothing is
    /// selected. Every footprint preview clips its cells to it, so a preview
    /// only ever shows cells a commit will actually write.
    pub selection_clip: Option<PixelClip>,
    /// Whether a selection drag gesture is live. The drag then follows the
    /// clamped pointer position, so a pointer that leaves the window keeps
    /// extending the selection (clamped at the canvas edge) instead of freezing.
    pub selection_drag_active: bool,
    /// Whether a floating SELECTION transform is the live gesture. The drag
    /// then follows the CLAMP-FREE pointer position
    /// ([`CanvasWidget::screen_to_canvas_unclamped`]), so the object can be
    /// moved/resized completely outside the canvas bounds. `selection_drag_active`
    /// keeps its clamped behavior; the two are mutually exclusive.
    pub transform_drag_active: bool,
    /// The visible canvas pixel under the pointer this frame, sampled by the
    /// App from the composited canvas. The live [`CanvasOverlay::Marquee`]
    /// outline inverts it per channel (`255 - channel`) so the plain solid box
    /// stays legible whatever the artwork underneath; `None` when the pointer
    /// is off-canvas, which falls back to the theme's live-outline stroke.
    pub marquee_probe: Option<Color>,
    /// The composited-canvas color at a canvas point, sampled by the App from
    /// the active layer's buffer (same mechanism as [`Self::marquee_probe`]).
    /// The mask marching-ants border previously blended this sample into its
    /// stroke; the border is now fixed black + white (see [`paint_ants_mask`]),
    /// so the field is retained only for the App-side sampling contract.
    /// `None` when the App hasn't sampled (tests, pointer off-canvas).
    pub canvas_color_at: Option<Color>,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self {
            canvas_size: (0, 0),
            grid_visible: true,
            tile_size: TileSize::square(16),
            brush: None,
            draw_mode: DrawMode::Pen,
            brush_color: Color::BLACK,
            line: None,
            selection_clip: None,
            selection_drag_active: false,
            transform_drag_active: false,
            marquee_probe: None,
            canvas_color_at: None,
        }
    }
}

/// Interactive canvas viewport: renders a texture through the app [`Camera`].
///
/// Pure egui — the texture is referenced by [`egui::TextureId`] only; the App
/// shell owns the actual `TextureHandle`.
/// BB: a latched selection-button press awaiting a small dead-zone move.
///
/// Owned by the canvas widget (not the App) so the App field-ownership
/// inventory stays unchanged. `screen` is the global press position (for the
/// dead-zone comparison), `point` the press projected+clamped into canvas
/// pixels (the gesture anchor), and `started` whether the gesture has been
/// emitted yet (one-shot).
#[derive(Clone, Copy, Default)]
pub(crate) struct SelectionLatch {
    pub(crate) screen: egui::Pos2,
    pub(crate) point: (i32, i32),
    pub(crate) started: bool,
}

pub struct CanvasWidget {
    overlay: CanvasOverlay,
    /// The shared [`InputCapture`] surface this widget routes through.
    ///
    /// The App owns one instance for the main canvas (`SurfaceId::Canvas`); a
    /// second viewport hosted inside a dock panel (the Frame Edit panel) owns
    /// its own instance and sets this to that panel's surface, so the gesture
    /// router sees the panel as the topmost surface without the widget masking
    /// itself via [`InputCapture::pointer_over_panel`].
    surface: SurfaceId,
    /// When `Some`, this widget displays ONLY this sub-rect of the sheet
    /// texture (the Frame Edit viewport); the full sheet size is still the
    /// per-frame `CanvasView::canvas_size`, used for the texture UV mapping.
    /// `None` means the whole canvas (the main canvas), unchanged.
    region: Option<Rect2i>,
    last_origin: egui::Pos2,
    gizmo_hovered: GizmoHit,
    /// The last canvas point a selection drag clamped to, replayed while
    /// the interact position is unavailable so the gesture holds at the edge
    /// instead of freezing. Cleared when the drag stops.
    last_clamped_drag_point: Option<(i32, i32)>,
    /// The last (possibly out-of-bounds) canvas point a floating-transform drag
    /// mapped to, replayed while the interact position is unavailable. Cleared
    /// when the drag stops.
    last_transform_drag_point: Option<(i32, i32)>,
    /// BB: the latched primary-button selection press awaiting the dead-zone
    /// move (`None` when no primary selection gesture is latched/active).
    pub(crate) selection_primary_latch: Option<SelectionLatch>,
    /// BB: the latched secondary-button selection press; the Rectangle/Lasso
    /// Subtract path.
    pub(crate) selection_secondary_latch: Option<SelectionLatch>,
    /// BB: whether a secondary selection gesture is down (latched or started),
    /// so its release emits `eyedropper_ended` exactly once.
    pub(crate) selection_secondary_armed: bool,
    /// D78/FF: modifiers latched on the primary-press frame, carried to the
    /// click/double-click frames via [`CanvasInteractions::click_modifiers`].
    /// `None` until the first primary press over the draw rect.
    click_press_modifiers: Option<egui::Modifiers>,
    /// HH: memoised mask-boundary polylines for the incoming overlay's boundary
    /// segments. `paint_ants_mask` needs the chained loops to walk the
    /// document-pixel edges ([`pixel_ant_edges_from_polylines`]), so it no
    /// longer re-chains the boundary per frame. Keyed on the exact segment
    /// list, so it is recomputed only when the selection boundary actually
    /// changes.
    ants_polylines: Option<(Vec<BoundarySegment>, Vec<Vec<(f32, f32)>>)>,
    /// Cached seamless 16×16 pale sand-grey "X" + dither canvas backdrop.
    sand_bg_texture: Option<egui::TextureHandle>,
    /// Sub-pixel pan remainder accumulated across slow drag frames so small
    /// deltas (< 0.5 px) are never dropped.
    pan_remainder: egui::Vec2,
    /// Optional canvas-space caret line `(top, bottom)` during Text Transform.
    text_caret: Option<((f32, f32), (f32, f32))>,
}

/// GG: order-independent union of two [`egui::Modifiers`] snapshots.
///
/// The Wand's click decision must be the same whether Alt or Shift was pressed
/// first. A single `Modifiers` is the full held set, but the press-time latch
/// and the click-time held state can each be missing whichever modifier was
/// reported later, so the click uses their union.
fn union_modifiers(a: egui::Modifiers, b: egui::Modifiers) -> egui::Modifiers {
    egui::Modifiers {
        alt: a.alt || b.alt,
        ctrl: a.ctrl || b.ctrl,
        shift: a.shift || b.shift,
        mac_cmd: a.mac_cmd || b.mac_cmd,
        command: a.command || b.command,
    }
}

impl CanvasWidget {
    /// New widget: 100% zoom, pan (0, 0).
    pub fn new() -> Self {
        Self {
            overlay: CanvasOverlay::None,
            surface: SurfaceId::Canvas,
            region: None,
            last_origin: egui::Pos2::ZERO,
            gizmo_hovered: GizmoHit::None,
            last_clamped_drag_point: None,
            last_transform_drag_point: None,
            selection_primary_latch: None,
            selection_secondary_latch: None,
            selection_secondary_armed: false,
            click_press_modifiers: None,
            ants_polylines: None,
            sand_bg_texture: None,
            pan_remainder: egui::Vec2::ZERO,
            text_caret: None,
        }
    }

    pub fn set_text_caret(&mut self, caret: Option<((f32, f32), (f32, f32))>) {
        self.text_caret = caret;
    }

    /// Lazily builds and caches the 24×24 seamless diagonal accordion-stripe
    /// backdrop texture (low-contrast ergonomic light grey wide bands with
    /// thinner darker grey bands, screen-locked to avoid shimmering on pan/zoom).
    fn ensure_sand_bg_texture(&mut self, ctx: &egui::Context) -> egui::TextureId {
        if let Some(tex) = &self.sand_bg_texture {
            return tex.id();
        }
        const SIZE: usize = 24;
        // Soft, low-contrast ergonomic greys: wide light band (16px) + thinner darker band (8px).
        const LIGHT_STRIPE: [u8; 4] = [126, 126, 130, 255];
        const DARK_STRIPE: [u8; 4] = [115, 115, 119, 255];
        const LIGHT_BAND_WIDTH: usize = 16;

        let mut rgba = vec![0u8; SIZE * SIZE * 4];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let diag = (x + y) % SIZE;
                let px = if diag < LIGHT_BAND_WIDTH {
                    LIGHT_STRIPE
                } else {
                    DARK_STRIPE
                };
                let i = (y * SIZE + x) * 4;
                rgba[i..i + 4].copy_from_slice(&px);
            }
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &rgba);
        let tex = ctx.load_texture(
            "canvas_diagonal_stripe_bg",
            image,
            egui::TextureOptions {
                magnification: egui::TextureFilter::Linear,
                minification: egui::TextureFilter::Linear,
                wrap_mode: egui::TextureWrapMode::Repeat,
                ..Default::default()
            },
        );
        let id = tex.id();
        self.sand_bg_texture = Some(tex);
        id
    }

    /// The [`SurfaceId`] this widget routes through the shared [`InputCapture`].
    ///
    /// Defaults to [`SurfaceId::Canvas`] (the main canvas). A second viewport
    /// hosted inside a dock panel sets it to that panel's surface so the router
    /// treats the panel as the topmost surface while the widget still processes
    /// the gesture it owns.
    pub fn set_surface(&mut self, surface: SurfaceId) {
        self.surface = surface;
    }

    /// Display ONLY `region` of the sheet texture (the Frame Edit viewport), or
    /// the whole canvas when `region` is `None` (the main canvas).
    ///
    /// The sheet's full size stays the per-frame `CanvasView::canvas_size`: the
    /// widget uses it for the texture UV mapping, while all drawing, mapping
    /// and clipping are scoped to `region`. The region's top-left becomes the
    /// camera's local origin, so the frame-edit camera pans/zooms over the
    /// region only. See [`Self::view_rect`] / [`Self::content_origin`].
    pub fn set_sheet_region(&mut self, region: Option<Rect2i>) {
        self.region = region.filter(|r| !r.is_empty());
    }

    /// The sub-rect currently displayed, if any (`None` = whole canvas).
    pub fn sheet_region(&self) -> Option<Rect2i> {
        self.region
    }

    /// The canvas coordinate at the viewport's local origin: the region's
    /// top-left when a region is set, otherwise `(0, 0)`.
    fn view_min(&self) -> (i32, i32) {
        self.region.map_or((0, 0), |region| (region.x, region.y))
    }

    /// The view extent in canvas pixels: the region's size when set, otherwise
    /// the full `canvas_size`.
    fn view_size(&self, canvas_size: (u32, u32)) -> (u32, u32) {
        self.region.map_or(canvas_size, |region| {
            (region.w.max(0) as u32, region.h.max(0) as u32)
        })
    }

    /// The canvas-pixel rect the viewport displays (region or full canvas).
    fn view_rect(&self, canvas_size: (u32, u32)) -> Rect2i {
        self.region.unwrap_or_else(|| {
            Rect2i::new(0, 0, canvas_size.0 as i32, canvas_size.1 as i32)
        })
    }

    /// Screen position of the absolute canvas origin `(0, 0)`: the camera's
    /// local pan puts the displayed rect's top-left at `last_origin + pan`, so
    /// the absolute origin is offset back by the region's top-left.
    fn content_origin(&self, camera: Camera) -> egui::Pos2 {
        let (vx, vy) = self.view_min();
        let scale = camera.scale() as f32;
        let pan = camera.pan();
        egui::pos2(
            self.last_origin.x + pan.0 as f32 - vx as f32 * scale,
            self.last_origin.y + pan.1 as f32 - vy as f32 * scale,
        )
    }

    /// Add the viewport's local origin to a mapping result so it becomes an
    /// ABSOLUTE canvas coordinate.
    fn local_to_canvas(&self, local: (i32, i32)) -> (i32, i32) {
        let (vx, vy) = self.view_min();
        (local.0 + vx, local.1 + vy)
    }

    /// Overlay to paint on top of the canvas (set by the App shell each frame).
    pub fn set_overlay(&mut self, overlay: CanvasOverlay) {
        // HH: chain the mask boundary's polylines ONCE here (per selection
        // revision) instead of inside `paint_ants_mask` every frame. The
        // per-frame cost of `mask_polylines`/`chain_component_loops` grows with
        // the boundary, so re-running it each frame was the camera "crunch".
        // Task II: the baseline and the dashes now both read these cached
        // polylines, so nothing re-chains per frame.
        if let Some(segments) = overlay_mask_segments(&overlay) {
            let stale = self
                .ants_polylines
                .as_ref()
                .is_none_or(|(key, _)| key.as_slice() != segments);
            if stale {
                let polylines = mask_polylines(segments);
                self.ants_polylines = Some((segments.to_vec(), polylines));
            }
        }
        self.overlay = overlay;
    }

    /// HH: the memoised polylines for the overlay's boundary, when it has one.
    /// `set_overlay` guarantees they match the currently assigned overlay.
    fn cached_ants_polylines(&self) -> &[Vec<(f32, f32)>] {
        self.ants_polylines
            .as_ref()
            .map_or(&[], |(_, polylines)| polylines.as_slice())
    }

    /// The currently assigned canvas overlay.
    pub fn overlay(&self) -> CanvasOverlay {
        self.overlay.clone()
    }

    /// The gizmo handle under the pointer, computed on the last frame.
    pub fn gizmo_hovered(&self) -> GizmoHit {
        self.gizmo_hovered
    }

    /// Hit-test the transform gizmo at a SCREEN position through this widget's
    /// last-known origin and the camera's pan/scale — the exact mapping
    /// [`CanvasWidget::ui`] feeds to [`crate::render::gizmo::hit_test`].
    ///
    /// Exposed for the App's global-pointer transform path: a press can land
    /// on a handle that lies OUTSIDE the canvas draw rect (letterbox or beyond
    /// the widget), where the widget-scoped `primary_response` never sees it,
    /// so the App cannot rely on the widget's own hover/hit reporting there.
    /// The gizmo is a rotated quad, so `corners` are the object's four
    /// canvas-space corners (`TransformObject::canvas_corners`).
    pub fn gizmo_hit_at(
        &self,
        screen_pos: egui::Pos2,
        corners: [(f32, f32); 4],
        camera: Camera,
    ) -> GizmoHit {
        crate::render::gizmo::hit_test(
            screen_pos,
            corners,
            self.last_origin,
            camera.pan(),
            camera.scale() as f32,
        )
    }

    /// Screen position for a canvas-pixel point, mirroring [`canvas_draw_rect`]
    /// math exactly: canvas coord `c` spans screen pixels `[pan + c*scale,
    /// pan + (c+1)*scale)`, offset by the widget origin captured on the last
    /// frame.
    ///
    /// Region-aware: with a sub-rect set, the absolute canvas origin is shifted
    /// by the region's top-left (see [`Self::content_origin`]), so an absolute
    /// canvas coordinate always maps to the correct screen position.
    fn canvas_point_to_screen(&self, camera: Camera, point: (i32, i32)) -> egui::Pos2 {
        let scale = camera.scale() as f32;
        let origin = self.content_origin(camera);
        egui::pos2(
            origin.x + point.0 as f32 * scale,
            origin.y + point.1 as f32 * scale,
        )
    }

    /// Screen rect for a canvas-pixel rect, mirroring [`canvas_draw_rect`] math
    /// exactly: canvas coord `c` spans screen pixels `[pan + c*scale, pan + (c+1)*scale)`,
    /// offset by the widget origin captured on the last frame.
    pub fn canvas_rect_to_screen(&self, camera: Camera, rect: Rect2i) -> egui::Rect {
        let scale = camera.scale() as f32;
        let min = self.canvas_point_to_screen(camera, (rect.x, rect.y));
        let size = egui::vec2(rect.w as f32 * scale, rect.h as f32 * scale);
        egui::Rect::from_min_size(min, size)
    }

    /// Paint the canvas texture and process pointer interactions for one frame.
    ///
    /// * Primary drag → stroke begin/continue/end.
    /// * Middle drag → viewport pan.
    /// * Plain scroll / pinch → anchored zoom.
    /// * Shift+scroll → brush size; Ctrl+scroll → brush shape (reported to the App).
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        capture: &InputCapture,
        image_id: egui::TextureId,
        view: CanvasView,
        mut camera: Camera,
    ) -> CanvasInteractions {
        let CanvasView {
            canvas_size,
            grid_visible,
            tile_size,
            brush,
            draw_mode,
            brush_color,
            line,
            selection_clip,
            selection_drag_active,
            transform_drag_active,
            marquee_probe,
            canvas_color_at: _,
        } = view;
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        let origin = response.rect.min;
        self.last_origin = origin;
        // Region scoping: when a sub-rect is set the viewport, its clip, the
        // grid and every mapping are relative to the region; otherwise these
        // are the full canvas and the main-canvas behaviour is unchanged.
        let view_size = self.view_size(canvas_size);
        let view_rect = self.view_rect(canvas_size);
        // Region: the fallback clip is the region itself; full canvas keeps the
        // historical whole-canvas clip exactly.
        let canvas_clip = if self.region.is_some() {
            (!view_rect.is_empty()).then(|| PixelClip::from_rect(view_rect))
        } else {
            canvas_rect_clip(canvas_size)
        };
        let preview_clip: Option<&PixelClip> = selection_clip.as_ref().or(canvas_clip.as_ref());
        let render_camera = camera;
        let scale = render_camera.scale() as f32;
        let pan = render_camera.pan();
        let draw_rect = self.canvas_rect_to_screen(render_camera, view_rect);
        // Primary/secondary button interactions are scoped to the canvas draw
        // rect: presses in the letterbox (or on widgets drawn above the canvas)
        // never claim the canvas gesture, so the App can arm a tool from
        // outside and a press on any other UI wins.
        let primary_response = ui.interact(
            draw_rect,
            response.id.with("canvas-primary"),
            egui::Sense::click_and_drag(),
        );

        let mut interactions = CanvasInteractions::default();

        // PANEL MASK: the App resolved the topmost surface under the pointer
        // once this frame (`InputCapture::set_target`, from the dock's own
        // `surface_at`) and routed wheel/middle-pan with it. Every canvas path
        // that reads the GLOBAL pointer instead of this widget's own response
        // must honour the same mask, or a press over panel chrome leaks to the
        // canvas underneath: the RAW press signal below, and the hover-driven
        // brush cursor / tile-placer preview painted further down.
        //
        // A widget whose OWN surface is the target is not masked by itself: the
        // second (Frame Edit) viewport lives inside a dock panel, so the panel
        // is the target while the viewport must still see its gesture.
        let pointer_over_panel = capture.pointer_over_panel() && !capture.is_target(self.surface);

        // RAW primary-press signal (see the field doc): a press over the canvas
        // draw rect reported straight from the egui pointer state, so the tile
        // placer stamps even when a DRAG-ONLY widget (a dock header band drawn
        // over the canvas) claims the press and swallows `clicked`/stroke.
        // egui 0.36 exposes `PointerState::press_origin()` for the press
        // position; fall back to the latest position for synthetic frames.
        // A press whose topmost surface is a PANEL never stamps: the panel
        // fully masks the canvas.
        let raw_primary_pressed = ui.input(|i| i.pointer.primary_pressed()) && !pointer_over_panel;
        if raw_primary_pressed {
            let press_pos = ui
                .input(|i| i.pointer.press_origin())
                .or_else(|| ui.input(|i| i.pointer.latest_pos()));
            // Fix 6: a raw press must be inside the widget's ACTUAL allocated
            // rect, not merely the mathematical draw rect. When the frame-edit
            // region is zoomed, the draw rect can extend over the panel's
            // control bar; requiring the allocated-rect containment keeps a
            // control-bar press from being reported as a canvas press.
            interactions.raw_pressed_over_canvas = press_pos
                .filter(|p| response.rect.contains(*p))
                .and_then(|p| screen_to_canvas_pt(p, origin, scale, pan, view_size))
                .map(|local| self.local_to_canvas(local));
        }

        // Paint the region of the texture (the full sheet when no region is
        // set), clipped to the viewport. With a region, clip to the region's
        // draw rect too so nothing (e.g. an off-region onion ghost) paints
        // outside it.
        let clip = if self.region.is_some() {
            draw_rect.intersect(response.rect)
        } else {
            response.rect
        };
        let painter = painter.with_clip_rect(clip);
        let uv = match self.region {
            Some(region) if canvas_size.0 > 0 && canvas_size.1 > 0 => egui::Rect::from_min_max(
                egui::pos2(
                    region.x as f32 / canvas_size.0 as f32,
                    region.y as f32 / canvas_size.1 as f32,
                ),
                egui::pos2(
                    region.right() as f32 / canvas_size.0 as f32,
                    region.bottom() as f32 / canvas_size.1 as f32,
                ),
            ),
            _ => egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        };
        let sand_id = self.ensure_sand_bg_texture(ui.ctx());
        let tile_px = 24.0_f32;
        // Lock UVs to screen coordinates so the backdrop never shimmers during pan/zoom.
        let sand_uv = egui::Rect::from_min_max(
            egui::pos2(draw_rect.left() / tile_px, draw_rect.top() / tile_px),
            egui::pos2(draw_rect.right() / tile_px, draw_rect.bottom() / tile_px),
        );
        painter.image(sand_id, draw_rect, sand_uv, crate::ui::theme::SKIN_TINT);
        painter.image(
            image_id,
            draw_rect,
            uv,
            egui::Color32::WHITE, // NB: neutral texture tint (multiply blend, not chrome)
        );
        painter.rect_stroke(
            draw_rect,
            0,
            egui::Stroke::new(1.0, theme.canvas_border32()),
            egui::StrokeKind::Inside,
        );
        let (vertical_lines, horizontal_lines) = tile_grid_lines(
            view_size,
            tile_size,
            scale,
            draw_rect,
            response.rect,
            grid_visible,
        );
        if !vertical_lines.is_empty() || !horizontal_lines.is_empty() {
            let grid_stroke = egui::Stroke::new(1.0, theme.canvas_border32());
            let top = draw_rect.top().max(response.rect.top());
            let bottom = draw_rect.bottom().min(response.rect.bottom());
            let left = draw_rect.left().max(response.rect.left());
            let right = draw_rect.right().min(response.rect.right());
            for x in vertical_lines {
                let screen_x = draw_rect.left() + x as f32 * scale;
                painter.line_segment(
                    [egui::pos2(screen_x, top), egui::pos2(screen_x, bottom)],
                    grid_stroke,
                );
            }
            for y in horizontal_lines {
                let screen_y = draw_rect.top() + y as f32 * scale;
                painter.line_segment(
                    [egui::pos2(left, screen_y), egui::pos2(right, screen_y)],
                    grid_stroke,
                );
            }
        }

        // The live selection gesture: a plain solid outline. No fill and no
        // dashes, so measuring a selection never hides the pixels under it.
        // Its stroke is the per-channel inverse of the App-sampled canvas
        // pixel under the pointer (D76), so the box stays visible on artwork
        // of any brightness; off-canvas falls back to the theme stroke.
        if let CanvasOverlay::Marquee(rect) = &self.overlay {
            let screen = self.canvas_rect_to_screen(camera, *rect);
            let stroke =
                marquee_probe.map_or_else(|| live_outline_color(theme), inverted_probe_color);
            paint_live_outline(&painter, screen, stroke);
        }

        // The live lasso trace: the traced path closed back to its start and
        // stroked as one plain solid polyline (D73). No fill and no dashes.
        if let CanvasOverlay::Lasso(points) = &self.overlay {
            if points.len() >= 2 {
                let screen: Vec<egui::Pos2> = points
                    .iter()
                    .map(|&point| self.canvas_point_to_screen(camera, point))
                    .collect();
                painter.add(egui::Shape::closed_line(
                    screen,
                    egui::Stroke::new(1.0, live_outline_color(theme)),
                ));
            }
        }

        // The live region-cell selection: the base selection's committed
        // marching ants first (when the gesture has one), then every swept
        // cell as a plain solid unit outline in the same live-gesture stroke
        // (D73). No fill.
        if let CanvasOverlay::RegionCells { cells, base } = &self.overlay {
            if !base.is_empty() {
                let time_ms = (ui.input(|i| i.time) * 1000.0) as u64;
                paint_ants_mask(
                    &painter,
                    theme,
                    self.last_origin,
                    scale,
                    pan,
                    self.cached_ants_polylines(),
                    time_ms,
                );
            }
            let outline = live_outline_color(theme);
            for &cell in cells {
                paint_live_outline(&painter, self.canvas_rect_to_screen(camera, cell), outline);
            }
        }

        // Paint the legacy rectangular selection overlay. No translucent fill
        // (D76): a committed selection must never cover the pixels it measures.
        // The App no longer emits this variant for committed selections (they
        // all go through `AntsMask`), but keep it as a thin plain outline so a
        // direct `set_overlay` still reads correctly.
        if let CanvasOverlay::Selection(rect) = &self.overlay {
            let time_ms = (ui.input(|i| i.time) * 1000.0) as u64;
            let color = ant_stroke_color(theme.marching_ants_core(), time_ms);
            let ant_color =
                egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
            let color = ant_under_stroke_color(theme.marching_ants_under_core());
            let under_color =
                egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
            paint_ants_under_rect(&painter, self.last_origin, scale, pan, *rect, under_color);
            paint_ants_rect(
                &painter,
                self.last_origin,
                scale,
                pan,
                *rect,
                ant_color,
                time_ms,
            );
        }

        // Paint the onion-skin ghosts: tinted textured quads sampling the
        // canvas texture, with the tint's alpha reduced to ~36% so the
        // already-drawn canvas shows through.
        if let CanvasOverlay::Onion(ghosts) = &self.overlay {
            for ghost in ghosts {
                let screen = self.canvas_rect_to_screen(camera, ghost.rect);
                let uv_min = egui::pos2(
                    ghost.rect.x as f32 / canvas_size.0 as f32,
                    ghost.rect.y as f32 / canvas_size.1 as f32,
                );
                let uv_max = egui::pos2(
                    ghost.rect.right() as f32 / canvas_size.0 as f32,
                    ghost.rect.bottom() as f32 / canvas_size.1 as f32,
                );
                let tint = ghost.tint;
                let alpha = (tint.a as u32 * 90 / 255) as u8;
                // NB: dynamic per-frame tint from the ghost's OnionConfig.
                let color = egui::Color32::from_rgba_unmultiplied(tint.r, tint.g, tint.b, alpha);
                let mut mesh = egui::epaint::Mesh::with_texture(image_id);
                mesh.vertices = vec![
                    egui::epaint::Vertex {
                        pos: screen.min,
                        uv: uv_min,
                        color,
                    },
                    egui::epaint::Vertex {
                        pos: egui::pos2(screen.max.x, screen.min.y),
                        uv: egui::pos2(uv_max.x, uv_min.y),
                        color,
                    },
                    egui::epaint::Vertex {
                        pos: screen.max,
                        uv: uv_max,
                        color,
                    },
                    egui::epaint::Vertex {
                        pos: egui::pos2(screen.min.x, screen.max.y),
                        uv: egui::pos2(uv_min.x, uv_max.y),
                        color,
                    },
                ];
                mesh.indices = vec![0, 1, 2, 0, 2, 3];
                painter.add(egui::Shape::mesh(mesh));
            }
        }

        // Paint the marching-ants border: animated dashes on the region's
        // frame, transformed to screen space with the same math as the
        // selection overlay.  Only the dash phase advances; the stroke color is
        // the low-contrast theme token and never flickers.
        if let CanvasOverlay::Ants(rect) = &self.overlay {
            let time_ms = (ui.input(|i| i.time) * 1000.0) as u64;
            // Solid, time-invariant grey baseline first, then the dashes; only
            // the dash phase advances, so the baseline can never flicker (D76).
            let color = ant_under_stroke_color(theme.marching_ants_under_core());
            let under_color =
                egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
            paint_ants_under_rect(&painter, self.last_origin, scale, pan, *rect, under_color);
            let color = ant_stroke_color(theme.marching_ants_core(), time_ms);
            let ant_color =
                egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
            paint_ants_rect(
                &painter,
                self.last_origin,
                scale,
                pan,
                *rect,
                ant_color,
                time_ms,
            );
        }

        // Mask-shaped marching ants: the boundary unit segments of a
        // non-rectangular selection, dashed by the same slow, low-contrast
        // phase.
        if let CanvasOverlay::AntsMask(_) = &self.overlay {
            let time_ms = (ui.input(|i| i.time) * 1000.0) as u64;
            paint_ants_mask(
                &painter,
                theme,
                self.last_origin,
                scale,
                pan,
                self.cached_ants_polylines(),
                time_ms,
            );
        }

        // Selected animation's frame-region borders: a pixel-perfect
        // marching-ants outline per region, in the Frame Region tokens (bordeaux
        // under-baseline + live-yellow dashes). Painter-only, tool-independent.
        if let CanvasOverlay::FrameRegions(regions) = &self.overlay {
            let time_ms = (ui.input(|i| i.time) * 1000.0) as u64;
            paint_frame_regions(
                &painter,
                theme,
                self.last_origin,
                scale,
                pan,
                regions,
                time_ms,
            );
        }

        // NB: the Tile-placer preview is NOT painted here. It runs in a
        // dedicated FINAL pass at the end of `ui` so it always lands on top of
        // the baked tile content (see `paint_tile_placer_overlay`).

        // Transform gizmo overlay (R4, D57): floating lifted pixels (D32) as a
        // textured quad over the object bounds, then the (rotated) quad outline,
        // scale/rotate/centre handles. `hovered` is the hit-test result from the
        // App-provided overlay; the widget re-runs the hit test each frame for
        // its own cursor feedback.
        if let CanvasOverlay::Gizmo {
            corners,
            pivot,
            hovered,
            active,
            preview,
            hud,
        } = &self.overlay
        {
            if let Some((tex, rect)) = preview {
                let screen = self.canvas_rect_to_screen(camera, *rect);
                // L1-A visual mask: a floating transform may leave the canvas
                // bounds (functional constraint = none), but its PAINT must not
                // spill into the letterbox/chrome. Clip ONLY the preview quad
                // to the canvas draw rect; the bbox outline and handles stay
                // unclipped (widget-clipped) so they remain grabbable while the
                // object is partly or fully outside the canvas.
                painter.with_clip_rect(draw_rect).image(
                    *tex,
                    screen,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE, // NB: neutral texture tint (multiply blend, not chrome)
                );
            }
            // Compute the fresh hit test from THIS frame's pointer before
            // painting, so the hover highlight has no one-frame lag.
            let hovered_now = ui
                .input(|i| i.pointer.hover_pos())
                .map(|p| crate::render::gizmo::hit_test(p, *corners, origin, pan, scale))
                .unwrap_or(GizmoHit::None);
            let _ = hovered; // App-provided overlay hint; the widget hit-tests itself.
            let colors = theme.gizmo_colors();
            crate::render::gizmo::paint(
                &painter,
                *corners,
                *pivot,
                origin,
                pan,
                scale,
                hovered_now,
                *active,
                colors,
            );
            if let Some((c0, c1)) = self.text_caret {
                let time = ui.input(|i| i.time);
                if (time * 2.0) as i64 % 2 == 0 {
                    let s0 = egui::pos2(
                        origin.x + pan.0 as f32 + c0.0 * scale,
                        origin.y + pan.1 as f32 + c0.1 * scale,
                    );
                    let s1 = egui::pos2(
                        origin.x + pan.0 as f32 + c1.0 * scale,
                        origin.y + pan.1 as f32 + c1.1 * scale,
                    );
                    painter.line_segment(
                        [s0, s1],
                        egui::Stroke::new(2.0, theme.selection_stroke_color32()),
                    );
                }
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            }
            // Live drag HUD (item 5): a compact size / angle / position / scale
            // readout near the pointer while a handle is grabbed.
            if let Some(hud) = hud {
                let anchor = ui
                    .input(|i| i.pointer.hover_pos())
                    .unwrap_or(egui::pos2(origin.x, origin.y));
                let text = format!(
                    "W:{} H:{}  {:.0}°  ({}, {})  {:.2}×{:.2}",
                    hud.size.0,
                    hud.size.1,
                    hud.angle_deg,
                    hud.pos.0,
                    hud.pos.1,
                    hud.scale.0,
                    hud.scale.1
                );
                let text_pos = anchor + egui::vec2(14.0, 14.0);
                let galley =
                    painter.layout_no_wrap(text, egui::FontId::monospace(12.0), colors.outline);
                let bg_rect = egui::Rect::from_min_size(
                    text_pos - egui::vec2(4.0, 2.0),
                    galley.size() + egui::vec2(8.0, 4.0),
                );
                painter.rect_filled(bg_rect, 2.0, colors.fill_normal);
                painter.galley(text_pos, galley, colors.outline);
            }
            self.gizmo_hovered = hovered_now;
        } else {
            self.gizmo_hovered = GizmoHit::None;
        }

        // Dynamic-line curve overlay (Phase 2): the previewed footprint cells
        // (the pixels a double-click will commit), the flattened spline guide,
        // then the four draggable gizmos on top — the gizmos always win.
        if let CanvasOverlay::Curve {
            points,
            samples,
            gizmos,
            hovered,
        } = &self.overlay
        {
            let colors = theme.gizmo_colors();
            let origin_x = origin.x + pan.0 as f32;
            let origin_y = origin.y + pan.1 as f32;
            let to_screen = |point: (f32, f32)| {
                egui::pos2(origin_x + point.0 * scale, origin_y + point.1 * scale)
            };
            for segment in points.windows(2) {
                painter.line_segment(
                    [to_screen(segment[0]), to_screen(segment[1])],
                    egui::Stroke::new(1.5, colors.outline),
                );
            }
            if let Some(brush) = brush {
                let cells = footprint_cells(samples.iter().copied(), &brush.stamp_offsets());
                paint_footprint(
                    &painter,
                    theme,
                    &FootprintPreview {
                        cells: &cells,
                        mode: draw_mode,
                        color: brush_color,
                        origin: egui::pos2(origin_x, origin_y),
                        scale,
                        clip: preview_clip,
                    },
                );
            }
            for (index, gizmo) in gizmos.iter().enumerate() {
                let rect = egui::Rect::from_center_size(to_screen(*gizmo), egui::vec2(8.0, 8.0));
                let fill = if *hovered == Some(index) {
                    colors.fill_hover
                } else {
                    colors.fill_normal
                };
                painter.rect_filled(rect, 0.0, fill);
                painter.rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, colors.outline),
                    egui::StrokeKind::Inside,
                );
            }
        }

        // Primary drag → stroke. `drag_started` only fires after the pointer has
        // moved past the click threshold, so a plain click never starts a stroke
        // here; the `drag_delta` check additionally filters zero-length gestures.
        // A live selection drag follows the pointer clamped into the canvas,
        // so it keeps extending once the pointer leaves the draw rect; every
        // other tool uses the footprint anchor, so a drag that leaves the draw
        // rect keeps reporting while the brush footprint still covers canvas
        // pixels.
        let anchor_size = brush.map_or(1, |spec| spec.size);
        let mapping = DragMapping {
            clamped_selection: selection_drag_active,
            transform_drag: transform_drag_active,
            camera,
            canvas_size,
            origin,
            scale,
            pan,
            anchor_size,
        };
        if primary_response.drag_started_by(egui::PointerButton::Primary)
            && primary_response.drag_delta().length() > 0.0
        {
            if let Some(pt) = self.primary_drag_point(&primary_response, &mapping) {
                interactions.stroke_started = true;
                interactions.stroke_point = Some(pt);
            }
        } else if primary_response.dragged_by(egui::PointerButton::Primary) {
            if let Some(pt) = self.primary_drag_point(&primary_response, &mapping) {
                interactions.stroke_point = Some(pt);
            }
        }
        if primary_response.drag_stopped_by(egui::PointerButton::Primary) {
            interactions.stroke_ended = true;
            self.last_clamped_drag_point = None;
            self.last_transform_drag_point = None;
        }

        // D78/FF: latch the modifiers at the PRIMARY-press frame so a
        // click-driven tool (the Wand) sees Alt/Shift as they were when the
        // button went down — even when winit batches the modifier key-up into
        // the same redraw as the mouse-up. Prefer the held state but OR in the
        // press event's own modifiers, mirroring `alt_pressed`/`shift_pressed`.
        let primary_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
        if primary_pressed && primary_response.hovered() {
            self.click_press_modifiers = Some(ui.input(|i| {
                let mut mods = i.modifiers;
                if let Some(event_mods) = i.events.iter().rev().find_map(|event| match event {
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                        ..
                    } => Some(*modifiers),
                    _ => None,
                }) {
                    mods.alt |= event_mods.alt;
                    mods.shift |= event_mods.shift;
                    mods.ctrl |= event_mods.ctrl;
                    mods.command |= event_mods.command;
                }
                mods
            }));
        }

        // Primary click without drag → single-pixel interaction (e.g. fill,
        // select, or a one-shot pencil tap). `clicked()` is true only on the
        // release frame of a press-and-release within the click threshold, so
        // it never overlaps the drag-based stroke reporting above.
        let primary_clicked = primary_response.clicked();
        let primary_double_clicked = primary_response.double_clicked();
        if primary_clicked || primary_double_clicked {
            // GG: the Wand's click decision must not depend on the ORDER in
            // which Alt/Shift were pressed. The primary-press latch captures
            // the modifier set at the mouse-down, but a modifier reported
            // AFTER the press (winit batching, or holding the button while
            // adding the second modifier) would be missing there; likewise a
            // modifier released before the click is missing from the held
            // state. UNION the press-time latch with the click-time held state
            // (and the release event's own modifiers) so both modifiers apply
            // regardless of press order.
            let released_mods = ui.input(|i| {
                i.events.iter().rev().find_map(|event| match event {
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers,
                        ..
                    } => Some(*modifiers),
                    _ => None,
                })
            });
            let held_now = ui.input(|i| i.modifiers);
            let mut click_mods = held_now;
            if let Some(latched) = self.click_press_modifiers {
                click_mods = union_modifiers(click_mods, latched);
            }
            if let Some(released) = released_mods {
                click_mods = union_modifiers(click_mods, released);
            }
            if let Some(pos) = primary_response.interact_pointer_pos() {
                if let Some(pt) = screen_to_canvas_pt(pos, origin, scale, pan, canvas_size) {
                    if primary_clicked {
                        interactions.clicked = Some(pt);
                    }
                    if primary_double_clicked {
                        interactions.double_clicked = Some(pt);
                    }
                    interactions.click_modifiers = Some(click_mods);
                }
            }
            // The latch has served its click/double-click; drop it so a later
            // synthetic click cannot reuse a stale snapshot.
            self.click_press_modifiers = None;
        }

        // Secondary button (right) → temporary eyedropper: press/drag/release.
        // `drag_started_by`/`dragged_by`/`drag_stopped_by` cover the drag path;
        // the pointer button pressed/released queries cover a plain right-click
        // (press and release without moving), which never registers as a drag.
        let secondary_pressed =
            ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary));
        let secondary_released =
            ui.input(|i| i.pointer.button_released(egui::PointerButton::Secondary));
        if primary_response.drag_started_by(egui::PointerButton::Secondary)
            || (secondary_pressed && primary_response.hovered())
        {
            if let Some(pos) = primary_response.interact_pointer_pos() {
                if let Some(pt) = screen_to_canvas_pt(pos, origin, scale, pan, canvas_size) {
                    interactions.eyedropper_started = true;
                    interactions.eyedropper_point = Some(pt);
                }
            }
        } else if primary_response.dragged_by(egui::PointerButton::Secondary) {
            if let Some(pos) = primary_response.interact_pointer_pos() {
                if let Some(pt) = screen_to_canvas_pt(pos, origin, scale, pan, canvas_size) {
                    interactions.eyedropper_point = Some(pt);
                }
            }
        }
        if primary_response.drag_stopped_by(egui::PointerButton::Secondary)
            || (secondary_released && primary_response.hovered())
        {
            interactions.eyedropper_ended = true;
        }

        // Middle drag OR Space + Left drag → pan. Accumulate fractional pointer
        // deltas in `pan_remainder` so slow drags never feel sticky.
        let canvas_surface = self.surface;
        let middle_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle));
        let middle_released = ui.input(|i| i.pointer.button_released(egui::PointerButton::Middle));
        let space_held = !ui.ctx().text_edit_focused() && ui.input(|i| i.key_down(egui::Key::Space));
        let space_pan_pressed = space_held && ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
        let space_pan_active = space_held && ui.input(|i| i.pointer.primary_down())
            && capture.handles_buttons(canvas_surface, space_pan_pressed);
        if middle_released && capture.is_owner(canvas_surface) {
            capture.release();
        }
        let middle_panning = ui.input(|i| i.pointer.middle_down())
            && capture.handles_buttons(canvas_surface, middle_pressed);
        if space_held && !pointer_over_panel {
            ui.ctx().set_cursor_icon(if space_pan_active {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        if space_pan_active {
            interactions.stroke_started = false;
            interactions.stroke_segment_started = false;
            interactions.stroke_point = None;
            interactions.stroke_ended = false;
            interactions.clicked = None;
            interactions.double_clicked = None;
            interactions.raw_pressed_over_canvas = None;
        }
        if middle_panning || space_pan_active {
            let delta = ui.input(|i| i.pointer.delta());
            self.pan_remainder += delta;
            let dx = self.pan_remainder.x.trunc() as i32;
            let dy = self.pan_remainder.y.trunc() as i32;
            self.pan_remainder.x -= dx as f32;
            self.pan_remainder.y -= dy as f32;
            if dx != 0 || dy != 0 {
                camera.pan_by(dx, dy);
                interactions.pan_by = (dx, dy);
            }
        } else {
            self.pan_remainder = egui::Vec2::ZERO;
        }

        // Wheel handling, routed to the topmost surface: Shift+scroll resizes
        // the Draw tool's brush (reported to the App), Ctrl+scroll cycles the
        // Draw tool's brush shape (reported to the App), Alt+scroll adjusts the
        // Draw tool's scatter (reported to the App), plain scroll and pinch
        // zoom freely.
        if capture.handles_wheel(canvas_surface) {
            if let Some(pointer_pos) = ui.input(|i| i.pointer.latest_pos()) {
                let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
                let zoom_delta = ui.input(|i| i.zoom_delta());
                if shift_wheel(ui) {
                    // egui redirects Shift+wheel into the horizontal scroll
                    // axis, so the raw wheel events are the only place the
                    // brush scroll survives.
                    interactions.brush_scroll = ui.input(|input| {
                        input
                            .events
                            .iter()
                            .filter_map(|event| match event {
                                egui::Event::MouseWheel {
                                    delta, modifiers, ..
                                } if modifiers.shift => Some(delta.y),
                                _ => None,
                            })
                            .sum()
                    });
                } else if ctrl_wheel(ui) {
                    // Ctrl+scroll cycles the Draw tool's brush shape. The raw
                    // wheel events are the only reliable source of the Ctrl
                    // modifier; each notch maps to an integer cycle step.
                    interactions.shape_cycle = ui.input(|input| {
                        input
                            .events
                            .iter()
                            .filter_map(|event| match event {
                                egui::Event::MouseWheel {
                                    delta, modifiers, ..
                                } if modifiers.ctrl => Some(scroll_steps(delta.y)),
                                _ => None,
                            })
                            .sum()
                    });
                } else if alt_wheel(ui) {
                    // Alt+scroll adjusts the Draw tool's scatter amount. The
                    // raw wheel events are the only reliable source of the Alt
                    // modifier; the App maps the summed delta to scatter steps.
                    interactions.scatter_scroll = ui.input(|input| {
                        input
                            .events
                            .iter()
                            .filter_map(|event| match event {
                                egui::Event::MouseWheel {
                                    delta, modifiers, ..
                                } if modifiers.alt => Some(delta.y),
                                _ => None,
                            })
                            .sum()
                    });
                } else {
                    let factor = if zoom_delta != 1.0 {
                        zoom_delta
                    } else if scroll_y != 0.0 {
                        scroll_zoom_factor(scroll_y)
                    } else {
                        1.0
                    };
                    if factor != 1.0 {
                        let new_zoom = (camera.zoom_percent() as f32 * factor).round() as i32;
                        let new_zoom = new_zoom.clamp(
                            Camera::MIN_ZOOM_PERCENT as i32,
                            Camera::MAX_ZOOM_PERCENT as i32,
                        ) as u32;
                        if let Some(zoom) =
                            Self::zoom_to(&mut camera, new_zoom, pointer_pos, origin)
                        {
                            interactions.zoom = Some(zoom);
                        }
                    }
                }
            }
        }

        // Brush cursor: the footprint the Draw tool will paint, following the
        // pointer like Pyxel Edit's pixel cursor. Pen paints the footprint as
        // solid cells in the primary color; Eraser paints the hollow outline
        // (the pen's old look).
        //
        // A curve transform session is the only producer of the `Curve`
        // overlay, and while it is active its preview owns the canvas: neither
        // the brush cursor nor the line preview may paint over its gizmos.
        //
        // The cursor is HOVER-driven (it follows the pointer without any button
        // held), so it is masked the same way a press is: a palette / toolbox /
        // timeline panel covers the pointer and must not show a canvas cursor
        // underneath it. Only the pure-hover case is gated — an in-flight line
        // preview below still tracks its gesture across a panel so the pending
        // stroke stays readable.
        let transform_phase = matches!(self.overlay, CanvasOverlay::Curve { .. });
        if let Some(brush) = brush.filter(|_| !transform_phase && !pointer_over_panel) {
            if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                if let Some((cx, cy)) =
                    canvas_footprint_anchor(pos, origin, scale, pan, view_size, brush.size)
                        .map(|local| self.local_to_canvas(local))
                {
                    let cells = footprint_cells(std::iter::once((cx, cy)), &brush.stamp_offsets());
                    paint_footprint(
                        &painter,
                        theme,
                        &FootprintPreview {
                            cells: &cells,
                            mode: draw_mode,
                            color: brush_color,
                            origin: self.content_origin(render_camera),
                            scale,
                            clip: selection_clip.as_ref(),
                        },
                    );
                }
            }
        }

        // LINE-mode preview: the anchor→end segment stamped with the active
        // brush footprint, clipped to the canvas. Nothing here is committed —
        // the App paints the real stroke on release.
        let line = line.filter(|_| !transform_phase);
        if let (Some(line), Some(brush)) = (line, brush) {
            let cells = footprint_cells(
                bresenham_line(line.anchor, line.end),
                &brush.stamp_offsets(),
            );
            paint_footprint(
                &painter,
                theme,
                &FootprintPreview {
                    cells: &cells,
                    mode: draw_mode,
                    color: brush_color,
                    origin: self.content_origin(render_camera),
                    scale,
                    // The pen preview clips its cells to the selection, or to
                    // the canvas when nothing is selected; the eraser
                    // silhouette stays window-clipped by the painter unless a
                    // selection is active.
                    clip: match draw_mode {
                        DrawMode::Pen => preview_clip,
                        DrawMode::Eraser => selection_clip.as_ref(),
                    },
                },
            );
        }

        // Tile-placer preview — the LAST thing this frame paints (item 1/2/4/6).
        // A dedicated final pass is what GUARANTEES the preview reads on top of
        // the tile's baked pixels: the overlay used to be painted mid-pass,
        // where anything drawn afterwards (footprints, line/gizmo chrome) could
        // bury it under the tile content it is meant to describe.
        self.paint_tile_placer_overlay(&painter, theme, render_camera, tile_size, scale);
        // Animation-placer boundary highlights share the same final pass, so the
        // red/green/grey region outlines and the live in-flight rectangle read
        // above the canvas content too.
        self.paint_animation_placer_overlay(&painter, theme, render_camera);
        // Frame-edit onion overlay viewports, painted in the same final pass so
        // the translucent neighbour regions sit ON TOP of the base frame-edit
        // content.
        self.paint_onion_overlay(&painter, theme, render_camera, image_id, canvas_size);

        interactions.primary_down_on_canvas = primary_response.is_pointer_button_down_on();
        interactions.updated_camera = camera;
        interactions
    }

    /// Paints the Tile-placer cursor preview (UX items 1/2/4/6): the green
    /// outlines of every previewed grid cell, the REAL flip+rotated tile pixels
    /// over each of them, and the green tile_id labels of every assigned cell.
    ///
    /// Called as the FINAL pass of [`Self::ui`] so the preview always sits above
    /// the canvas texture and every other decoration — the tile placer must be
    /// readable whatever is already baked under the pointer.
    ///
    /// Item 2: the outline/pixels cover the hovered cell AND every cell of the
    /// in-flight gesture, so a drag previews the tile at each cell it paints
    /// rather than only under the pointer.
    fn paint_tile_placer_overlay(
        &self,
        painter: &egui::Painter,
        theme: &ThemeColors,
        camera: Camera,
        tile_size: TileSize,
        scale: f32,
    ) {
        let CanvasOverlay::TilePlacer {
            hover_cell,
            gesture_cells,
            preview_pixels,
            indices,
        } = &self.overlay
        else {
            return;
        };
        // The tile-placer green: the theme's green token. `onion_next_tint` is
        // the green accent of the default dark theme (no hard-coded chrome
        // color — everything routes through ThemeColors).
        let green = theme.onion_next_tint32();
        let cell_rect = |cx: u32, cy: u32| {
            Rect2i::new(
                cx.saturating_mul(tile_size.w) as i32,
                cy.saturating_mul(tile_size.h) as i32,
                tile_size.w as i32,
                tile_size.h as i32,
            )
        };
        // Item 2: every gesture cell gets its own outline, so the whole painted
        // trail of the drag is marked — not just the pointer's cell. Deduped
        // against the hover cell, which is drawn right after.
        let mut outlined: Vec<(u32, u32)> = gesture_cells.clone();
        if let Some(cell) = hover_cell {
            if !outlined.contains(cell) {
                outlined.push(*cell);
            }
        }
        // Item 1: each cell's boundary, as a plain solid outline (no fill, no
        // dashes), like the live-gesture boxes.
        for &(cx, cy) in &outlined {
            let screen = self.canvas_rect_to_screen(camera, cell_rect(cx, cy));
            paint_live_outline(painter, screen, green);
        }
        // Item 4: the REAL tile content over each previewed cell — every
        // oriented OPAQUE pixel is a filled quad in its own colour, so the
        // preview shows exactly what the stamp writes (transparency preserved:
        // transparent pixels are simply not painted). Same convention as the
        // Pen brush footprint preview.
        for &(cx, cy, color) in preview_pixels {
            let min = self.canvas_point_to_screen(camera, (cx, cy));
            // At least 1 pt per pixel, like the brush preview cells.
            let rect = egui::Rect::from_min_max(
                min,
                min + egui::vec2(scale, scale).max(egui::vec2(1.0, 1.0)),
            );
            let color32 = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
            painter.rect_filled(rect, 0.0, color32);
        }
        // Item 6: the green tile_id label at each assigned cell's top-left.
        for ((cx, cy), id) in indices {
            let cell = cell_rect(*cx, *cy);
            let top_left = self.canvas_point_to_screen(camera, (cell.x, cell.y));
            let text = format!("{id}");
            let galley = painter.layout_no_wrap(text, egui::FontId::monospace(10.0), green);
            painter.galley(top_left + egui::vec2(1.0, 0.0), galley, green);
        }
    }

    /// Paints the Animation-placer boundary highlights (Phase 3): the FIRST
    /// frame's region (red), the LAST frame's region (green), the whole indexed
    /// area (grey), and the live in-flight rectangle. All strokes are plain
    /// solid 1 pt outlines and every colour is a [`ThemeColors`] token (no
    /// hard-coded chrome colour).
    fn paint_animation_placer_overlay(
        &self,
        painter: &egui::Painter,
        theme: &ThemeColors,
        camera: Camera,
    ) {
        let CanvasOverlay::AnimationPlacer {
            first,
            last,
            area,
            in_flight,
        } = &self.overlay
        else {
            return;
        };
        let (red, green, grey) = animation_placer_colors(theme);
        // Live in-flight rectangle first, so the three committed boundaries
        // read on top of it.
        if let Some(rect) = in_flight {
            paint_live_outline(
                painter,
                self.canvas_rect_to_screen(camera, *rect),
                live_outline_color(theme),
            );
        }
        paint_live_outline(painter, self.canvas_rect_to_screen(camera, *area), grey);
        paint_live_outline(painter, self.canvas_rect_to_screen(camera, *first), red);
        paint_live_outline(painter, self.canvas_rect_to_screen(camera, *last), green);
    }

    /// Paints the onion-skin overlay viewports.
    ///
    /// Each wrapped neighbour region is drawn as a translucent textured quad
    /// occupying the CURRENT region's screen rect (so the ghost aligns cell-for-
    /// cell with the base viewport), with UVs sampling the neighbour region.
    /// Painter-only: it never touches interactions. `prev` paints first, then
    /// `next` ON TOP.
    ///
    /// Both [`CanvasOverlay::FrameOnion`] and [`CanvasOverlay::OnionOverlay`]
    /// feed this pass. The tints are the theme's `onion_prev_view_tint` /
    /// `onion_next_view_tint` tokens (light orange / light blue with the 140/255
    /// preview alpha already baked in), darkened by [`onion_multiply_tint`] for
    /// an approximate multiply blend. The renderer has no per-shape blend mode,
    /// so a real multiply would need a renderer change; every colour channel
    /// still comes from the theme — no hard-coded chrome.
    fn paint_onion_overlay(
        &self,
        painter: &egui::Painter,
        theme: &ThemeColors,
        camera: Camera,
        image_id: egui::TextureId,
        sheet_size: (u32, u32),
    ) {
        let (prev, next) = match &self.overlay {
            CanvasOverlay::FrameOnion { prev, next }
            | CanvasOverlay::OnionOverlay { prev, next } => (prev, next),
            _ => return,
        };
        // Approximate multiply: darken the tint's RGB, preserving its alpha.
        let prev_tint = onion_multiply_tint(theme.onion_prev_view_tint_core());
        let next_tint = onion_multiply_tint(theme.onion_next_view_tint_core());
        // Only meaningful for a region-scoped (frame-edit) viewport.
        let Some(current) = self.region else {
            return;
        };
        if sheet_size.0 == 0 || sheet_size.1 == 0 || current.is_empty() {
            return;
        }
        let screen = self.canvas_rect_to_screen(camera, current);
        // Order matters: prev first, next last so `next` reads on top.
        for (region, tint) in [(prev, prev_tint), (next, next_tint)] {
            let Some(region) = region else {
                continue;
            };
            if region.is_empty() {
                continue;
            }
            let uv = egui::Rect::from_min_max(
                egui::pos2(
                    region.x as f32 / sheet_size.0 as f32,
                    region.y as f32 / sheet_size.1 as f32,
                ),
                egui::pos2(
                    region.right() as f32 / sheet_size.0 as f32,
                    region.bottom() as f32 / sheet_size.1 as f32,
                ),
            );
            paint_textured_quad(painter, image_id, screen, uv, tint);
        }
    }

    /// The canvas pixel the primary drag reports this frame.
    ///
    /// A live floating SELECTION transform maps the pointer with the clamp-free
    /// [`Self::screen_to_canvas_unclamped`], so the object can be dragged fully
    /// outside the canvas; it holds its last point while the interact position
    /// is unavailable. A live Fieldier selection drag maps with
    /// [`Self::screen_to_canvas_clamped`], so a pointer outside the window
    /// still extends the gesture at the canvas edge, and it holds its last
    /// clamped point while the interact position is unavailable. Every other
    /// tool maps the pointer with the strict footprint anchor, unchanged.
    fn primary_drag_point(
        &mut self,
        primary_response: &egui::Response,
        mapping: &DragMapping,
    ) -> Option<(i32, i32)> {
        if mapping.transform_drag {
            return match primary_response.interact_pointer_pos() {
                Some(pos) => {
                    let pt =
                        self.screen_to_canvas_unclamped(pos, mapping.camera, mapping.canvas_size);
                    self.last_transform_drag_point = pt;
                    pt
                }
                None => self.last_transform_drag_point,
            };
        }
        if !mapping.clamped_selection {
            return primary_response.interact_pointer_pos().and_then(|pos| {
                canvas_footprint_anchor(
                    pos,
                    mapping.origin,
                    mapping.scale,
                    mapping.pan,
                    self.view_size(mapping.canvas_size),
                    mapping.anchor_size,
                )
                .map(|local| self.local_to_canvas(local))
            });
        }
        match primary_response.interact_pointer_pos() {
            Some(pos) => {
                let pt = self.screen_to_canvas_clamped(pos, mapping.camera, mapping.canvas_size);
                self.last_clamped_drag_point = Some(pt);
                Some(pt)
            }
            None => self.last_clamped_drag_point,
        }
    }

    /// Map a screen position to a canvas pixel, or `None` when outside the
    /// canvas draw rect (mirrors [`screen_to_canvas_pt`]).
    pub fn screen_to_canvas(
        &self,
        pos: egui::Pos2,
        camera: Camera,
        canvas_size: (u32, u32),
    ) -> Option<(i32, i32)> {
        screen_to_canvas_pt(
            pos,
            self.last_origin,
            camera.scale() as f32,
            camera.pan(),
            self.view_size(canvas_size),
        )
        .map(|local| self.local_to_canvas(local))
    }

    /// Map any screen position to a canvas pixel, clamping the position into
    /// the canvas draw rect and the result into the canvas pixel bounds.
    pub fn screen_to_canvas_clamped(
        &self,
        pos: egui::Pos2,
        camera: Camera,
        canvas_size: (u32, u32),
    ) -> (i32, i32) {
        let scale = camera.scale() as f32;
        let pan = camera.pan();
        let view_size = self.view_size(canvas_size);
        let draw_rect = canvas_draw_rect(self.last_origin, scale, pan, view_size);
        let clamped = draw_rect.clamp(pos);
        let rel = clamped - self.last_origin.to_vec2();
        let cx = ((rel.x - pan.0 as f32) / scale).floor() as i32;
        let cy = ((rel.y - pan.1 as f32) / scale).floor() as i32;
        let max_x = i32::try_from(view_size.0.saturating_sub(1)).unwrap_or(i32::MAX);
        let max_y = i32::try_from(view_size.1.saturating_sub(1)).unwrap_or(i32::MAX);
        let local = (cx.clamp(0, max_x), cy.clamp(0, max_y));
        self.local_to_canvas(local)
    }

    /// Map ANY screen position to its real canvas pixel coordinate WITHOUT
    /// clamping: the result may be negative or beyond the canvas dimensions.
    /// This is the clamp-free sibling of [`Self::screen_to_canvas_clamped`],
    /// used by the floating transform so its bbox can leave the canvas bounds
    /// entirely (and so an outside-canvas commit point stays outside the
    /// object). Returns `None` only when the position is genuinely unusable
    /// (non-finite pointer or non-positive scale).
    pub fn screen_to_canvas_unclamped(
        &self,
        pos: egui::Pos2,
        camera: Camera,
        _canvas_size: (u32, u32),
    ) -> Option<(i32, i32)> {
        let scale = camera.scale() as f32;
        if !pos.x.is_finite() || !pos.y.is_finite() || !(scale > 0.0) {
            return None;
        }
        let pan = camera.pan();
        let rel = pos - self.last_origin.to_vec2();
        let cx = ((rel.x - pan.0 as f32) / scale).floor() as i32;
        let cy = ((rel.y - pan.1 as f32) / scale).floor() as i32;
        Some(self.local_to_canvas((cx, cy)))
    }

    /// Map a screen position to the canvas cell a brush of `size` pixels stamps
    /// at, or `None` when the pointer's footprint cannot touch the canvas.
    ///
    /// Unlike [`Self::screen_to_canvas`] the pointer may lie outside the draw
    /// rect: the anchor is computed for any position and the pointer keeps
    /// counting as over the canvas while the footprint's bounding box still
    /// intersects it (mirrors `canvas_footprint_anchor`). `size` is the
    /// sanitized brush size (`1..=64`).
    pub fn screen_to_canvas_footprint(
        &self,
        pos: egui::Pos2,
        camera: Camera,
        canvas_size: (u32, u32),
        size: u8,
    ) -> Option<(i32, i32)> {
        canvas_footprint_anchor(
            pos,
            self.last_origin,
            camera.scale() as f32,
            camera.pan(),
            self.view_size(canvas_size),
            size,
        )
        .map(|local| self.local_to_canvas(local))
    }

    /// Apply an absolute zoom percent around the canvas point under `pointer_pos`,
    /// keeping that point fixed on screen. Returns the applied zoom factor.
    fn zoom_to(
        camera: &mut Camera,
        new_zoom: u32,
        pointer_pos: egui::Pos2,
        origin: egui::Pos2,
    ) -> Option<(f32, egui::Pos2)> {
        let old_zoom = camera.zoom_percent();
        let new_zoom = new_zoom.clamp(Camera::MIN_ZOOM_PERCENT, Camera::MAX_ZOOM_PERCENT);
        if new_zoom == old_zoom {
            return None;
        }
        let anchor = Self::anchor_canvas_at(camera, pointer_pos, origin);
        camera.anchor_zoom(anchor, new_zoom);
        Some((new_zoom as f32 / old_zoom as f32, pointer_pos))
    }

    /// The canvas pixel under `pointer_pos`, via the camera's exact integer math.
    fn anchor_canvas_at(
        camera: &Camera,
        pointer_pos: egui::Pos2,
        origin: egui::Pos2,
    ) -> (i32, i32) {
        let rel = pointer_pos - origin.to_vec2();
        camera.screen_to_canvas(rel.x.round() as i32, rel.y.round() as i32)
    }
}

impl Default for CanvasWidget {
    fn default() -> Self {
        Self::new()
    }
}

/// The whole-canvas clip previews fall back to when no selection is active.
fn canvas_rect_clip(canvas_size: (u32, u32)) -> Option<PixelClip> {
    let rect = Rect2i::new(0, 0, canvas_size.0 as i32, canvas_size.1 as i32);
    (!rect.is_empty()).then(|| PixelClip::from_rect(rect))
}

/// Screen rect occupied by the canvas texture.
///
/// Mirrors the camera convention: canvas coord `c` spans screen pixels
/// `[pan + c*scale, pan + (c+1)*scale)`, so the canvas's top-left screen corner
/// is `origin + pan` and its size is `canvas_size * scale`.
fn canvas_draw_rect(
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    canvas_size: (u32, u32),
) -> egui::Rect {
    let min = egui::pos2(origin.x + pan.0 as f32, origin.y + pan.1 as f32);
    let size = egui::vec2(canvas_size.0 as f32 * scale, canvas_size.1 as f32 * scale);
    egui::Rect::from_min_size(min, size)
}

/// Interior region-grid lines along one axis: multiples of `tile` inside the
/// canvas that fall into the visible viewport range.
fn tile_grid_axis(
    length: u32,
    tile: u32,
    scale: f32,
    start: f32,
    viewport_start: f32,
    viewport_end: f32,
) -> Vec<u32> {
    if tile == 0 || length <= tile {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut coord = tile;
    while coord < length {
        let screen = start + coord as f32 * scale;
        if screen >= viewport_start && screen <= viewport_end {
            lines.push(coord);
        }
        coord += tile;
    }
    lines
}

/// Region-grid lines for both axes, culled to the viewport.
///
/// The grid is region-based (one line per `tile_size` region boundary), not
/// per pixel. The size is PER AXIS, so density suppression is applied
/// independently: vertical lines use `tile_size.w` and horizontal lines use
/// `tile_size.h`; an axis whose lines are closer than
/// [`MIN_TILE_LINE_SPACING`] is suppressed alone.
pub fn tile_grid_lines(
    canvas_size: (u32, u32),
    tile_size: TileSize,
    scale: f32,
    draw_rect: egui::Rect,
    viewport: egui::Rect,
    visible: bool,
) -> (Vec<u32>, Vec<u32>) {
    if !visible || !scale.is_finite() {
        return (Vec::new(), Vec::new());
    }
    let vertical = if tile_size.w as f32 * scale >= MIN_TILE_LINE_SPACING {
        tile_grid_axis(
            canvas_size.0,
            tile_size.w,
            scale,
            draw_rect.left(),
            viewport.left(),
            viewport.right(),
        )
    } else {
        Vec::new()
    };
    let horizontal = if tile_size.h as f32 * scale >= MIN_TILE_LINE_SPACING {
        tile_grid_axis(
            canvas_size.1,
            tile_size.h,
            scale,
            draw_rect.top(),
            viewport.top(),
            viewport.bottom(),
        )
    } else {
        Vec::new()
    };
    (vertical, horizontal)
}

/// Continuous zoom factor for a smoothed scroll delta (plain scroll).
///
/// Shared with the dynamic nest so both zoom at the same rate.
pub fn scroll_zoom_factor(scroll_y: f32) -> f32 {
    ZOOM_PER_NOTCH.powf(scroll_y / SCROLL_POINTS_PER_NOTCH)
}

/// Integer wheel steps for a smoothed scroll delta: any non-zero scroll counts
/// as at least one step.
pub fn scroll_steps(scroll_y: f32) -> i32 {
    if scroll_y > 0.0 {
        ((scroll_y / SCROLL_POINTS_PER_NOTCH).round() as i32).max(1)
    } else if scroll_y < 0.0 {
        ((scroll_y / SCROLL_POINTS_PER_NOTCH).round() as i32).min(-1)
    } else {
        0
    }
}

/// True when this frame carries an Alt+wheel gesture.
///
/// Same event-modifier caveat as `ctrl_wheel`/`shift_wheel`: `InputState`'s
/// held-key modifiers can miss a synthetic or fast wheel, so the raw wheel
/// events are checked too.
fn alt_wheel(ui: &egui::Ui) -> bool {
    ui.input(|input| {
        input.modifiers.alt
            || input.events.iter().any(
                |event| matches!(event, egui::Event::MouseWheel { modifiers, .. } if modifiers.alt),
            )
    })
}

/// Boundary edges of a brush footprint, in pixel-edge coordinates.
///
/// Each offset is one footprint pixel; an edge is emitted only when the
/// neighbouring pixel is outside the footprint, so the result is the
/// footprint's silhouette (a hollow pixel cursor).
pub fn brush_outline_segments(offsets: &[(i32, i32)]) -> Vec<((i32, i32), (i32, i32))> {
    let pixels: std::collections::BTreeSet<(i32, i32)> = offsets.iter().copied().collect();
    let mut segments = Vec::new();
    for (dx, dy) in pixels.iter().copied() {
        if !pixels.contains(&(dx, dy - 1)) {
            segments.push(((dx, dy), (dx + 1, dy)));
        }
        if !pixels.contains(&(dx, dy + 1)) {
            segments.push(((dx, dy + 1), (dx + 1, dy + 1)));
        }
        if !pixels.contains(&(dx - 1, dy)) {
            segments.push(((dx, dy), (dx, dy + 1)));
        }
        if !pixels.contains(&(dx + 1, dy)) {
            segments.push(((dx + 1, dy), (dx + 1, dy + 1)));
        }
    }
    segments
}

/// Deduped union of the brush footprint stamped at every `point`: the cells
/// the path previews paint.
fn footprint_cells(
    points: impl IntoIterator<Item = (i32, i32)>,
    offsets: &[(i32, i32)],
) -> std::collections::BTreeSet<(i32, i32)> {
    let mut cells: std::collections::BTreeSet<(i32, i32)> = std::collections::BTreeSet::new();
    for (px, py) in points {
        for &(dx, dy) in offsets {
            cells.insert((px + dx, py + dy));
        }
    }
    cells
}

/// How this frame's primary drag turns a pointer position into a canvas point.
struct DragMapping {
    /// A selection drag is live: follow the pointer clamped into the canvas
    /// so the gesture keeps extending after the pointer leaves the window.
    clamped_selection: bool,
    /// A floating selection transform is the live gesture: follow the pointer
    /// CLAMP-FREE so the object can leave the canvas bounds. Mutually exclusive
    /// with [`Self::clamped_selection`].
    transform_drag: bool,
    camera: Camera,
    canvas_size: (u32, u32),
    /// Screen position of canvas pixel (0, 0) captured this frame.
    origin: egui::Pos2,
    /// 1× camera scale.
    scale: f32,
    /// Canvas-space pan.
    pan: (i32, i32),
    /// The active tool's brush size (1 for the point tools).
    anchor_size: u8,
}

/// One footprint preview: the cells, the draw mode, the primary color, and the
/// canvas→screen mapping they are painted through.
struct FootprintPreview<'a> {
    cells: &'a std::collections::BTreeSet<(i32, i32)>,
    mode: DrawMode,
    color: Color,
    /// Screen position of canvas pixel (0, 0).
    origin: egui::Pos2,
    scale: f32,
    /// Cells outside this clip are dropped; `None` paints every cell (the brush
    /// cursor is window-clipped by the painter instead).
    clip: Option<&'a PixelClip>,
}

/// Paint a footprint preview the way the brush cursor does: Pen fills each
/// cell solid in [`FootprintPreview::color`], Eraser draws the hollow
/// silhouette in the theme's selection stroke. Edges are drawn as thin filled
/// quads: 1 pt line segments are culled by the tessellator, filled shapes are
/// not.
fn paint_footprint(painter: &egui::Painter, theme: &ThemeColors, preview: &FootprintPreview<'_>) {
    let inside = |cx: i32, cy: i32| preview.clip.is_none_or(|clip| clip.contains(cx, cy));
    match preview.mode {
        DrawMode::Pen => {
            let color = preview.color;
            let color32 = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
            for &(cx, cy) in preview.cells {
                if !inside(cx, cy) {
                    continue;
                }
                let min = egui::pos2(
                    preview.origin.x + cx as f32 * preview.scale,
                    preview.origin.y + cy as f32 * preview.scale,
                );
                // At least 1 pt per cell, like the outline quads.
                let rect = egui::Rect::from_min_max(
                    min,
                    min + egui::vec2(preview.scale, preview.scale).max(egui::vec2(1.0, 1.0)),
                );
                painter.rect_filled(rect, 0.0, color32);
            }
        }
        DrawMode::Eraser => {
            let color = theme.selection_stroke32();
            let cells: Vec<(i32, i32)> = preview
                .cells
                .iter()
                .copied()
                .filter(|&(cx, cy)| inside(cx, cy))
                .collect();
            for ((x0, y0), (x1, y1)) in brush_outline_segments(&cells) {
                let sx0 = preview.origin.x + x0 as f32 * preview.scale;
                let sy0 = preview.origin.y + y0 as f32 * preview.scale;
                let sx1 = preview.origin.x + x1 as f32 * preview.scale;
                let sy1 = preview.origin.y + y1 as f32 * preview.scale;
                let rect = egui::Rect::from_min_max(
                    egui::pos2(sx0.min(sx1), sy0.min(sy1)),
                    egui::pos2(
                        sx0.max(sx1).max(sx0.min(sx1) + 1.0),
                        sy0.max(sy1).max(sy0.min(sy1) + 1.0),
                    ),
                );
                painter.rect_filled(rect, 0.0, color);
            }
        }
    }
}

/// The live-gesture outline color: the marching-ants core token converted to
/// egui straight-alpha, the same stroke the live marquee paints.
fn live_outline_color(theme: &ThemeColors) -> egui::Color32 {
    let color = theme.marching_ants_core();
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// The `(first, last, area)` stroke colours for [`CanvasOverlay::AnimationPlacer`],
/// read from [`ThemeColors`] tokens so no UI colour is ever hard-coded:
/// red = the previous-frame onion tint, green = the next-frame onion tint
/// (the same green the tile placer already uses), grey = the fixed
/// marching-ants under-stroke token.
fn animation_placer_colors(theme: &ThemeColors) -> (egui::Color32, egui::Color32, egui::Color32) {
    (
        theme.onion_prev_tint32(),
        theme.onion_next_tint32(),
        theme.marching_ants_under32(),
    )
}

/// The live-marquee stroke for an App-sampled canvas pixel: the per-channel
/// inverse (`255 - channel`) of `probe` (D76), opaque. Only a stroke — the
/// live box is never filled.
fn inverted_probe_color(probe: Color) -> egui::Color32 {
    let color = Color::rgba(255 - probe.r, 255 - probe.g, 255 - probe.b, 255);
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// Approximate multiply factor for the onion-skin tints: the tint's RGB is
/// scaled down in gamma space while its alpha is preserved, so the neighbouring
/// frame content reads as if multiplied into the tint at the token's opacity.
///
/// The egui/wgpu renderer here supports no per-shape blend mode, so a real
/// multiply blend would require a renderer change; this cheap gamma-space
/// darkening is the documented stand-in (factor chosen for a clearly darker but
/// still readable ghost).
const ONION_MULTIPLY: f32 = 0.6;

/// Darken `base`'s RGB by [`ONION_MULTIPLY`] while PRESERVING its alpha, giving
/// the onion-skin overlays an approximate multiply look at the token's opacity.
///
/// This mirrors `Color32::gamma_multiply`'s gamma-space RGB scaling but holds
/// alpha fixed (that method scales the alpha too, fading the ghost instead of
/// darkening it). `base` is the token's STRAIGHT (un-premultiplied) RGBA, so the
/// scaled tint re-premultiplies correctly at the same alpha. Every channel still
/// comes from the theme token — no hard-coded chrome colour.
fn onion_multiply_tint(base: Color) -> egui::Color32 {
    let factor = ONION_MULTIPLY;
    let color = Color::rgba(
        (base.r as f32 * factor).round() as u8,
        (base.g as f32 * factor).round() as u8,
        (base.b as f32 * factor).round() as u8,
        base.a,
    );
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// A textured quad (a translucent region image) between `screen` corners and
/// `uv` texture coordinates, tinted `color`. Shared by the frame-edit onion
/// overlay viewports.
fn paint_textured_quad(
    painter: &egui::Painter,
    image_id: egui::TextureId,
    screen: egui::Rect,
    uv: egui::Rect,
    color: egui::Color32,
) {
    let mut mesh = egui::epaint::Mesh::with_texture(image_id);
    mesh.vertices = vec![
        egui::epaint::Vertex {
            pos: screen.min,
            uv: uv.min,
            color,
        },
        egui::epaint::Vertex {
            pos: egui::pos2(screen.max.x, screen.min.y),
            uv: egui::pos2(uv.max.x, uv.min.y),
            color,
        },
        egui::epaint::Vertex {
            pos: screen.max,
            uv: uv.max,
            color,
        },
        egui::epaint::Vertex {
            pos: egui::pos2(screen.min.x, screen.max.y),
            uv: egui::pos2(uv.min.x, uv.max.y),
            color,
        },
    ];
    mesh.indices = vec![0, 1, 2, 0, 2, 3];
    painter.add(egui::Shape::mesh(mesh));
}

/// One plain solid unit outline (1 pt, centered, no fill): the shared
/// live-gesture stroke of the marquee, lasso, and region-cell overlays (D73).
fn paint_live_outline(painter: &egui::Painter, screen: egui::Rect, color: egui::Color32) {
    painter.rect_stroke(
        screen,
        0.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Middle,
    );
}

/// The solid, time-invariant grey under-stroke beneath the rect marching ants:
/// the full rectangle perimeter as four 1 px lines, painted before the dashes
/// so the white dash phase always reads against the canvas (D76).
fn paint_ants_under_rect(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    rect: Rect2i,
    color: egui::Color32,
) {
    let to_screen = |x: f32, y: f32| {
        egui::pos2(
            origin.x + pan.0 as f32 + x * scale,
            origin.y + pan.1 as f32 + y * scale,
        )
    };
    let min = to_screen(rect.x as f32, rect.y as f32);
    let max = to_screen(rect.right() as f32, rect.bottom() as f32);
    let top_right = egui::pos2(max.x, min.y);
    let bottom_left = egui::pos2(min.x, max.y);
    for (a, b) in [
        (min, top_right),
        (top_right, max),
        (max, bottom_left),
        (bottom_left, min),
    ] {
        painter.line_segment([a, b], egui::Stroke::new(2.0, color));
    }
}

fn paint_ants_rect(
    painter: &egui::Painter,
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    rect: Rect2i,
    color: egui::Color32,
    time_ms: u64,
) {
    for ((x0, y0), (x1, y1)) in ant_segments(rect, time_ms) {
        let p0 = egui::pos2(
            origin.x + pan.0 as f32 + x0 * scale,
            origin.y + pan.1 as f32 + y0 * scale,
        );
        let p1 = egui::pos2(
            origin.x + pan.0 as f32 + x1 * scale,
            origin.y + pan.1 as f32 + y1 * scale,
        );
        painter.line_segment([p0, p1], egui::Stroke::new(1.5, color));
    }
}

/// The mask-boundary segments an overlay paints marching ants over, if any.
///
/// Shared by [`CanvasWidget::set_overlay`] (to memoise the chained polylines)
/// and the paint path, so the key and the painted boundary can never disagree.
fn overlay_mask_segments(overlay: &CanvasOverlay) -> Option<&[BoundarySegment]> {
    match overlay {
        CanvasOverlay::AntsMask(segments) => Some(segments.as_slice()),
        CanvasOverlay::RegionCells { base, .. } if !base.is_empty() => Some(base.as_slice()),
        _ => None,
    }
}

/// Paint a mask boundary as pixel-perfect marching ants (Pyxel Edit style).
///
/// The boundary is treated as a per-pixel binary mask: `mask_polylines` chains
/// the mask's unit edges into closed clockwise loops, and
/// [`pixel_ant_edges_from_polylines`] splits every document-pixel edge into
/// [`PIXEL_ANT_SUBDIV`] sub-edges and marks each ON (dash) or OFF (gap) from the
/// animation phase. Only the ON edges are painted — so the border is a trail of
/// small alternating sub-pixel segments (4× smaller than the former per-pixel
/// segments), not a thin continuous line.
///
/// Sub-unit coordinates (`document_px * PIXEL_ANT_SUBDIV`) are mapped to screen
/// with `origin + pan + (x / PIXEL_ANT_SUBDIV) * scale`, so zoom scales the
/// segments while the pattern stays locked to the sub-unit grid. The dashes use
/// the theme's `marching_ants` token (its alpha scaled by [`ant_stroke_color`]);
/// nothing is hard-coded.
///
/// The legacy fractional-arc-length dash walk
/// (`mask_dash_segments_from_polylines`) is no longer used here.
fn paint_ants_mask(
    painter: &egui::Painter,
    theme: &ThemeColors,
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    polylines: &[Vec<(f32, f32)>],
    time_ms: u64,
) {
    let subdiv = PIXEL_ANT_SUBDIV as f32;
    // `x`/`y` are SUB-UNIT coordinates: convert back to document pixels first.
    let to_screen = |x: f32, y: f32| {
        egui::pos2(
            origin.x + pan.0 as f32 + (x / subdiv) * scale,
            origin.y + pan.1 as f32 + (y / subdiv) * scale,
        )
    };
    let dark_under = theme.marching_ants_contrast_dark32();
    let bright_ant = crate::ui::theme::SKIN_TINT;
    let edges = pixel_ant_edges_from_polylines(polylines, time_ms);
    for edge in &edges {
        painter.line_segment(
            [
                to_screen(edge.start.0 as f32, edge.start.1 as f32),
                to_screen(edge.end.0 as f32, edge.end.1 as f32),
            ],
            egui::Stroke::new(2.0, dark_under),
        );
    }
    for edge in edges {
        // ON runs are bright white dashes over the dark backing stroke.
        if !edge.on {
            continue;
        }
        painter.line_segment(
            [
                to_screen(edge.start.0 as f32, edge.start.1 as f32),
                to_screen(edge.end.0 as f32, edge.end.1 as f32),
            ],
            egui::Stroke::new(1.5, bright_ant),
        );
    }
}

/// The union footprint of the frame regions: a pixel is covered when ANY
/// non-empty region contains it.
///
/// Returns the union boundary as unit segments — [the same rule as
/// `Selection::outline_segments`](crate::core::select::Selection::outline_segments):
/// a unit edge is produced only where a covered pixel meets a NON-covered
/// neighbour. Adjacent frames therefore share NO inner edge: only the outer
/// silhouette of the union is drawn. Overlapping rects emit an edge once
/// (`sort` + `dedup`). Deterministic; empty input yields an empty vec.
fn frame_region_boundary_segments(regions: &[Rect2i]) -> Vec<BoundarySegment> {
    let covered =
        |x: i32, y: i32| regions.iter().any(|r| !r.is_empty() && r.contains(x, y));
    let mut segs = Vec::new();
    for r in regions {
        if r.is_empty() {
            continue;
        }
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                // Only the outer ring of this rect can border the union: a
                // strictly interior pixel has all four neighbours covered too.
                if x != r.x && x != r.right() - 1 && y != r.y && y != r.bottom() - 1 {
                    continue;
                }
                // Top.
                if !covered(x, y - 1) {
                    segs.push(((x, y), (x + 1, y)));
                }
                // Bottom.
                if !covered(x, y + 1) {
                    segs.push(((x, y + 1), (x + 1, y + 1)));
                }
                // Left.
                if !covered(x - 1, y) {
                    segs.push(((x, y), (x, y + 1)));
                }
                // Right.
                if !covered(x + 1, y) {
                    segs.push(((x + 1, y), (x + 1, y + 1)));
                }
            }
        }
    }
    segs.sort();
    segs.dedup();
    segs
}

/// Paint the union outline of the frame regions as pixel-perfect marching ants.
///
/// Only the OUTER silhouette of the union is drawn:
/// [`frame_region_boundary_segments`] drops the inner edges shared by adjacent
/// frames, and [`mask_polylines`] chains the unit edges into closed clockwise
/// loops (one per disjoint component). The solid, time-invariant
/// under-baseline is the `frame_region_ant_under` token (bordeaux) painted
/// along every loop in document-pixel coordinates; the animated dashes are the
/// `frame_region_ant` token's RGB at [`FRAME_REGION_ANT_STROKE_ALPHA`] (220)
/// — clearly visible yellow — and march at half the selection-ant speed
/// (`time_ms * FRAME_REGION_ANT_SPEED_SCALE`). Sub-unit coordinates
/// (`document_px * PIXEL_ANT_SUBDIV`) are mapped to screen the same way as
/// [`paint_ants_mask`], so zoom scales the segments while the pattern stays
/// locked to the sub-unit grid. Painter-only and non-interactive; an empty
/// union paints nothing.
fn paint_frame_regions(
    painter: &egui::Painter,
    theme: &ThemeColors,
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    regions: &[Rect2i],
    time_ms: u64,
) {
    let segs = frame_region_boundary_segments(regions);
    if segs.is_empty() {
        return;
    }
    let loops = mask_polylines(&segs);
    let subdiv = PIXEL_ANT_SUBDIV as f32;
    // `x`/`y` are DOCUMENT pixels here: loops are in doc-px coordinates.
    let to_screen = |x: f32, y: f32| {
        egui::pos2(
            origin.x + pan.0 as f32 + x * scale,
            origin.y + pan.1 as f32 + y * scale,
        )
    };
    // ... while `x`/`y` are SUB-UNIT coordinates for the dash edges.
    let to_sub_screen = |x: f32, y: f32| {
        egui::pos2(
            origin.x + pan.0 as f32 + (x / subdiv) * scale,
            origin.y + pan.1 as f32 + (y / subdiv) * scale,
        )
    };
    // Theme-tinted only: the construction keeps the channels on one line
    // (`color.r, color.g, color.b`) so no hard-coded chrome colour is added.
    // Solid bordeaux baseline (unchanged: `ant_under_stroke_color` alpha 200).
    let color = ant_under_stroke_color(theme.frame_region_ant_under_core());
    let under_color = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
    // Yellow dashes: the token's RGB at the higher FRAME_REGION alpha so the
    // moving line stays clearly yellow over the bordeaux baseline.
    let color = theme.frame_region_ant_core();
    let a = (u32::from(color.a) * u32::from(FRAME_REGION_ANT_STROKE_ALPHA) / 255) as u8;
    let ant_color = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, a);
    // Half-speed march: halve the phase input (ANT_SPEED_PX_PER_MS untouched).
    let t = (time_ms as f32 * FRAME_REGION_ANT_SPEED_SCALE) as u64;
    // Solid bordeaux baseline along every outer loop, then yellow dashes.
    for loop_ in &loops {
        if loop_.len() < 2 {
            continue;
        }
        for i in 0..loop_.len() {
            let a = loop_[i];
            let b = loop_[(i + 1) % loop_.len()];
            painter.line_segment(
                [to_screen(a.0, a.1), to_screen(b.0, b.1)],
                egui::Stroke::new(1.0, under_color),
            );
        }
    }
    for edge in pixel_ant_edges_from_polylines(&loops, t) {
        // ON runs are dashes; OFF runs are gaps, so the border reads as small
        // alternating sub-pixel segments rather than a continuous line.
        if !edge.on {
            continue;
        }
        painter.line_segment(
            [
                to_sub_screen(edge.start.0 as f32, edge.start.1 as f32),
                to_sub_screen(edge.end.0 as f32, edge.end.1 as f32),
            ],
            egui::Stroke::new(1.0, ant_color),
        );
    }
}

/// Map an egui screen position to a canvas pixel coordinate.
///
/// Returns `None` when `p` is outside the canvas draw rect. Floor semantics
/// mirror the camera's nearest-neighbor convention: a canvas pixel at coord `c`
/// occupies screen pixels `[pan + c*scale, pan + (c+1)*scale)`.
fn screen_to_canvas_pt(
    p: egui::Pos2,
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    canvas_size: (u32, u32),
) -> Option<(i32, i32)> {
    let rect = canvas_draw_rect(origin, scale, pan, canvas_size);
    // Half-open bounds: canvas pixel c spans [pan + c*scale, pan + (c+1)*scale),
    // so the max edge belongs to the next pixel (or is outside the canvas).
    if p.x < rect.min.x || p.y < rect.min.y || p.x >= rect.max.x || p.y >= rect.max.y {
        return None;
    }
    let rel = p - origin.to_vec2();
    let cx = ((rel.x - pan.0 as f32) / scale).floor() as i32;
    let cy = ((rel.y - pan.1 as f32) / scale).floor() as i32;
    Some((cx, cy))
}

/// The canvas anchor a brush of `size` pixels stamps at for a pointer at `p`,
/// or `None` when the pointer's `size×size` footprint cannot touch the canvas.
///
/// Unlike [`screen_to_canvas_pt`], `p` may lie outside the draw rect: the
/// anchor is computed for any position and the pointer keeps counting as over
/// the canvas while the footprint's bounding box still intersects the canvas
/// rect, so a large brush paints on until its own edge leaves the canvas (the
/// paint path clips the stamp to the buffer).
///
/// `size` is the sanitized brush size (`1..=64`); the footprint's bounding box
/// is the `size×size` square centered on the anchor, exactly as stamped.
fn canvas_footprint_anchor(
    p: egui::Pos2,
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    canvas_size: (u32, u32),
    size: u8,
) -> Option<(i32, i32)> {
    let (cx, cy) = brush_anchor_at(p, origin, scale, pan, size);
    let size = i32::from(size);
    let half = size / 2;
    let footprint = Rect2i::new(cx - half, cy - half, size, size);
    let canvas = Rect2i::new(0, 0, canvas_size.0 as i32, canvas_size.1 as i32);
    footprint.intersects(canvas).then_some((cx, cy))
}

/// Raw canvas anchor for a brush of `size` pixels at `p` (no canvas gating).
///
/// Odd sizes keep the cursor cell (floor): their middle pixel is the cell
/// center, which is where the OS pointer sits. Even sizes have no middle pixel
/// — their footprint's center is the lattice point (cell corner) nearest the
/// raw pointer, so the block straddles the pointer evenly instead of hanging
/// off the cell's top-left corner.
fn brush_anchor_at(
    p: egui::Pos2,
    origin: egui::Pos2,
    scale: f32,
    pan: (i32, i32),
    size: u8,
) -> (i32, i32) {
    let rel = p - origin.to_vec2();
    let fx = (rel.x - pan.0 as f32) / scale;
    let fy = (rel.y - pan.1 as f32) / scale;
    if size.is_multiple_of(2) {
        (fx.round() as i32, fy.round() as i32)
    } else {
        (fx.floor() as i32, fy.floor() as i32)
    }
}

/// Integer zoom steps for a smoothed scroll delta (in points).
///
/// Any non-zero scroll counts as at least one step (egui smooths discrete wheel
/// notches over several frames, so a single notch may arrive as a fraction of
/// `SCROLL_POINTS_PER_NOTCH`); larger deltas scale proportionally.
fn zoom_steps(scroll_y: f32) -> i32 {
    if scroll_y > 0.0 {
        ((scroll_y / SCROLL_POINTS_PER_NOTCH).round() as i32).max(1)
    } else if scroll_y < 0.0 {
        ((scroll_y / SCROLL_POINTS_PER_NOTCH).round() as i32).min(-1)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::overlay::{ant_phase, ANT_DASH_PX, ANT_GAP_PX, ANT_SPEED_PX_PER_MS};
    use crate::ui::theme::Theme;

    // Gesture router for the headless harness: one per test thread, mirroring
    // the app's long-lived capture, so multi-frame gestures keep ownership.
    thread_local! {
        static TEST_CAPTURE: InputCapture = InputCapture::new();
    }

    /// Run one headless frame of the widget in a full-screen panel.
    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
    ) -> CanvasInteractions {
        let mut camera = Camera::new();
        run_frame_with_camera(ctx, events, widget, canvas_size, &mut camera, true)
    }

    fn run_frame_with_camera(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
        camera: &mut Camera,
        grid_visible: bool,
    ) -> CanvasInteractions {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut result = CanvasInteractions::default();
        let theme = Theme::default_dark().colors;
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    TEST_CAPTURE.with(|capture| {
                        capture.set_target(Some(SurfaceId::Canvas));
                        result = widget.ui(
                            ui,
                            &theme,
                            capture,
                            egui::TextureId::default(),
                            CanvasView {
                                canvas_size,
                                grid_visible,
                                tile_size: TileSize::square(16),
                                ..Default::default()
                            },
                            *camera,
                        );
                    });
                });
        });
        *camera = result.updated_camera;
        output.textures_delta.clear();
        result
    }

    /// Like [`run_frame_with_shapes_state`] with an explicit brush preview:
    /// `(spec, draw mode, primary color)`.
    fn run_frame_with_shapes_state_with_brush(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
        camera: &mut Camera,
        preview: Option<(crate::core::brush::BrushSpec, DrawMode, Color)>,
    ) -> (CanvasInteractions, Vec<egui::Shape>) {
        run_frame_targeting_surface(
            ctx,
            events,
            widget,
            canvas_size,
            camera,
            preview,
            SurfaceId::Canvas,
        )
    }

    /// Like [`run_frame_with_shapes_state_with_brush`] but records `target` as
    /// the frame's topmost surface in the shared [`InputCapture`], so a test can
    /// stand in "the pointer is over a docked / floating panel".
    fn run_frame_targeting_surface(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
        camera: &mut Camera,
        preview: Option<(crate::core::brush::BrushSpec, DrawMode, Color)>,
        target: SurfaceId,
    ) -> (CanvasInteractions, Vec<egui::Shape>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut result = CanvasInteractions::default();
        let theme = Theme::default_dark().colors;
        let (brush, draw_mode, brush_color) = preview.map_or(
            (None, DrawMode::Pen, Color::BLACK),
            |(spec, mode, color)| (Some(spec), mode, color),
        );
        let mut full_output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    TEST_CAPTURE.with(|capture| {
                        capture.set_target(Some(target));
                        result = widget.ui(
                            ui,
                            &theme,
                            capture,
                            egui::TextureId::default(),
                            CanvasView {
                                canvas_size,
                                grid_visible: true,
                                tile_size: TileSize::square(16),
                                brush,
                                draw_mode,
                                brush_color,
                                ..Default::default()
                            },
                            *camera,
                        );
                    });
                });
        });
        *camera = result.updated_camera;
        full_output.textures_delta.clear();
        let shapes: Vec<egui::Shape> = full_output.shapes.into_iter().map(|cs| cs.shape).collect();
        (result, shapes)
    }

    /// Like [`run_frame`] but also returns the flattened painted shapes.
    fn run_frame_with_shapes(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
    ) -> (CanvasInteractions, Vec<egui::Shape>) {
        let mut camera = Camera::new();
        run_frame_with_shapes_state(ctx, events, widget, canvas_size, &mut camera, true)
    }

    /// Like [`run_frame_with_shapes`] but pins the frame's wall-clock
    /// (`ui.input.time` seconds), for the animation-timing tests.
    fn run_frame_with_shapes_time(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
        time_s: f64,
    ) -> (CanvasInteractions, Vec<egui::Shape>) {
        let mut camera = Camera::new();
        run_frame_with_shapes_state_at(
            ctx,
            events,
            widget,
            canvas_size,
            &mut camera,
            true,
            Some(time_s),
        )
    }

    fn run_frame_with_shapes_state(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
        camera: &mut Camera,
        grid_visible: bool,
    ) -> (CanvasInteractions, Vec<egui::Shape>) {
        run_frame_with_shapes_state_at(ctx, events, widget, canvas_size, camera, grid_visible, None)
    }

    /// The shared driver behind [`run_frame_with_shapes_state`]; `time_s`, when
    /// `Some`, sets `RawInput.time` (a `None` input lets egui synthesize it
    /// from `predicted_dt`).
    fn run_frame_with_shapes_state_at(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        canvas_size: (u32, u32),
        camera: &mut Camera,
        grid_visible: bool,
        time_s: Option<f64>,
    ) -> (CanvasInteractions, Vec<egui::Shape>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            time: time_s,
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut result = CanvasInteractions::default();
        let theme = Theme::default_dark().colors;
        let mut full_output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    TEST_CAPTURE.with(|capture| {
                        capture.set_target(Some(SurfaceId::Canvas));
                        result = widget.ui(
                            ui,
                            &theme,
                            capture,
                            egui::TextureId::default(),
                            CanvasView {
                                canvas_size,
                                grid_visible,
                                tile_size: TileSize::square(16),
                                ..Default::default()
                            },
                            *camera,
                        );
                    });
                });
        });
        *camera = result.updated_camera;
        full_output.textures_delta.clear();
        let shapes: Vec<egui::Shape> = full_output.shapes.into_iter().map(|cs| cs.shape).collect();
        (result, shapes)
    }

    /// Like [`run_frame_with_shapes_state`] but drives the widget with a
    /// caller-built [`CanvasView`], so tests can set `selection_clip` and
    /// `selection_drag_active` exactly as the App shell does.
    fn run_frame_with_view(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut CanvasWidget,
        view: CanvasView,
        camera: Camera,
    ) -> (CanvasInteractions, Vec<egui::Shape>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut result = CanvasInteractions::default();
        let theme = Theme::default_dark().colors;
        let mut full_output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    TEST_CAPTURE.with(|capture| {
                        capture.set_target(Some(SurfaceId::Canvas));
                        result = widget.ui(
                            ui,
                            &theme,
                            capture,
                            egui::TextureId::default(),
                            view.clone(),
                            camera,
                        );
                    });
                });
        });
        full_output.textures_delta.clear();
        let shapes: Vec<egui::Shape> = full_output.shapes.into_iter().map(|cs| cs.shape).collect();
        (result, shapes)
    }

    fn zoomed_view(
        brush: Option<crate::core::brush::BrushSpec>,
        draw_mode: DrawMode,
        brush_color: Color,
        line: Option<LinePreview>,
        selection_clip: Option<crate::core::clip::PixelClip>,
    ) -> CanvasView {
        CanvasView {
            canvas_size: (32, 32),
            grid_visible: false,
            tile_size: TileSize::square(16),
            brush,
            draw_mode,
            brush_color,
            line,
            selection_clip,
            selection_drag_active: false,
            transform_drag_active: false,
            marquee_probe: None,
            canvas_color_at: None,
        }
    }

    fn rect_clip(rect: Rect2i) -> crate::core::clip::PixelClip {
        let buffer = crate::core::buffer::PixelBuffer::new(32, 32);
        let selection =
            crate::core::select::Selection::capture(&buffer, rect).expect("in-bounds capture");
        crate::core::clip::PixelClip::from_selection(&selection)
    }

    fn mask_clip(rect: Rect2i, mask: Vec<bool>) -> crate::core::clip::PixelClip {
        let buffer = crate::core::buffer::PixelBuffer::new(32, 32);
        let selection = crate::core::select::Selection::capture_mask(&buffer, rect, mask)
            .expect("in-bounds masked capture");
        crate::core::clip::PixelClip::from_selection(&selection)
    }

    fn ant_line_points(shapes: &[egui::Shape], color: egui::Color32) -> Vec<[egui::Pos2; 2]> {
        shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::LineSegment { points, stroke }
                    if stroke.color == color && (1.0..=2.0).contains(&stroke.width) =>
                {
                    Some(*points)
                }
                _ => None,
            })
            .collect()
    }

    fn wheel(delta_y: f32, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, delta_y),
            phase: egui::TouchPhase::Move,
            modifiers,
        }
    }

    fn press(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A docked panel surface, for the "pointer sits over panel chrome" tests.
    fn panel_surface() -> SurfaceId {
        SurfaceId::Panel(crate::ui::panel_dock::PanelId::new(1))
    }

    /// PANEL MASK — RAW press: the press point is INSIDE the canvas draw rect,
    /// but the frame's topmost surface is a panel, so the RAW press signal must
    /// stay `None`. This is the path a DRAG-ONLY panel widget (a dock header
    /// band, a resize handle) would otherwise leak through, because it bypasses
    /// egui's widget attribution entirely.
    #[test]
    fn raw_press_over_a_panel_surface_is_not_reported() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        let (over_panel, _) = run_frame_targeting_surface(
            &ctx,
            vec![press(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            None,
            panel_surface(),
        );
        assert_eq!(
            over_panel.raw_pressed_over_canvas, None,
            "a press whose topmost surface is a panel must not reach the canvas"
        );

        // Same press, same point, canvas is the topmost surface: the raw signal
        // carries the pressed cell as before.
        let (over_canvas, _) = run_frame_targeting_surface(
            &ctx,
            vec![press(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            None,
            SurfaceId::Canvas,
        );
        assert_eq!(
            over_canvas.raw_pressed_over_canvas,
            Some((50, 50)),
            "a press over the canvas itself must still report the raw press point"
        );
    }

    /// PANEL MASK — wheel: no camera change, no zoom, no brush/shape/scatter
    /// scroll when the pointer is over a panel, for every wheel flavour.
    #[test]
    fn wheel_over_a_panel_surface_never_touches_the_canvas() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        // Point the pointer inside the canvas draw rect first.
        run_frame_targeting_surface(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            None,
            SurfaceId::Canvas,
        );

        for modifiers in [
            egui::Modifiers::NONE,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
            egui::Modifiers {
                ctrl: true,
                ..egui::Modifiers::NONE
            },
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        ] {
            let zoom_before = camera.zoom_percent();
            let pan_before = camera.pan();
            let (scrolled, _) = run_frame_targeting_surface(
                &ctx,
                vec![wheel(1.0, modifiers)],
                &mut widget,
                (64, 64),
                &mut camera,
                Some((
                    BrushSpec::new(3, crate::core::brush::BrushShape::Square),
                    DrawMode::Pen,
                    Color::BLACK,
                )),
                panel_surface(),
            );
            assert_eq!(scrolled.zoom, None, "no zoom over a panel ({modifiers:?})");
            assert_eq!(
                scrolled.pan_by,
                (0, 0),
                "no pan over a panel ({modifiers:?})"
            );
            assert_eq!(
                scrolled.brush_scroll, 0.0,
                "no brush resize over a panel ({modifiers:?})"
            );
            assert_eq!(
                scrolled.shape_cycle, 0,
                "no brush-shape cycle over a panel ({modifiers:?})"
            );
            assert_eq!(
                scrolled.scatter_scroll, 0.0,
                "no scatter scroll over a panel ({modifiers:?})"
            );
            assert_eq!(camera.zoom_percent(), zoom_before, "camera unchanged");
            assert_eq!(camera.pan(), pan_before, "camera unchanged");
        }
    }

    /// PANEL MASK — middle-drag pan: a middle press over a panel never claims
    /// the canvas pan gesture.
    #[test]
    fn middle_drag_over_a_panel_surface_never_pans_the_canvas() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        let middle = |pressed: bool, pos: egui::Pos2| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Middle,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let pan_before = camera.pan();
        let (scrolled, _) = run_frame_targeting_surface(
            &ctx,
            vec![
                egui::Event::PointerMoved(egui::pos2(50.0, 50.0)),
                middle(true, egui::pos2(50.0, 50.0)),
            ],
            &mut widget,
            (64, 64),
            &mut camera,
            None,
            panel_surface(),
        );
        let (dragged, _) = run_frame_targeting_surface(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(70.0, 60.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            None,
            panel_surface(),
        );
        assert_eq!(
            (scrolled.pan_by, dragged.pan_by),
            ((0, 0), (0, 0)),
            "a middle drag that starts over a panel must not pan the canvas"
        );
        assert_eq!(camera.pan(), pan_before, "camera unchanged");
    }

    /// PANEL MASK — hover preview: the brush cursor follows the pointer with no
    /// button held, so it must disappear over a panel (a palette covers the
    /// cursor and masks the canvas) while staying on the canvas otherwise.
    #[test]
    fn brush_cursor_is_not_painted_over_a_panel_surface() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        let preview = Some((
            BrushSpec::new(3, crate::core::brush::BrushShape::Square),
            DrawMode::Pen,
            Color::rgb(255, 0, 0),
        ));
        let count_footprints = |shapes: &[egui::Shape]| {
            shapes
                .iter()
                .filter(|shape| matches!(shape, egui::Shape::Rect(_)))
                .count()
        };

        let (_, over_canvas) = run_frame_targeting_surface(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            preview,
            SurfaceId::Canvas,
        );
        let (_, over_panel) = run_frame_targeting_surface(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            preview,
            panel_surface(),
        );
        assert!(
            count_footprints(&over_canvas) > count_footprints(&over_panel),
            "the brush cursor must paint over the canvas ({}) but not over a panel ({} rect shapes)",
            count_footprints(&over_canvas),
            count_footprints(&over_panel),
        );
    }

    #[test]
    fn canvas_draw_rect_at_origin() {
        let rect = canvas_draw_rect(egui::pos2(0.0, 0.0), 1.0, (0, 0), (32, 32));
        assert_eq!(
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(32.0, 32.0))
        );
    }

    #[test]
    fn canvas_draw_rect_with_pan_and_scale() {
        let rect = canvas_draw_rect(egui::pos2(10.0, 20.0), 2.0, (5, -3), (16, 16));
        assert_eq!(rect.min, egui::pos2(15.0, 17.0));
        assert_eq!(rect.size(), egui::vec2(32.0, 32.0));
    }

    #[test]
    fn canvas_draw_rect_zero_canvas() {
        let rect = canvas_draw_rect(egui::pos2(4.0, 4.0), 1.0, (0, 0), (0, 0));
        assert_eq!(rect.min, egui::pos2(4.0, 4.0));
        assert_eq!(rect.size(), egui::vec2(0.0, 0.0));
    }

    #[test]
    fn tile_grid_suppresses_lines_when_hidden_or_too_dense() {
        let draw = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 200.0));
        let viewport = draw;
        let (vertical, horizontal) =
            tile_grid_lines((64, 32), TileSize::square(16), 4.0, draw, viewport, true);
        assert_eq!(vertical, vec![16, 32, 48]);
        assert_eq!(horizontal, vec![16]);
        assert!(tile_grid_lines((64, 32), TileSize::square(16), 4.0, draw, viewport, false)
            .0
            .is_empty());
        // 16 px regions at 20% zoom are 3.2 pt apart: suppressed as too dense.
        assert!(tile_grid_lines((64, 32), TileSize::square(16), 0.2, draw, viewport, true)
            .0
            .is_empty());
    }

    #[test]
    fn tile_grid_lines_use_per_axis_steps() {
        let draw = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 200.0));
        let viewport = draw;
        let (vertical, horizontal) =
            tile_grid_lines((64, 32), TileSize::new(16, 8), 4.0, draw, viewport, true);
        assert_eq!(vertical, vec![16, 32, 48]);
        assert_eq!(horizontal, vec![8, 16, 24]);

        // A dense short axis is suppressed alone: at 3x, 16 px (48 pt) is kept
        // but 1 px (3 pt) is below MIN_TILE_LINE_SPACING, so only vertical lines
        // remain.
        let (vertical, horizontal) =
            tile_grid_lines((64, 32), TileSize::new(16, 1), 3.0, draw, viewport, true);
        assert!(!vertical.is_empty());
        assert!(horizontal.is_empty());
    }

    #[test]
    fn tile_grid_lines_are_culled_to_the_viewport() {
        let draw = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(4000.0, 4000.0));
        let viewport = egui::Rect::from_min_max(egui::pos2(100.0, 200.0), egui::pos2(500.0, 600.0));
        let (vertical, horizontal) =
            tile_grid_lines((1000, 1000), TileSize::square(16), 4.0, draw, viewport, true);
        assert_eq!(vertical.first(), Some(&32));
        assert_eq!(vertical.last(), Some(&112));
        assert!(vertical.iter().all(|coord| coord % 16 == 0));
        assert_eq!(horizontal.first(), Some(&64));
        assert_eq!(horizontal.last(), Some(&144));
    }

    #[test]
    fn tile_grid_handles_fractional_zoom_and_large_canvas_with_bounded_output() {
        let draw = egui::Rect::from_min_max(egui::pos2(-10.0, 5.0), egui::pos2(9990.0, 10005.0));
        let viewport = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(800.0, 600.0));
        let (vertical, horizontal) =
            tile_grid_lines((1_000_000, 1_000_000), TileSize::square(16), 4.5, draw, viewport, true);
        assert!(!vertical.is_empty());
        assert!(vertical.len() < 200);
        assert!(horizontal.len() < 200);
        assert!(vertical.iter().all(|coord| coord % 16 == 0));
    }

    #[test]
    fn painter_outputs_grid_lines_only_when_visible() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let (_, visible_shapes) =
            run_frame_with_shapes_state(&ctx, vec![], &mut widget, (32, 32), &mut camera, true);
        let visible_lines = visible_shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::LineSegment { .. }))
            .count();
        assert_eq!(visible_lines, 2);

        let (_, hidden_shapes) =
            run_frame_with_shapes_state(&ctx, vec![], &mut widget, (32, 32), &mut camera, false);
        let hidden_lines = hidden_shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::LineSegment { .. }))
            .count();
        assert_eq!(hidden_lines, 0);
    }

    #[test]
    fn screen_to_canvas_pt_inside() {
        let pt = screen_to_canvas_pt(
            egui::pos2(10.0, 20.0),
            egui::pos2(0.0, 0.0),
            1.0,
            (0, 0),
            (32, 32),
        );
        assert_eq!(pt, Some((10, 20)));
    }

    #[test]
    fn screen_to_canvas_pt_outside_returns_none() {
        assert_eq!(
            screen_to_canvas_pt(
                egui::pos2(32.0, 0.0),
                egui::pos2(0.0, 0.0),
                1.0,
                (0, 0),
                (32, 32)
            ),
            None
        );
        assert_eq!(
            screen_to_canvas_pt(
                egui::pos2(0.0, -1.0),
                egui::pos2(0.0, 0.0),
                1.0,
                (0, 0),
                (32, 32)
            ),
            None
        );
    }

    #[test]
    fn screen_to_canvas_pt_floor_semantics() {
        // Canvas pixel c spans screen [pan + c*scale, pan + (c+1)*scale): floor.
        assert_eq!(
            screen_to_canvas_pt(
                egui::pos2(1.9, 1.9),
                egui::pos2(0.0, 0.0),
                1.0,
                (0, 0),
                (32, 32)
            ),
            Some((1, 1))
        );
        assert_eq!(
            screen_to_canvas_pt(
                egui::pos2(2.0, 2.0),
                egui::pos2(0.0, 0.0),
                1.0,
                (0, 0),
                (32, 32)
            ),
            Some((2, 2))
        );
    }

    #[test]
    fn screen_to_canvas_pt_with_pan_and_scale() {
        // origin (10,20), scale 2, pan (5,-3): canvas (0,0) spans screen
        // [15,17) x [17,19), canvas (1,1) spans [17,19) x [19,21).
        assert_eq!(
            screen_to_canvas_pt(
                egui::pos2(15.0, 17.0),
                egui::pos2(10.0, 20.0),
                2.0,
                (5, -3),
                (16, 16)
            ),
            Some((0, 0))
        );
        assert_eq!(
            screen_to_canvas_pt(
                egui::pos2(16.9, 18.9),
                egui::pos2(10.0, 20.0),
                2.0,
                (5, -3),
                (16, 16)
            ),
            Some((0, 0))
        );
        assert_eq!(
            screen_to_canvas_pt(
                egui::pos2(17.0, 19.0),
                egui::pos2(10.0, 20.0),
                2.0,
                (5, -3),
                (16, 16)
            ),
            Some((1, 1))
        );
    }

    #[test]
    fn screen_to_canvas_clamped_clamps_all_edges() {
        // Given: a 2× zoom, panned canvas whose draw rect spans x 15..47, y 17..41.
        let mut widget = CanvasWidget::new();
        widget.last_origin = egui::pos2(10.0, 20.0);
        let mut camera = Camera::new();
        camera.pan_by(5, -3);
        camera.set_zoom_percent(200);
        let canvas_size = (16, 12);

        // When: positions lie past each draw-rect edge and just inside two edges.
        // Then: each axis clamps into the inclusive canvas pixel range.
        assert_eq!(
            widget.screen_to_canvas_clamped(egui::pos2(-500.0, 20.0), camera, canvas_size),
            (0, 1)
        );
        assert_eq!(
            widget.screen_to_canvas_clamped(egui::pos2(60.0, 20.0), camera, canvas_size),
            (15, 1)
        );
        assert_eq!(
            widget.screen_to_canvas_clamped(egui::pos2(20.0, 0.0), camera, canvas_size),
            (2, 0)
        );
        assert_eq!(
            widget.screen_to_canvas_clamped(egui::pos2(20.0, 60.0), camera, canvas_size),
            (2, 11)
        );
        assert_eq!(
            widget.screen_to_canvas_clamped(egui::pos2(15.0, 17.0), camera, canvas_size),
            (0, 0)
        );
        assert_eq!(
            widget.screen_to_canvas_clamped(egui::pos2(46.9, 40.9), camera, canvas_size),
            (15, 11)
        );
    }

    #[test]
    fn gizmo_hit_at_uses_the_widget_origin_and_camera() {
        // EK SORUN: the App's global transform path needs the gizmo hit-test
        // resolved in SCREEN coordinates through the widget's captured origin
        // and the camera's pan/scale — and it must work for points OUTSIDE the
        // canvas draw rect (that is the whole point of the global path).
        let mut widget = CanvasWidget::new();
        widget.last_origin = egui::pos2(100.0, 100.0);
        let mut camera = Camera::new();
        camera.pan_by(5, 5);
        camera.set_zoom_percent(200);
        let corners = [(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 20.0)];
        // Canvas (10,10) at 2× zoom, origin (100,100), pan (5,5) →
        // screen 100 + 5 + 10*2 = 125.
        assert_eq!(
            widget.gizmo_hit_at(egui::pos2(125.0, 125.0), corners, camera),
            GizmoHit::ScaleNW,
        );
        // A point well outside the tiny canvas/bbox resolves to no handle
        // rather than panicking or wrapping.
        assert_eq!(
            widget.gizmo_hit_at(egui::pos2(200.0, 200.0), corners, camera),
            GizmoHit::None,
        );
    }

    #[test]
    fn screen_to_canvas_clamped_is_total_for_far_outside_positions() {
        // Given: a one-pixel canvas and extreme screen coordinates.
        let mut widget = CanvasWidget::new();
        widget.last_origin = egui::pos2(-40.0, 25.0);
        let camera = Camera::new();

        // When: positions are far outside the draw rect.
        // Then: the total mapping still returns a valid in-canvas pixel.
        for pos in [
            egui::pos2(f32::MIN, f32::MIN),
            egui::pos2(f32::MAX, f32::MAX),
            egui::pos2(0.0, 0.0),
        ] {
            let (x, y) = widget.screen_to_canvas_clamped(pos, camera, (1, 1));
            assert_eq!((x, y), (0, 0), "pos {pos:?} escaped the canvas bounds");
        }
    }

    #[test]
    fn canvas_footprint_anchor_requires_footprint_overlap() {
        let origin = egui::pos2(0.0, 0.0);
        // A 16 px footprint at anchor x = 32 (just past the 32 px canvas edge)
        // spans 24..=40 and still covers canvas pixels; at x = 48 it spans
        // 40..=56 and is entirely outside.
        assert_eq!(
            canvas_footprint_anchor(egui::pos2(32.0, 10.0), origin, 1.0, (0, 0), (32, 32), 16),
            Some((32, 10))
        );
        assert_eq!(
            canvas_footprint_anchor(egui::pos2(48.0, 10.0), origin, 1.0, (0, 0), (32, 32), 16),
            None
        );
        // A 1 px footprint only counts while the cursor cell is on the canvas.
        assert_eq!(
            canvas_footprint_anchor(egui::pos2(31.9, 10.0), origin, 1.0, (0, 0), (32, 32), 1),
            Some((31, 10))
        );
        assert_eq!(
            canvas_footprint_anchor(egui::pos2(32.0, 10.0), origin, 1.0, (0, 0), (32, 32), 1),
            None
        );
    }

    #[test]
    fn zoom_steps_positive_negative_zero() {
        assert_eq!(zoom_steps(0.0), 0);
        assert_eq!(zoom_steps(36.0), 1);
        assert_eq!(zoom_steps(-36.0), -1);
        assert_eq!(zoom_steps(108.0), 3);
        assert_eq!(zoom_steps(-108.0), -3);
        assert_eq!(zoom_steps(12.0), 1);
        assert_eq!(zoom_steps(-12.0), -1);
    }

    #[test]
    fn ui_reports_default_interactions() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let result = run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
        );
        assert_eq!(result, CanvasInteractions::default());
        assert_eq!(result.updated_camera.zoom_percent(), 100);
        assert_eq!(result.updated_camera.pan(), (0, 0));
    }

    #[test]
    fn primary_drag_reports_stroke() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        // Move past the click threshold → drag starts.
        let started = run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))],
            &mut widget,
            (64, 64),
        );
        assert!(started.stroke_started);
        assert_eq!(started.stroke_point, Some((20, 20)));
        let continued = run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(30.0, 30.0))],
            &mut widget,
            (64, 64),
        );
        assert!(!continued.stroke_started);
        assert_eq!(continued.stroke_point, Some((30, 30)));
        let ended = run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(30.0, 30.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        assert!(ended.stroke_ended);
    }

    #[test]
    fn click_without_drag_no_stroke() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        let released = run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        assert!(!released.stroke_started);
        assert_eq!(released.stroke_point, None);
        assert!(!released.stroke_ended);
    }

    #[test]
    fn middle_drag_pans_camera() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(50.0, 50.0),
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let panned = run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(60.0, 55.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert_eq!(panned.pan_by, (10, 5));
        assert_eq!(camera.pan(), (10, 5));
    }

    #[test]
    fn middle_drag_never_reports_stroke() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let pressed = run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(50.0, 50.0),
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let dragged = run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(60.0, 55.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let released = run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(60.0, 55.0),
                button: egui::PointerButton::Middle,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );

        for interaction in [pressed, dragged, released] {
            assert!(!interaction.stroke_started);
            assert_eq!(interaction.stroke_point, None);
            assert!(!interaction.stroke_ended);
        }
    }

    #[test]
    fn scroll_wheel_zooms_in() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let zoomed = run_frame_with_camera(
            &ctx,
            vec![wheel(1.0, egui::Modifiers::NONE)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert!(camera.zoom_percent() > 100, "plain scroll zooms in");
        let (_, pos) = zoomed.zoom.expect("zoom interaction");
        assert_eq!(pos, egui::pos2(50.0, 50.0));
    }

    #[test]
    fn scroll_wheel_zooms_out() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let zoomed = run_frame_with_camera(
            &ctx,
            vec![wheel(-1.0, egui::Modifiers::NONE)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert!(camera.zoom_percent() < 100, "plain scroll zooms out");
        assert!(zoomed.zoom.is_some());
    }

    #[test]
    fn zoom_clamped_at_max() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(Camera::MAX_ZOOM_PERCENT);
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let zoomed = run_frame_with_camera(
            &ctx,
            vec![wheel(1.0, egui::Modifiers::NONE)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert_eq!(camera.zoom_percent(), Camera::MAX_ZOOM_PERCENT);
        assert_eq!(zoomed.zoom, None);
    }

    #[test]
    fn zoom_anchors_at_pointer() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        run_frame_with_camera(
            &ctx,
            vec![wheel(1.0, egui::Modifiers::NONE)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert!(camera.zoom_percent() > 100);
        assert_eq!(camera.canvas_to_screen(50, 50), (50, 50));
    }

    #[test]
    fn hollow_brush_preview_stays_aligned_with_rendered_canvas_during_zoom() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let theme = crate::ui::theme::Theme::default_dark();
        let stroke_color = theme.colors.selection_stroke32();
        let brush = Some(crate::core::brush::BrushSpec::new(
            1,
            crate::core::brush::BrushShape::Square,
        ));
        let pointer_pos = egui::pos2(62.0, 62.0);

        let collect_hollow_rects = |output: &egui::FullOutput| -> Vec<egui::Rect> {
            let mut out = Vec::new();
            for clipped in &output.shapes {
                if let egui::epaint::Shape::Rect(rect_shape) = &clipped.shape {
                    if rect_shape.fill == stroke_color {
                        out.push(rect_shape.rect);
                    }
                }
            }
            out
        };

        let run_eraser_frame =
            |widget: &mut CanvasWidget, camera: &mut Camera, events: Vec<egui::Event>| {
                let capture = crate::ui::input_capture::InputCapture::new();
                capture.set_target(Some(crate::ui::input_capture::SurfaceId::Canvas));
                let mut interactions = CanvasInteractions::default();
                let raw = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 400.0),
                    )),
                    events,
                    ..Default::default()
                };
                let mut output = ctx.run_ui(raw, |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            interactions = widget.ui(
                                ui,
                                &theme.colors,
                                &capture,
                                egui::TextureId::Managed(0),
                                zoomed_view(brush, DrawMode::Eraser, Color::BLACK, None, None),
                                *camera,
                            );
                        });
                });
                output.textures_delta.clear();
                *camera = interactions.updated_camera;
                (interactions, output)
            };

        let (_, baseline_out) = run_eraser_frame(
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(pointer_pos)],
        );
        let baseline_rects = collect_hollow_rects(&baseline_out);
        assert_eq!(baseline_rects.len(), 4);

        let (zoom_interactions, zoom_out) = run_eraser_frame(
            &mut widget,
            &mut camera,
            vec![
                egui::Event::PointerMoved(pointer_pos),
                wheel(1.0, egui::Modifiers::NONE),
            ],
        );
        assert!(zoom_interactions.zoom.is_some());
        assert_ne!(camera.pan(), (0, 0));

        // On the zoom frame itself, the canvas texture is still drawn with the
        // pre-zoom `render_camera`, so the hollow brush preview must stay
        // aligned with that same `render_camera` instead of jumping to the
        // post-zoom pan while keeping the pre-zoom scale.
        let zoom_rects = collect_hollow_rects(&zoom_out);
        assert_eq!(zoom_rects, baseline_rects);
    }

    #[test]
    fn ctrl_scroll_reports_shape_cycle_without_zooming() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let scrolled = run_frame_with_camera(
            &ctx,
            vec![wheel(1.0, egui::Modifiers::CTRL)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert_eq!(camera.zoom_percent(), 100, "Ctrl+scroll must not zoom");
        assert_eq!(scrolled.zoom, None);
        assert_eq!(
            scrolled.brush_scroll, 0.0,
            "Ctrl+scroll must not report brush size input"
        );
        assert_eq!(
            scrolled.shape_cycle, 1,
            "Ctrl+scroll up reports one cycle step"
        );
    }

    #[test]
    fn alt_scroll_reports_scatter_scroll_without_zooming() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let scrolled = run_frame_with_camera(
            &ctx,
            vec![wheel(1.0, egui::Modifiers::ALT)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert_eq!(camera.zoom_percent(), 100, "Alt+scroll must not zoom");
        assert_eq!(scrolled.zoom, None);
        assert_eq!(
            scrolled.brush_scroll, 0.0,
            "Alt+scroll must not report brush size input"
        );
        assert_eq!(
            scrolled.shape_cycle, 0,
            "Alt+scroll must not cycle the shape"
        );
        assert!(
            scrolled.scatter_scroll > 0.0,
            "Alt+scroll up reports scatter input"
        );

        let scrolled = run_frame_with_camera(
            &ctx,
            vec![wheel(-1.0, egui::Modifiers::ALT)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert!(
            scrolled.scatter_scroll < 0.0,
            "Alt+scroll down reports the other direction"
        );
        assert_eq!(camera.zoom_percent(), 100);
    }

    #[test]
    fn shift_scroll_reports_brush_scroll_without_zooming() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        let scrolled = run_frame_with_camera(
            &ctx,
            vec![wheel(1.0, egui::Modifiers::SHIFT)],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        assert_eq!(camera.zoom_percent(), 100, "Shift+scroll must not zoom");
        assert_eq!(scrolled.zoom, None);
        assert!(
            scrolled.brush_scroll != 0.0,
            "Shift+scroll reports brush input"
        );
    }

    #[test]
    fn plain_scroll_zooms_continuously() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        run_frame_with_camera(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(50.0, 50.0))],
            &mut widget,
            (64, 64),
            &mut camera,
            true,
        );
        // A small scroll zooms less than a full notch: continuous, not stepped.
        let mut small_camera = Camera::new();
        let mut big_camera = Camera::new();
        for _ in 0..3 {
            run_frame_with_camera(
                &ctx,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 5.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                }],
                &mut widget,
                (64, 64),
                &mut small_camera,
                true,
            );
            run_frame_with_camera(
                &ctx,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 40.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                }],
                &mut widget,
                (64, 64),
                &mut big_camera,
                true,
            );
        }
        assert!(
            small_camera.zoom_percent() > 100,
            "small scroll still zooms in"
        );
        assert!(
            small_camera.zoom_percent() < big_camera.zoom_percent(),
            "zoom must be continuous: {} vs {}",
            small_camera.zoom_percent(),
            big_camera.zoom_percent()
        );
    }

    #[test]
    fn brush_cursor_outline_is_hollow() {
        // 1 px square: four silhouette edges.
        let one = crate::core::brush::BrushSpec::PENCIL_1PX;
        assert_eq!(brush_outline_segments(&one.stamp_offsets()).len(), 4);

        // 2x2 square: the shared interior edge is not emitted (8 edges, not 16).
        let two =
            crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square);
        assert_eq!(brush_outline_segments(&two.stamp_offsets()).len(), 8);

        // Round 3: every edge is on the circle's silhouette.
        let round =
            crate::core::brush::BrushSpec::sanitize(3, crate::core::brush::BrushShape::Round);
        let segments = brush_outline_segments(&round.stamp_offsets());
        assert!(!segments.is_empty());
        assert!(segments.len() < 16);
    }

    #[test]
    fn brush_cursor_outline_is_centered_on_the_cursor() {
        // Even sizes straddle the pixel boundary at 0, so the silhouette's
        // bounding box (in pixel-edge coordinates) is centered on the cursor.
        for size in [2u8, 4u8] {
            for shape in [
                crate::core::brush::BrushShape::Square,
                crate::core::brush::BrushShape::Round,
            ] {
                let spec = crate::core::brush::BrushSpec::new(size, shape);
                let segments = brush_outline_segments(&spec.stamp_offsets());
                let mut min_x = i32::MAX;
                let mut max_x = i32::MIN;
                let mut min_y = i32::MAX;
                let mut max_y = i32::MIN;
                for &((x0, y0), (x1, y1)) in &segments {
                    min_x = min_x.min(x0).min(x1);
                    max_x = max_x.max(x0).max(x1);
                    min_y = min_y.min(y0).min(y1);
                    max_y = max_y.max(y0).max(y1);
                }
                assert_eq!(
                    (min_x + max_x) as f32 / 2.0,
                    0.0,
                    "size {size} {shape:?} outline x not centered"
                );
                assert_eq!(
                    (min_y + max_y) as f32 / 2.0,
                    0.0,
                    "size {size} {shape:?} outline y not centered"
                );
            }
        }
    }

    #[test]
    fn brush_cursor_is_painted_while_hovering_with_a_brush() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let cursor_fills = |shapes: &[egui::Shape]| {
            let color = Theme::default_dark().colors.selection_stroke32();
            shapes
                .iter()
                .filter(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == color))
                .count()
        };
        let (_, shapes) = run_frame_with_shapes_state(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut camera,
            true,
        );
        assert_eq!(cursor_fills(&shapes), 0, "no brush, no cursor");

        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut camera,
            Some((
                crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square),
                DrawMode::Eraser,
                Color::BLACK,
            )),
        );
        assert_eq!(
            cursor_fills(&shapes),
            8,
            "a 2x2 eraser shows its hollow silhouette (8 edges)"
        );
    }

    #[test]
    fn pencil_cursor_preview_is_filled_with_the_primary_color() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let spec =
            crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square);
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut camera,
            Some((spec, DrawMode::Pen, primary)),
        );
        let filled = shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == primary32))
            .count();
        assert_eq!(
            filled,
            spec.stamp_offsets().len(),
            "the pen preview paints one solid cell per footprint pixel"
        );
    }

    /// Canvas cells the pen preview paints, recovered from the filled `Rect`
    /// shapes (origin (0,0), pan (0,0), so `rect.min / scale` is the cell).
    fn preview_cells(shapes: &[egui::Shape], scale: f32, color: egui::Color32) -> Vec<(i32, i32)> {
        let mut cells: Vec<(i32, i32)> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == color => Some((
                    (rect.rect.min.x / scale).round() as i32,
                    (rect.rect.min.y / scale).round() as i32,
                )),
                _ => None,
            })
            .collect();
        cells.sort_unstable();
        cells
    }

    /// The deduped, canvas-clipped footprint union `samples × spec.stamp_offsets()`:
    /// the cells a commit lands along the samples.
    fn expected_footprint_cells(
        samples: &[(i32, i32)],
        spec: crate::core::brush::BrushSpec,
        canvas: (i32, i32),
    ) -> Vec<(i32, i32)> {
        let offsets = spec.stamp_offsets();
        let mut cells: std::collections::BTreeSet<(i32, i32)> = std::collections::BTreeSet::new();
        for &(sx, sy) in samples {
            for &(dx, dy) in &offsets {
                let (x, y) = (sx + dx, sy + dy);
                if (0..canvas.0).contains(&x) && (0..canvas.1).contains(&y) {
                    cells.insert((x, y));
                }
            }
        }
        cells.into_iter().collect()
    }

    /// Pixels a stroke paints when anchored at `anchor` with `spec`.
    fn painted_pixels(spec: crate::core::brush::BrushSpec, anchor: (i32, i32)) -> Vec<(i32, i32)> {
        let mut buf = crate::core::buffer::PixelBuffer::new(64, 64);
        crate::core::brush::stamp_at(
            &mut buf,
            crate::core::brush::DrawTool {
                spec,
                mode: DrawMode::Pen,
                color: Color::WHITE,
                scatter: 0,
                scatter_shape: crate::core::brush::ScatterShape::Square,
                tail: 0,
            },
            anchor.0,
            anchor.1,
        );
        let mut pixels: Vec<(i32, i32)> = (0..64i32)
            .flat_map(|x| (0..64i32).map(move |y| (x, y)))
            .filter(|&(x, y)| buf.get_pixel(x as usize, y as usize) == Some(Color::WHITE))
            .collect();
        pixels.sort_unstable();
        pixels
    }

    #[test]
    fn preview_cells_match_the_painted_pixels_for_even_sizes() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let scale = 4.0f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        // Pointer at screen (10,10) → canvas pos 2.5; an even brush anchors at
        // the nearest lattice point, round(2.5) = 3.
        for shape in [
            crate::core::brush::BrushShape::Square,
            crate::core::brush::BrushShape::Round,
        ] {
            for size in [2u8, 4, 6] {
                let spec = crate::core::brush::BrushSpec::new(size, shape);
                let (_, shapes) = run_frame_with_shapes_state_with_brush(
                    &ctx,
                    vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
                    &mut widget,
                    (32, 32),
                    &mut camera,
                    Some((spec, DrawMode::Pen, primary)),
                );
                let preview = preview_cells(&shapes, scale, primary32);
                let painted = painted_pixels(spec, (3, 3));
                assert_eq!(
                    preview, painted,
                    "size {size} {shape:?}: preview cells must equal the painted pixels"
                );
            }
        }
    }

    #[test]
    fn line_preview_paints_the_brush_footprint_along_the_segment() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let camera = Camera::new();
        let scale = camera.scale() as f32;
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let view = CanvasView {
            canvas_size: (32, 32),
            grid_visible: false,
            brush: Some(crate::core::brush::BrushSpec::new(
                1,
                crate::core::brush::BrushShape::Square,
            )),
            brush_color: Color::rgb(200, 30, 40),
            line: Some(LinePreview {
                anchor: (2, 10),
                end: (20, 10),
            }),
            ..Default::default()
        };
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events: Vec::new(),
            ..Default::default()
        };
        let theme = Theme::default_dark().colors;
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    TEST_CAPTURE.with(|capture| {
                        capture.set_target(Some(SurfaceId::Canvas));
                        let _ = widget.ui(
                            ui,
                            &theme,
                            capture,
                            egui::TextureId::default(),
                            view.clone(),
                            camera,
                        );
                    });
                });
        });
        output.textures_delta.clear();
        let shapes: Vec<egui::Shape> = output.shapes.into_iter().map(|cs| cs.shape).collect();
        // No pointer hovered, so the only painted cells are the line preview.
        let cells = preview_cells(&shapes, scale, primary32);
        let expected: Vec<(i32, i32)> = (2..=20).map(|x| (x, 10)).collect();
        assert_eq!(
            cells, expected,
            "the preview must paint the whole anchor→end segment"
        );
    }

    #[test]
    fn canvas_reports_a_double_click_on_the_canvas() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let pos = egui::pos2(20.0, 20.0);
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(pos)],
            &mut widget,
            (32, 32),
        );
        run_frame(&ctx, vec![primary_press(pos)], &mut widget, (32, 32));
        run_frame(&ctx, vec![primary_release(pos)], &mut widget, (32, 32));
        run_frame(&ctx, vec![primary_press(pos)], &mut widget, (32, 32));
        let result = run_frame(&ctx, vec![primary_release(pos)], &mut widget, (32, 32));
        assert_eq!(
            result.double_clicked,
            Some((20, 20)),
            "the second click must be reported as a canvas double-click"
        );
    }

    fn primary_press(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn primary_release(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn curve_overlay_paints_the_polyline_and_gizmos_in_theme_colors() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let theme = Theme::default_dark().colors;
        widget.set_overlay(CanvasOverlay::Curve {
            points: vec![(2.0, 8.0), (8.0, 8.0), (14.0, 8.0)],
            samples: vec![(2, 8), (8, 8), (14, 8)],
            gizmos: [(6.0, 8.0), (8.0, 8.0), (10.0, 8.0), (12.0, 8.0)],
            hovered: Some(0),
        });
        let (_, shapes) = run_frame_with_shapes(&ctx, Vec::new(), &mut widget, (16, 16));

        let fills: Vec<egui::Color32> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) => Some(rect.fill),
                _ => None,
            })
            .collect();
        assert!(
            fills.contains(&theme.gizmo_fill_normal32()),
            "gizmos must use the theme's normal gizmo fill"
        );
        assert!(
            fills.contains(&theme.gizmo_fill_hover32()),
            "the hovered gizmo must use the theme's hover fill"
        );
        let outline = theme.gizmo_outline32();
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::Shape::LineSegment { stroke, .. } if stroke.color == outline
            )),
            "the polyline must be stroked in the theme's gizmo outline color"
        );
    }

    #[test]
    fn curve_gizmos_paint_above_the_curve_line() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let theme = Theme::default_dark().colors;
        widget.set_overlay(CanvasOverlay::Curve {
            points: vec![(2.0, 8.0), (8.0, 8.0), (14.0, 8.0)],
            samples: vec![(2, 8), (8, 8), (14, 8)],
            gizmos: [(6.0, 8.0), (8.0, 8.0), (10.0, 8.0), (12.0, 8.0)],
            hovered: Some(0),
        });
        let (_, shapes) = run_frame_with_shapes(&ctx, Vec::new(), &mut widget, (16, 16));

        let last_line = shapes
            .iter()
            .rposition(|shape| {
                matches!(
                    shape,
                    egui::Shape::LineSegment { stroke, .. } if stroke.color == theme.gizmo_outline32()
                )
            })
            .expect("the curve polyline must be stroked");
        let first_gizmo = shapes
            .iter()
            .position(|shape| {
                matches!(
                    shape,
                    egui::Shape::Rect(rect)
                        if rect.fill == theme.gizmo_fill_normal32()
                            || rect.fill == theme.gizmo_fill_hover32()
                )
            })
            .expect("the curve gizmos must be filled");
        assert!(
            first_gizmo > last_line,
            "the gizmos must paint after the curve polyline: line {last_line}, gizmo {first_gizmo}"
        );
    }

    #[test]
    fn curve_transform_hides_the_brush_cursor() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let cursor_color = Theme::default_dark().colors.selection_stroke32();
        let cursor_edges = |shapes: &[egui::Shape]| {
            shapes
                .iter()
                .filter_map(|shape| match shape {
                    egui::Shape::Rect(rect) if rect.fill == cursor_color => Some(rect.rect),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let spec =
            crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square);
        let preview = Some((spec, DrawMode::Eraser, Color::BLACK));
        let pointer = vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))];

        // Without a transform the eraser's hollow footprint preview is painted
        // around its anchor: canvas (3,3) → silhouette edges at screen y 8..16.
let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut camera,
            preview,
        );
        assert_eq!(
            cursor_edges(&shapes).len(),
            8,
            "a 2x2 eraser shows its hollow silhouette"
        );

        // While the curve transform's overlay owns the canvas, the pointer's
        // cursor must not emit geometry: the only hollow-eraser geometry is the
        // curve footprint, painted down at canvas y 9..11.
        widget.set_overlay(CanvasOverlay::Curve {
            points: vec![(2.0, 10.0), (14.0, 10.0)],
            samples: (2..=14).map(|x| (x, 10)).collect(),
            gizmos: [(6.0, 10.0), (8.0, 10.0), (10.0, 10.0), (12.0, 10.0)],
            hovered: None,
        });
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            pointer,
            &mut widget,
            (32, 32),
            &mut camera,
            preview,
        );
        let edges = cursor_edges(&shapes);
        assert!(
            !edges.is_empty(),
            "the curve transform must preview the footprint it will commit"
        );
        assert!(
            edges.iter().all(|rect| rect.min.y >= 32.0),
            "the brush cursor at canvas (3,3) must stay hidden: {edges:?}"
        );
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::Shape::Rect(rect) if rect.fill == Theme::default_dark().colors.gizmo_fill_normal32()
            )),
            "the curve gizmos must stay painted"
        );
    }

    #[test]
    fn curve_transform_previews_the_committed_footprint_cells() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let scale = 4.0f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let spec =
            crate::core::brush::BrushSpec::sanitize(3, crate::core::brush::BrushShape::Square);
        let samples: Vec<(i32, i32)> = (0..=8).map(|x| (x, 8)).collect();
        widget.set_overlay(CanvasOverlay::Curve {
            points: vec![(0.0, 8.0), (8.0, 8.0)],
            samples: samples.clone(),
            gizmos: [(1.0, 8.0), (3.0, 8.0), (5.0, 8.0), (7.0, 8.0)],
            hovered: None,
        });
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            Vec::new(),
            &mut widget,
            (32, 32),
            &mut camera,
            Some((spec, DrawMode::Pen, primary)),
        );
        let expected = expected_footprint_cells(&samples, spec, (32, 32));
        let painted = preview_cells(&shapes, scale, primary32);
        assert_eq!(
            painted, expected,
            "the transform preview must paint the deduped footprint union along the samples"
        );
        assert!(
            painted.len() > samples.len(),
            "the preview must be a footprint, not a thin line: {} cells for {} samples",
            painted.len(),
            samples.len()
        );
    }

    /// UX item 1/4/6: the Tile-placer overlay paints the hovered cell's green
    /// outline, the REAL tile content (opaque pixels in their own colours)
    /// and the green tile_id labels.
    #[test]
    fn tile_placer_overlay_paints_hover_outline_real_pixels_and_index_labels() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let theme = Theme::default_dark().colors;
        let green = theme.onion_next_tint32();
        // 32x32 canvas at 100%, tile_size 16: hover cell (1,0) spans canvas
        // (16,0)-(32,16). A 2x2 tile preview with two distinct opaque colours
        // occupies (16,0)-(18,2), and two assigned cells carry ids 5 and 7.
        let red = Color::rgb(255, 0, 0);
        let blue = Color::rgb(0, 0, 255);
        widget.set_overlay(CanvasOverlay::TilePlacer {
            hover_cell: Some((1, 0)),
            gesture_cells: vec![],
            preview_pixels: vec![(16, 0, red), (17, 0, blue), (16, 1, red), (17, 1, blue)],
            indices: vec![((0, 0), 5), ((1, 0), 7)],
        });
        let (_, shapes) = run_frame_with_shapes(&ctx, Vec::new(), &mut widget, (32, 32));

        // Item 1: the hover outline is a green-stroked rect over the cell.
        let hover_outline = shapes.iter().any(|shape| match shape {
            egui::Shape::Rect(rect) => {
                rect.stroke.color == green
                    && rect.rect
                        == egui::Rect::from_min_max(egui::pos2(16.0, 0.0), egui::pos2(32.0, 16.0))
            }
            _ => false,
        });
        assert!(
            hover_outline,
            "the hovered cell must be outlined in the theme green at its bounds"
        );

        // Item 4: the REAL tile content — filled quads in the tile's own
        // colours (red + blue), NOT a green hollow silhouette.
        let red32 = egui::Color32::from_rgba_unmultiplied(255, 0, 0, 255);
        let blue32 = egui::Color32::from_rgba_unmultiplied(0, 0, 255, 255);
        let red_quads = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == red32 => Some(rect.rect),
                _ => None,
            })
            .collect::<Vec<_>>();
        let blue_quads = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == blue32 => Some(rect.rect),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            red_quads.len(),
            2,
            "the two red opaque pixels must be painted in their own colour"
        );
        assert_eq!(
            blue_quads.len(),
            2,
            "the two blue opaque pixels must be painted in their own colour"
        );
        let green_fills = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == green => Some(rect.rect),
                _ => None,
            })
            .count();
        assert_eq!(
            green_fills, 0,
            "the preview must paint the tile's real pixels, not a green silhouette"
        );

        // Item 6: the green tile_id labels at each assigned cell's top-left.
        let labels: Vec<String> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Text(text)
                    if text.galley.text() == "5" || text.galley.text() == "7" =>
                {
                    Some(text.galley.text().to_string())
                }
                _ => None,
            })
            .collect();
        assert!(
            labels.contains(&"5".to_string()) && labels.contains(&"7".to_string()),
            "assigned cells must carry their tile_id labels: {labels:?}"
        );
    }

    /// UX item 1 + Fix 1: a Tile-placer overlay with no hovered cell paints NO
    /// hover outline, and a TRANSPARENT tile pixel is not painted at all
    /// (the preview preserves transparency).
    #[test]
    fn tile_placer_overlay_without_hover_paints_no_outline_and_skips_transparent() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let theme = Theme::default_dark().colors;
        let green = theme.onion_next_tint32();
        let red = Color::rgb(255, 0, 0);
        // Pixel (17,0) carries alpha 0 — it must NOT be painted.
        widget.set_overlay(CanvasOverlay::TilePlacer {
            hover_cell: None,
            gesture_cells: vec![],
            preview_pixels: vec![(16, 0, red), (17, 0, Color::rgba(255, 0, 0, 0))],
            indices: vec![((0, 0), 5)],
        });
        let (_, shapes) = run_frame_with_shapes(&ctx, Vec::new(), &mut widget, (32, 32));
        let green_stroked: Vec<&egui::Shape> = shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.stroke.color == green))
            .collect();
        assert!(
            green_stroked.is_empty(),
            "no hovered cell means no green outline stroke"
        );
        // The opaque pixel paints red; the fully-transparent pixel (alpha 0)
        // must not appear as a red quad at (17,0).
        let red32 = egui::Color32::from_rgba_unmultiplied(255, 0, 0, 255);
        let red_quads: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill == red32 => Some(rect.rect),
                _ => None,
            })
            .collect();
        assert_eq!(
            red_quads.len(),
            1,
            "only the opaque pixel is painted: {red_quads:?}"
        );
        assert!(
            red_quads.iter().all(|rect| rect.min.x < 17.0),
            "no quad may appear at the transparent pixel (17,0): {red_quads:?}"
        );
        assert!(
            shapes.iter().any(|shape| matches!(
                shape,
                egui::Shape::Text(text) if text.galley.text() == "5"
            )),
            "index labels still paint without a hovered cell"
        );
    }

    /// Item 2: the Tile-placer preview covers EVERY cell of the in-flight
    /// gesture, not only the pointer's cell — each gesture cell gets a green
    /// outline AND its own real-pixel preview quad in place.
    #[test]
    fn tile_placer_overlay_previews_every_gesture_cell() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let theme = Theme::default_dark().colors;
        let green = theme.onion_next_tint32();
        let red = Color::rgb(255, 0, 0);
        let blue = Color::rgb(0, 0, 255);
        // Gesture stamped cells (0,0) and (1,0) -> cell rects (0,0)-(16,16) and
        // (16,0)-(32,16); the pointer hovers (2,0) -> (32,0)-(48,16). Each cell
        // carries ONE preview pixel at its origin, in its own colour.
        widget.set_overlay(CanvasOverlay::TilePlacer {
            hover_cell: Some((2, 0)),
            gesture_cells: vec![(0, 0), (1, 0)],
            preview_pixels: vec![(0, 0, red), (16, 0, blue), (32, 0, red)],
            indices: vec![],
        });
        let (_, shapes) = run_frame_with_shapes(&ctx, Vec::new(), &mut widget, (48, 16));

        let outlines: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.stroke.color == green => Some(rect.rect),
                _ => None,
            })
            .collect();
        for cell_x in 0..3u32 {
            let expected = egui::Rect::from_min_max(
                egui::pos2((cell_x * 16) as f32, 0.0),
                egui::pos2((cell_x * 16 + 16) as f32, 16.0),
            );
            assert!(
                outlines.contains(&expected),
                "cell ({cell_x},0) must be outlined in the theme green: {outlines:?}"
            );
        }
        let red32 = egui::Color32::from_rgba_unmultiplied(255, 0, 0, 255);
        let blue32 = egui::Color32::from_rgba_unmultiplied(0, 0, 255, 255);
        let fills: Vec<(egui::Color32, egui::Rect)> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect) if rect.fill != egui::Color32::TRANSPARENT => {
                    Some((rect.fill, rect.rect))
                }
                _ => None,
            })
            .filter(|(fill, _)| *fill == red32 || *fill == blue32)
            .collect();
        assert!(
            fills.contains(&(red32, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))))
                && fills.contains(&(blue32, egui::Rect::from_min_max(egui::pos2(16.0, 0.0), egui::pos2(17.0, 1.0))))
                && fills.contains(&(red32, egui::Rect::from_min_max(egui::pos2(32.0, 0.0), egui::pos2(33.0, 1.0)))),
            "each previewed cell must paint its own tile pixel in place: {fills:?}"
        );
    }

    /// Z-order: the Tile-placer preview must be the LAST thing the canvas paints,
    /// so it always reads ON TOP of the tile's baked content — nothing drawn
    /// afterwards (brush footprint, line preview) may bury it.
    #[test]
    fn tile_placer_overlay_paints_after_every_other_decoration() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let theme = Theme::default_dark().colors;
        let green = theme.onion_next_tint32();
        let red = Color::rgb(255, 0, 0);
        widget.set_overlay(CanvasOverlay::TilePlacer {
            hover_cell: Some((1, 0)),
            gesture_cells: vec![],
            preview_pixels: vec![(16, 0, red)],
            indices: vec![],
        });
        // A brush cursor is active too, so its footprint quads compete with the
        // tile preview for the same z-range.
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut Camera::new(),
            Some((
                crate::core::brush::BrushSpec::sanitize(4, crate::core::brush::BrushShape::Square),
                DrawMode::Pen,
                Color::rgb(0, 255, 0),
            )),
        );
        // The brush cursor paints green FILLED footprint quads; the tile-placer
        // preview is a green STROKE (outline) + the red tile pixel. The whole
        // preview pass must come after EVERY brush footprint quad.
        let brush_green = egui::Color32::from_rgba_unmultiplied(0, 255, 0, 255);
        let brush_fills: Vec<usize> = shapes
            .iter()
            .enumerate()
            .filter_map(|(i, shape)| match shape {
                egui::Shape::Rect(rect) if rect.fill == brush_green => Some(i),
                _ => None,
            })
            .collect();
        let green_stroke = shapes
            .iter()
            .position(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.stroke.color == green));
        let red_fill = shapes.iter().position(
            |shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == egui::Color32::from_rgba_unmultiplied(255, 0, 0, 255)),
        );
        let colors: Vec<egui::Color32> = shapes
            .iter()
            .filter_map(|s| {
                if let egui::Shape::Rect(r) = s {
                    Some(r.fill)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            !brush_fills.is_empty(),
            "the brush cursor must paint its footprint (shapes: {colors:?})"
        );
        assert!(
            green_stroke.is_some() && red_fill.is_some(),
            "both tile-placer shapes must paint"
        );
        let last_brush = *brush_fills.last().unwrap();
        assert!(
            red_fill.unwrap() > last_brush && green_stroke.unwrap() > last_brush,
            "the tile-placer preview must come after the brush footprint, got last brush fill at {last_brush}, outline at {green_stroke:?} and preview at {red_fill:?}"
        );
        // And nothing is painted after the tile-id labels either.
        let label = shapes.iter().position(|shape| {
            matches!(shape, egui::Shape::Text(text) if text.galley.text() == "1")
        });
        if let Some(label) = label {
            assert_eq!(
                label,
                shapes.len() - 1,
                "the index labels are the last shapes the canvas emits"
            );
        }
    }

    /// L1-A: the floating-pixels preview quad is painted through a clip to the
    /// canvas draw rect, so an object dragged partly/fully outside the canvas
    /// never spills its image into the letterbox/chrome.
    #[test]
    fn gizmo_preview_is_clipped_to_the_canvas_draw_rect() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let preview_id = egui::TextureId::User(4242);
        // A 4x4 canvas at 100%: draw rect (0,0)-(4,4). The preview rect is
        // placed so its screen quad extends far past the canvas on all sides.
        widget.set_overlay(CanvasOverlay::Gizmo {
            corners: [(-20.0, -20.0), (24.0, -20.0), (24.0, 24.0), (-20.0, 24.0)],
            pivot: (2.0, 2.0),
            hovered: GizmoHit::None,
            active: None,
            preview: Some((preview_id, Rect2i::new(-20, -20, 44, 44))),
            hud: None,
        });

        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            ..Default::default()
        };
        let camera = Camera::new();
        let theme = Theme::default_dark().colors;
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    TEST_CAPTURE.with(|capture| {
                        capture.set_target(Some(SurfaceId::Canvas));
                        let _ = widget.ui(
                            ui,
                            &theme,
                            capture,
                            egui::TextureId::default(),
                            CanvasView {
                                canvas_size: (4, 4),
                                grid_visible: true,
                                tile_size: TileSize::square(16),
                                ..Default::default()
                            },
                            camera,
                        );
                    });
                });
        });
        output.textures_delta.clear();

        let canvas_rect = canvas_draw_rect(widget.last_origin, 1.0, (0, 0), (4, 4));
        let preview_clip = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id == preview_id => Some(clipped.clip_rect),
                _ => None,
            })
            .expect("the gizmo preview mesh must be painted");
        assert_eq!(
            preview_clip, canvas_rect,
            "the preview image must be clipped to the canvas draw rect {canvas_rect:?}, got {preview_clip:?}"
        );
    }

    #[test]
    fn curve_transform_footprint_is_hollow_for_the_eraser() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let hollow_color = Theme::default_dark().colors.selection_stroke32();
        let spec =
            crate::core::brush::BrushSpec::sanitize(3, crate::core::brush::BrushShape::Square);
        let samples: Vec<(i32, i32)> = (0..=8).map(|x| (x, 8)).collect();
        widget.set_overlay(CanvasOverlay::Curve {
            points: vec![(0.0, 8.0), (8.0, 8.0)],
            samples: samples.clone(),
            gizmos: [(1.0, 8.0), (3.0, 8.0), (5.0, 8.0), (7.0, 8.0)],
            hovered: None,
        });
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            Vec::new(),
            &mut widget,
            (32, 32),
            &mut camera,
            Some((spec, DrawMode::Eraser, primary)),
        );
        let expected = expected_footprint_cells(&samples, spec, (32, 32));
        let expected_edges = brush_outline_segments(&expected).len();
        let hollow = shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == hollow_color))
            .count();
        let solid = shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == primary32))
            .count();
        assert_eq!(
            hollow, expected_edges,
            "the eraser transform preview must be the hollow silhouette of the footprint union"
        );
        assert!(
            hollow > samples.len(),
            "the eraser preview must outline the footprint, not a thin line: {hollow} edges for {} samples",
            samples.len()
        );
        assert_eq!(
            solid, 0,
            "the eraser preview must not fill cells with the primary color"
        );
    }

    #[test]
    fn curve_transform_footprint_paints_below_the_gizmos() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let theme = Theme::default_dark().colors;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let spec =
            crate::core::brush::BrushSpec::sanitize(3, crate::core::brush::BrushShape::Square);
        let samples: Vec<(i32, i32)> = (2..=10).map(|x| (x, 8)).collect();
        widget.set_overlay(CanvasOverlay::Curve {
            points: vec![(2.0, 8.0), (10.0, 8.0)],
            samples,
            gizmos: [(4.0, 8.0), (6.0, 8.0), (8.0, 8.0), (10.0, 8.0)],
            hovered: None,
        });
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            Vec::new(),
            &mut widget,
            (32, 32),
            &mut camera,
            Some((spec, DrawMode::Pen, primary)),
        );
        let last_footprint = shapes
            .iter()
            .rposition(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == primary32))
            .expect("the transform must preview its footprint cells");
        let first_gizmo = shapes
            .iter()
            .position(|shape| {
                matches!(
                    shape,
                    egui::Shape::Rect(rect)
                        if rect.fill == theme.gizmo_fill_normal32()
                            || rect.fill == theme.gizmo_fill_hover32()
                )
            })
            .expect("the curve gizmos must be filled");
        assert!(
            first_gizmo > last_footprint,
            "the gizmos must paint above the footprint: footprint {last_footprint}, gizmo {first_gizmo}"
        );
    }

    #[test]
    fn brush_preview_stays_visible_while_the_footprint_overlaps() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let scale = 4.0f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let spec = crate::core::brush::BrushSpec::new(16, crate::core::brush::BrushShape::Square);
        // Pointer at screen (130,10) → canvas 32.5; an even brush anchors at
        // round(32.5) = 33 and its 16 px footprint (25..=40) still covers the
        // 32 px canvas, so the preview must stay visible.
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(130.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut camera,
            Some((spec, DrawMode::Pen, primary)),
        );
        let preview = preview_cells(&shapes, scale, primary32);
        assert_eq!(
            preview.len(),
            spec.stamp_offsets().len(),
            "the preview must keep painting the whole footprint while it overlaps"
        );
        assert!(
            preview.contains(&(31, 0)),
            "the preview must reach the canvas edge: {preview:?}"
        );

        // Far outside: the footprint no longer touches the canvas and the
        // preview disappears.
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(300.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut camera,
            Some((spec, DrawMode::Pen, primary)),
        );
        assert!(
            preview_cells(&shapes, scale, primary32).is_empty(),
            "a footprint clear of the canvas must hide the preview"
        );
    }

    #[test]
    fn odd_brush_center_is_unchanged_by_the_anchor_fix() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let scale = 4.0f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        // Pointer at screen (10,10) → canvas pos 2.5; an odd brush keeps the
        // cursor cell floor(2.5) = 2, whose center is where the pointer sits.
        for shape in [
            crate::core::brush::BrushShape::Square,
            crate::core::brush::BrushShape::Round,
        ] {
            for size in [1u8, 3, 5] {
                let spec = crate::core::brush::BrushSpec::new(size, shape);
                let (_, shapes) = run_frame_with_shapes_state_with_brush(
                    &ctx,
                    vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
                    &mut widget,
                    (32, 32),
                    &mut camera,
                    Some((spec, DrawMode::Pen, primary)),
                );
                let preview = preview_cells(&shapes, scale, primary32);
                let painted = painted_pixels(spec, (2, 2));
                assert_eq!(
                    preview, painted,
                    "size {size} {shape:?}: odd preview must stay on the cursor cell"
                );
                assert!(
                    preview.contains(&(2, 2)),
                    "size {size} {shape:?}: the cursor cell (2,2) must be painted"
                );
            }
        }
    }

    #[test]
    fn eraser_cursor_preview_is_hollow() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let spec =
            crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square);
        let (_, shapes) = run_frame_with_shapes_state_with_brush(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (32, 32),
            &mut camera,
            Some((spec, DrawMode::Eraser, primary)),
        );
        let outline = Theme::default_dark().colors.selection_stroke32();
        let hollow = shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == outline))
            .count();
        assert_eq!(
            hollow, 8,
            "a 2x2 eraser shows its hollow silhouette (8 edges)"
        );
        let solid = shapes
            .iter()
            .filter(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == primary32))
            .count();
        assert_eq!(
            solid, 0,
            "the eraser preview must not fill cells with the primary color"
        );
    }

    #[test]
    fn primary_click_reports_clicked() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        let released = run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        assert_eq!(released.clicked, Some((10, 10)));
        assert!(!released.stroke_started);
        assert_eq!(released.stroke_point, None);
        assert!(!released.stroke_ended);
    }

    #[test]
    fn primary_click_outside_canvas_reports_nothing() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(200.0, 200.0))],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(200.0, 200.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        let released = run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(200.0, 200.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        assert_eq!(released.clicked, None);
    }

    #[test]
    fn secondary_press_reports_eyedropper_started() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (64, 64),
        );
        let pressed = run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        assert!(pressed.eyedropper_started);
        assert_eq!(pressed.eyedropper_point, Some((10, 10)));
        assert!(!pressed.eyedropper_ended);
    }

    #[test]
    fn secondary_drag_reports_eyedropper_point() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        let dragged = run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))],
            &mut widget,
            (64, 64),
        );
        assert!(dragged.eyedropper_started);
        assert_eq!(dragged.eyedropper_point, Some((20, 20)));
        assert!(!dragged.eyedropper_ended);
    }

    #[test]
    fn secondary_release_reports_eyedropper_ended() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(20.0, 20.0))],
            &mut widget,
            (64, 64),
        );
        let released = run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(20.0, 20.0),
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        assert!(released.eyedropper_ended);
        assert!(!released.eyedropper_started);
    }

    #[test]
    fn secondary_click_reports_started_and_ended() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            (64, 64),
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        let released = run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(10.0, 10.0),
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut widget,
            (64, 64),
        );
        assert!(released.eyedropper_ended);
        assert!(!released.eyedropper_started);
        assert_eq!(released.eyedropper_point, None);
    }

    #[test]
    fn overlay_defaults_to_none() {
        assert_eq!(CanvasWidget::new().overlay(), CanvasOverlay::None);
        assert_eq!(CanvasWidget::default().overlay(), CanvasOverlay::None);
    }

    #[test]
    fn grid_visibility_defaults_on_and_can_be_toggled_without_pixels() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut visible_camera = Camera::new();
        visible_camera.set_zoom_percent(400);
        let (_, visible) = run_frame_with_shapes_state(
            &ctx,
            vec![],
            &mut widget,
            (32, 32),
            &mut visible_camera,
            true,
        );
        let (_, hidden) = run_frame_with_shapes_state(
            &ctx,
            vec![],
            &mut widget,
            (3, 3),
            &mut Camera::new(),
            false,
        );
        assert!(visible.len() > hidden.len());
    }

    #[test]
    fn set_overlay_round_trips() {
        let mut widget = CanvasWidget::new();
        let overlay = CanvasOverlay::Selection(Rect2i::new(1, 2, 3, 4));
        widget.set_overlay(overlay.clone());
        assert_eq!(widget.overlay(), overlay);
    }

    #[test]
    fn canvas_rect_to_screen_at_100_percent() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        run_frame(&ctx, vec![], &mut widget, (64, 64));
        assert_eq!(
            widget.canvas_rect_to_screen(Camera::new(), Rect2i::new(2, 3, 4, 5)),
            egui::Rect::from_min_size(egui::pos2(2.0, 3.0), egui::vec2(4.0, 5.0))
        );
    }

    #[test]
    fn canvas_rect_to_screen_with_pan_and_zoom() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.pan_by(5, -3);
        camera.set_zoom_percent(200);
        run_frame_with_camera(&ctx, vec![], &mut widget, (64, 64), &mut camera, true);
        let rect = widget.canvas_rect_to_screen(camera, Rect2i::new(2, 3, 4, 5));
        assert_eq!(rect.min, egui::pos2(9.0, 3.0));
        assert_eq!(rect.size(), egui::vec2(8.0, 10.0));
    }

    #[test]
    fn selection_overlay_paints_no_fill_and_dashed_ants() {
        // Given: the legacy rectangular Selection overlay on a 64×64 canvas.
        // When: the widget paints one headless frame.
        // Then: no translucent fill is emitted (D76), no solid selection stroke,
        // a grey under-line sits under the dashes, and the border is one-pixel
        // marching-ants segments from the theme token.
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let selection = Rect2i::new(2, 2, 4, 4);
        widget.set_overlay(CanvasOverlay::Selection(selection));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));
        let theme = Theme::default_dark().colors;
        let expected = widget.canvas_rect_to_screen(Camera::new(), selection);

        let tint = shapes.iter().any(|shape| {
            matches!(
                shape,
                egui::Shape::Rect(rect)
                    if rect.fill == theme.selection_fill32() && rect.rect == expected
            )
        });
        assert!(
            !tint,
            "a committed selection must not cover its pixels (D76)"
        );

        let solid_stroke = shapes.iter().any(|shape| {
            matches!(
                shape,
                egui::Shape::Rect(rect)
                    if rect.stroke.color == theme.selection_stroke32() && rect.rect == expected
            )
        });
        assert!(
            !solid_stroke,
            "the selection border must be dashed marching ants, not a solid stroke"
        );

        let ant_color = ant_stroke_color(theme.marching_ants_core(), 0);
        let ant_color = egui::Color32::from_rgba_unmultiplied(
            ant_color.r,
            ant_color.g,
            ant_color.b,
            ant_color.a,
        );
        assert!(
            ant_line_points(&shapes, ant_color).len() >= 2,
            "expected dashed ants line segments around the selection"
        );
    }

    #[test]
    fn selection_overlay_dash_geometry_advances_with_time() {
        // Given: the same Selection overlay painted on consecutive frame times.
        // When: a quarter-second of animation elapses.
        // Then: the emitted ant dash endpoints move while the fill stays put.
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        widget.set_overlay(CanvasOverlay::Selection(Rect2i::new(2, 2, 4, 4)));
        let theme = Theme::default_dark().colors;
        let ant_color = ant_stroke_color(theme.marching_ants_core(), 0);
        let ant_color = egui::Color32::from_rgba_unmultiplied(
            ant_color.r,
            ant_color.g,
            ant_color.b,
            ant_color.a,
        );

        let (_, initial_shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));
        let initial_dashes = ant_line_points(&initial_shapes, ant_color);
        for _ in 0..15 {
            run_frame(&ctx, vec![], &mut widget, (64, 64));
        }
        let (_, later_shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));
        let later_dashes = ant_line_points(&later_shapes, ant_color);

        assert!(!initial_dashes.is_empty(), "expected dashed ants segments");
        assert_ne!(
            initial_dashes, later_dashes,
            "the selection dash phase must advance with time"
        );
    }

    #[test]
    fn ants_mask_paints_pixel_aligned_theme_ants() {
        // Given: a closed mask boundary (a 4×2 rectangle, 12 document edges) on
        // a 64×64 canvas.
        // When: the AntsMask overlay paints one headless frame at 100% zoom.
        // Then: the border is a trail of sub-unit segments (axis-aligned, each
        // 1/SUBDIV screen pixel long), split into theme-tinted ON dashes and OFF
        // gaps — never a hard-coded black/white line.
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let boundary = vec![
            ((2, 2), (6, 2)),
            ((6, 2), (6, 4)),
            ((6, 4), (2, 4)),
            ((2, 4), (2, 2)),
        ];
        widget.set_overlay(CanvasOverlay::AntsMask(boundary));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));

        let theme = Theme::default_dark().colors;
        let on_color = crate::ui::theme::SKIN_TINT;
        let dark_under = theme.marching_ants_contrast_dark32();

        let on_lines = ant_line_points(&shapes, on_color);
        let under_lines = ant_line_points(&shapes, dark_under);
        assert!(!on_lines.is_empty(), "expected bright ON dashes");
        assert_eq!(
            under_lines.len(),
            12 * PIXEL_ANT_SUBDIV as usize,
            "all 12 document edges × SUBDIV sub-edges have a dark backing stroke"
        );

        let all: Vec<[egui::Pos2; 2]> = on_lines;
        let sub_px = 1.0 / PIXEL_ANT_SUBDIV as f32;
        for [a, b] in &all {
            let dx = (b.x - a.x).abs();
            let dy = (b.y - a.y).abs();
            assert!(dx < 1e-3 || dy < 1e-3, "edge {a:?}→{b:?} is not axis-aligned");
            assert!(
                (dx.max(dy) - sub_px).abs() < 1e-3,
                "edge {a:?}→{b:?} is not exactly one SUB-UNIT long"
            );
        }
        assert_eq!(
            all.len(),
            6 * PIXEL_ANT_SUBDIV as usize,
            "half of the 12 document edges × SUBDIV sub-edges are ON"
        );
    }

    #[test]
    fn ants_mask_uses_theme_tokens_without_a_canvas_sample() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let boundary = vec![
            ((2, 2), (6, 2)),
            ((6, 2), (6, 4)),
            ((6, 4), (2, 4)),
            ((2, 4), (2, 2)),
        ];
        widget.set_overlay(CanvasOverlay::AntsMask(boundary));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));

        let theme = Theme::default_dark().colors;
        assert!(
            !ant_line_points(&shapes, theme.marching_ants_contrast_dark32()).is_empty(),
            "the theme dark contrast backing must appear on the mask border"
        );
        assert!(
            !ant_line_points(&shapes, crate::ui::theme::SKIN_TINT).is_empty(),
            "the bright ON dashes must appear over the dark backing"
        );
    }

    #[test]
    fn ants_mask_stays_theme_tinted_with_a_canvas_sample() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let boundary = vec![
            ((2, 2), (6, 2)),
            ((6, 2), (6, 4)),
            ((6, 4), (2, 4)),
            ((2, 4), (2, 2)),
        ];
        widget.set_overlay(CanvasOverlay::AntsMask(boundary));
        let theme = Theme::default_dark().colors;
        let dark_under = theme.marching_ants_contrast_dark32();
        let bright_ant = crate::ui::theme::SKIN_TINT;
        for bg in [
            Color::rgba(255, 255, 255, 255),
            Color::rgba(128, 128, 128, 255),
        ] {
            let view = CanvasView {
                canvas_color_at: Some(bg),
                ..Default::default()
            };
            let (_, shapes) = run_frame_with_view(&ctx, vec![], &mut widget, view, Camera::new());
            assert!(
                !ant_line_points(&shapes, dark_under).is_empty()
                    && !ant_line_points(&shapes, bright_ant).is_empty(),
                "mask ants must keep high-contrast backing + bright dashes over {bg:?}"
            );
        }
    }

    #[test]
    fn ants_rect_paints_a_grey_under_stroke() {
        // Given: a rectangular Ants overlay on a 64×64 canvas.
        // When: the overlay paints one headless frame.
        // Then: the full rectangle perimeter is laid down as solid grey 1 px
        // under-lines, with the dashed white ants painted on top.
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let rect = Rect2i::new(2, 2, 4, 4);
        widget.set_overlay(CanvasOverlay::Ants(rect));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));
        let theme = Theme::default_dark().colors;
        let under = ant_under_stroke_color(theme.marching_ants_under_core());
        let under_color = egui::Color32::from_rgba_unmultiplied(under.r, under.g, under.b, under.a);
        let ant = ant_stroke_color(theme.marching_ants_core(), 0);
        let ant_color = egui::Color32::from_rgba_unmultiplied(ant.r, ant.g, ant.b, ant.a);

        let screen = widget.canvas_rect_to_screen(Camera::new(), rect);
        let top_right = egui::pos2(screen.max.x, screen.min.y);
        let bottom_left = egui::pos2(screen.min.x, screen.max.y);
        let expected_edges = [
            [screen.min, top_right],
            [top_right, screen.max],
            [screen.max, bottom_left],
            [bottom_left, screen.min],
        ];
        let under_lines = ant_line_points(&shapes, under_color);
        for edge in expected_edges {
            assert!(
                under_lines.contains(&edge),
                "expected a solid grey under-line on the rect edge {edge:?}"
            );
        }

        assert!(
            !ant_line_points(&shapes, ant_color).is_empty(),
            "expected the dashed white ants over the grey under-stroke"
        );

        let is_color = |shape: &egui::Shape, color: egui::Color32| matches!(shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == color);
        let first_under = shapes.iter().position(|s| is_color(s, under_color));
        let first_dash = shapes.iter().position(|s| is_color(s, ant_color));
        assert!(
            first_under < first_dash,
            "the grey under-stroke must be painted beneath the dashes"
        );
    }

    #[test]
    fn frame_regions_paint_pixel_ants_in_theme_tokens() {
        // Given: the FrameRegions overlay for one 4×4 region.
        // Then: the region border is a bordeaux under-baseline with live-yellow
        // pixel-perfect (sub-unit) dashes on top, all from the tokens; an empty
        // region list paints nothing.
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let region = Rect2i::new(2, 2, 4, 4);
        widget.set_overlay(CanvasOverlay::FrameRegions(vec![region]));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));

        let theme = Theme::default_dark().colors;
        let under = ant_under_stroke_color(theme.frame_region_ant_under_core());
        let under_color =
            egui::Color32::from_rgba_unmultiplied(under.r, under.g, under.b, under.a);
        // The dashes: the token's RGB at the higher FRAME_REGION alpha (the
        // same construction the paint path uses), NOT `ant_stroke_color`.
        let base = theme.frame_region_ant_core();
        let a = (u32::from(base.a) * u32::from(FRAME_REGION_ANT_STROKE_ALPHA) / 255) as u8;
        let ant_color = egui::Color32::from_rgba_unmultiplied(base.r, base.g, base.b, a);
        assert_eq!(
            ant_color.a(),
            FRAME_REGION_ANT_STROKE_ALPHA,
            "the frame-region dashes must use the higher FRAME_REGION alpha"
        );
        assert_ne!(
            under_color, ant_color,
            "the two frame-region tokens must be distinct tones"
        );

        // The full region perimeter is laid down as the solid under-baseline:
        // `mask_polylines` chains the boundary into doc-px unit segments, so a
        // 4×4 region is covered by 16 contiguous bordeaux under-segments.
        let origin = widget.canvas_rect_to_screen(Camera::new(), Rect2i::new(0, 0, 1, 1)).min;
        let to_screen = |x: f32, y: f32| egui::pos2(origin.x + x, origin.y + y);
        let under_lines = ant_line_points(&shapes, under_color);
        assert_eq!(
            under_lines.len(),
            16,
            "a 4×4 region has 16 boundary unit edges → 16 under segments"
        );
        for seg in frame_region_boundary_segments(&[region]) {
            let a = to_screen(seg.0 .0 as f32, seg.0 .1 as f32);
            let b = to_screen(seg.1 .0 as f32, seg.1 .1 as f32);
            assert!(
                under_lines.contains(&[a, b]) || under_lines.contains(&[b, a]),
                "missing bordeaux under-segment for boundary unit edge {seg:?}"
            );
        }

        // Pixel-perfect dashes from the live-yellow token: axis-aligned and
        // exactly one SUB-UNIT long each (locked to the document grid).
        let dashes = ant_line_points(&shapes, ant_color);
        assert!(!dashes.is_empty(), "expected live-yellow frame-region dashes");
        let sub_px = 1.0 / PIXEL_ANT_SUBDIV as f32;
        for [a, b] in &dashes {
            let dx = (b.x - a.x).abs();
            let dy = (b.y - a.y).abs();
            assert!(dx < 1e-3 || dy < 1e-3, "dash {a:?}→{b:?} is not axis-aligned");
            assert!(
                (dx.max(dy) - sub_px).abs() < 1e-3,
                "dash {a:?}→{b:?} is not exactly one SUB-UNIT long"
            );
        }

        // The under-baseline is painted beneath the dashes.
        let is_color = |shape: &egui::Shape, color: egui::Color32| {
            matches!(shape, egui::Shape::LineSegment { stroke, .. } if stroke.color == color)
        };
        let first_under = shapes.iter().position(|s| is_color(s, under_color));
        let first_dash = shapes.iter().position(|s| is_color(s, ant_color));
        assert!(
            first_under < first_dash,
            "the bordeaux under-stroke must be painted beneath the dashes"
        );

        // An empty region list paints nothing at all.
        widget.set_overlay(CanvasOverlay::FrameRegions(vec![]));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));
        assert!(
            ant_line_points(&shapes, ant_color).is_empty(),
            "an empty region list must paint no dashes"
        );
        assert!(
            ant_line_points(&shapes, under_color).is_empty(),
            "an empty region list must paint no under-baseline"
        );
    }

    #[test]
    fn frame_regions_union_boundary_skips_inner_edges() {
        // Given: TWO ADJACENT 4×4 frames sharing the vertical edge at x=6
        // (right(2,2,4,4) == x(6,2,4,4)).
        // Then: the union boundary is exactly the outer 8×4 rect — the shared
        // INNER edge at x=6 is produced by NEITHER the under-baseline NOR the
        // dashes.
        let regions = [Rect2i::new(2, 2, 4, 4), Rect2i::new(6, 2, 4, 4)];
        let union = Rect2i::new(2, 2, 8, 4);

        // Pure geometry first: the boundary collapses to the union rect's
        // outline (same trick as `Selection::outline_segments`).
        let segs = frame_region_boundary_segments(&regions);
        assert_eq!(
            segs,
            frame_region_boundary_segments(&[union]),
            "adjacent frames must collapse to the union's outer boundary"
        );
        assert_eq!(segs.len(), 24, "2*(w+h) = 2*(8+4) unit edges");
        assert!(
            segs.iter().all(|((x0, _), (x1, _))| !(*x0 == 6 && *x1 == 6)),
            "no vertical segment may sit on the shared inner edge x=6"
        );

        // Painted: the outer union frame is drawn, the inner line is not.
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        widget.set_overlay(CanvasOverlay::FrameRegions(regions.to_vec()));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));

        let theme = Theme::default_dark().colors;
        let under = ant_under_stroke_color(theme.frame_region_ant_under_core());
        let under_color =
            egui::Color32::from_rgba_unmultiplied(under.r, under.g, under.b, under.a);
        let base = theme.frame_region_ant_core();
        let a = (u32::from(base.a) * u32::from(FRAME_REGION_ANT_STROKE_ALPHA) / 255) as u8;
        let ant_color = egui::Color32::from_rgba_unmultiplied(base.r, base.g, base.b, a);
        let under_lines = ant_line_points(&shapes, under_color);
        let dashes = ant_line_points(&shapes, ant_color);
        let origin = widget.canvas_rect_to_screen(Camera::new(), Rect2i::new(0, 0, 1, 1)).min;

        // The union's full perimeter is laid down as the bordeaux under-line:
        // 2*(8+4) = 24 contiguous unit segments.
        let to_screen = |x: f32, y: f32| egui::pos2(origin.x + x, origin.y + y);
        assert_eq!(
            under_lines.len(),
            24,
            "the 8×4 union has 2*(8+4)=24 boundary unit edges"
        );
        for seg in frame_region_boundary_segments(&[union]) {
            let a = to_screen(seg.0 .0 as f32, seg.0 .1 as f32);
            let b = to_screen(seg.1 .0 as f32, seg.1 .1 as f32);
            assert!(
                under_lines.contains(&[a, b]) || under_lines.contains(&[b, a]),
                "missing bordeaux under-segment for union boundary unit edge {seg:?}"
            );
        }
        assert!(!dashes.is_empty(), "the outer union frame must show dashes");

        // No painted segment sits on the shared inner edge (doc x=6, y in
        // [2,6)): scale is 1 and pan (0,0), so origin maps doc point (x,y).
        let inner_x = origin.x + 6.0;
        let on_inner_line = |lines: &[[egui::Pos2; 2]]| {
            lines.iter().any(|[a, b]| {
                (a.x - inner_x).abs() < 1e-3
                    && (b.x - inner_x).abs() < 1e-3
                    && a.y >= origin.y + 2.0 - 1e-3
                    && a.y <= origin.y + 6.0 + 1e-3
            })
        };
        assert!(
            !on_inner_line(&dashes),
            "no dash may be painted on the shared inner edge x=6"
        );
        assert!(
            !on_inner_line(&under_lines),
            "no bordeaux under-line may be painted on the shared inner edge x=6"
        );
    }

    #[test]
    fn frame_regions_disjoint_frames_stay_separate_boundaries() {
        // Given: two DISJOINT 4×4 frames with a 2-px gap (doc x in [6, 8)).
        // Then: the union boundary has TWO separate closed loops — painting
        // never connects the two frames with a line across the gap.
        let regions = [Rect2i::new(2, 2, 4, 4), Rect2i::new(8, 2, 4, 4)];
        let segs = frame_region_boundary_segments(&regions);
        assert_eq!(segs.len(), 32, "two perimeters of 16 unit edges");
        let loops = mask_polylines(&segs);
        assert_eq!(loops.len(), 2, "two disjoint frames yield two separate loops");
        for lp in &loops {
            assert_eq!(
                lp.len(),
                16,
                "each loop walks every doc-pixel corner along a 16-edge perimeter"
            );
        }

        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        widget.set_overlay(CanvasOverlay::FrameRegions(regions.to_vec()));
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));

        let theme = Theme::default_dark().colors;
        let under = ant_under_stroke_color(theme.frame_region_ant_under_core());
        let under_color =
            egui::Color32::from_rgba_unmultiplied(under.r, under.g, under.b, under.a);
        let base = theme.frame_region_ant_core();
        let a = (u32::from(base.a) * u32::from(FRAME_REGION_ANT_STROKE_ALPHA) / 255) as u8;
        let ant_color = egui::Color32::from_rgba_unmultiplied(base.r, base.g, base.b, a);
        let under_lines = ant_line_points(&shapes, under_color);
        let dashes = ant_line_points(&shapes, ant_color);
        assert_eq!(
            under_lines.len(),
            32,
            "two perimeters × 16 unit edges of bordeaux under-line"
        );
        assert_eq!(
            dashes.len(),
            (16 * PIXEL_ANT_SUBDIV as usize / 2) * 2,
            "each 16-edge perimeter contributes half its sub-edges as dashes"
        );

        // Every unit edge of both outer perimeters is painted (undirected).
        let origin = widget.canvas_rect_to_screen(Camera::new(), Rect2i::new(0, 0, 1, 1)).min;
        let to_screen = |x: f32, y: f32| egui::pos2(origin.x + x, origin.y + y);
        for r in regions {
            for seg in frame_region_boundary_segments(std::slice::from_ref(&r)) {
                let a = to_screen(seg.0 .0 as f32, seg.0 .1 as f32);
                let b = to_screen(seg.1 .0 as f32, seg.1 .1 as f32);
                assert!(
                    (under_lines.contains(&[a, b]) || under_lines.contains(&[b, a])),
                    "missing bordeaux under-segment for frame {r:?} unit edge {seg:?}"
                );
            }
        }

        // Nothing is painted across (or inside) the open gap band (6, 8).
        let (gap_min, gap_max) = (origin.x + 6.0, origin.x + 8.0);
        let crosses_or_fills_gap = |lines: &[[egui::Pos2; 2]]| {
            lines.iter().any(|[a, b]| {
                let lo = a.x.min(b.x);
                let hi = a.x.max(b.x);
                lo < gap_max - 1e-3 && hi > gap_min + 1e-3
            })
        };
        assert!(
            !crosses_or_fills_gap(&dashes) && !crosses_or_fills_gap(&under_lines),
            "nothing may be painted across the gap between the two frames"
        );
    }

    #[test]
    fn frame_regions_march_at_half_the_global_ant_speed() {
        // Given: one 4×4 frame region painted at t = 0.5 s (500 ms of wall
        // clock).
        // Then: the painted dash pattern is EXACTLY the one
        // `pixel_ant_edges_from_polylines` yields at t = 250 ms
        // (`time_ms * FRAME_REGION_ANT_SPEED_SCALE` = 500 * 0.5), and differs
        // from the full-speed 500 ms pattern — the halved phase input matters.
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let region = Rect2i::new(2, 2, 4, 4);
        widget.set_overlay(CanvasOverlay::FrameRegions(vec![region]));
        let (_, shapes) = run_frame_with_shapes_time(&ctx, vec![], &mut widget, (64, 64), 0.5);

        let theme = Theme::default_dark().colors;
        let base = theme.frame_region_ant_core();
        let a = (u32::from(base.a) * u32::from(FRAME_REGION_ANT_STROKE_ALPHA) / 255) as u8;
        let ant_color = egui::Color32::from_rgba_unmultiplied(base.r, base.g, base.b, a);
        let painted = ant_line_points(&shapes, ant_color);
        assert!(!painted.is_empty(), "expected live-yellow dashes");

        let loops = mask_polylines(&frame_region_boundary_segments(&[region]));
        let half_edges: std::collections::BTreeSet<((i32, i32), (i32, i32))> =
            pixel_ant_edges_from_polylines(&loops, 250)
                .into_iter()
                .filter(|e| e.on)
                .map(|e| (e.start, e.end))
                .collect();
        let full_edges: std::collections::BTreeSet<((i32, i32), (i32, i32))> =
            pixel_ant_edges_from_polylines(&loops, 500)
                .into_iter()
                .filter(|e| e.on)
                .map(|e| (e.start, e.end))
                .collect();
        assert_ne!(
            half_edges, full_edges,
            "half-speed (250 ms) and full-speed (500 ms) phases must differ"
        );

        // Recover each painted dash's SUB-UNIT coordinates by inverting the
        // paint path's mapping (origin + pan + su/SUBDIV * scale; pan (0,0),
        // scale 1), then compare with the integer half-speed edge set.
        let origin = widget.canvas_rect_to_screen(Camera::new(), Rect2i::new(0, 0, 1, 1)).min;
        let subdiv = PIXEL_ANT_SUBDIV as f32;
        let actual: std::collections::BTreeSet<((i32, i32), (i32, i32))> = painted
            .iter()
            .map(|[a, b]| {
                (
                    (
                        ((a.x - origin.x) * subdiv).round() as i32,
                        ((a.y - origin.y) * subdiv).round() as i32,
                    ),
                    (
                        ((b.x - origin.x) * subdiv).round() as i32,
                        ((b.y - origin.y) * subdiv).round() as i32,
                    ),
                )
            })
            .collect();
        assert_eq!(
            actual, half_edges,
            "the painted dashes must be exactly the HALF-speed (250 ms) pattern"
        );
    }

    #[test]
    fn live_marquee_uses_the_inverted_probe_color() {
        // Given: a live Marquee overlay and an App-sampled probe pixel of
        // (10, 20, 30) under the pointer.
        // When: the marquee paints over one headless frame.
        // Then: its plain solid stroke is the per-channel inverse
        // (245, 235, 225, 255); an off-canvas probe (None) falls back to the
        // theme's live-outline stroke. No fill, no dashes either way (D73/D76).
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let rect = Rect2i::new(2, 2, 4, 4);
        widget.set_overlay(CanvasOverlay::Marquee(rect));
        let mut view = zoomed_view(None, DrawMode::Pen, Color::BLACK, None, None);
        view.marquee_probe = Some(Color::rgba(10, 20, 30, 255));
        let (_, shapes) =
            run_frame_with_view(&ctx, vec![], &mut widget, view.clone(), Camera::new());
        let screen = widget.canvas_rect_to_screen(Camera::new(), rect);
        let inverted = egui::Color32::from_rgb(245, 235, 225);
        let stroke = shapes
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Rect(r) if r.rect == screen && r.stroke.color == inverted => Some(r),
                _ => None,
            })
            .expect("the live marquee must stroke with the inverted probe color");
        assert_eq!(stroke.stroke.width, 1.0, "the live box stroke is 1 pt");
        assert_eq!(
            stroke.fill,
            egui::Color32::TRANSPARENT,
            "the live box must never be filled"
        );
        assert_eq!(
            stroke.stroke_kind,
            egui::StrokeKind::Middle,
            "the live box must stay a plain solid outline"
        );

        view.marquee_probe = None;
        let (_, fallback_shapes) =
            run_frame_with_view(&ctx, vec![], &mut widget, view, Camera::new());
        let fallback = Theme::default_dark().colors.marching_ants32();
        assert!(
            fallback_shapes.iter().any(|shape| matches!(
                shape,
                egui::Shape::Rect(r) if r.rect == screen && r.stroke.color == fallback
            )),
            "an off-canvas pointer must fall back to the theme live-outline stroke"
        );
    }

    #[test]
    fn no_overlay_paints_nothing() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let (_, shapes) = run_frame_with_shapes(&ctx, vec![], &mut widget, (64, 64));
        assert!(!shapes.iter().any(|s| {
            matches!(
                s,
                egui::Shape::Rect(r) if r.stroke.color == Theme::default_dark().colors.selection_stroke32()
            )
        }));
        assert!(!shapes.iter().any(|s| {
            matches!(
                s,
                egui::Shape::Rect(r) if r.fill == Theme::default_dark().colors.selection_fill32()
            )
        }));
    }

    #[test]
    /// Given a lasso trace viewed through a panned, zoomed camera, when the
    /// overlay paints, then exactly one closed, unfilled polyline appears at
    /// the marquee-transformed trace points, stroked 1 pt in the live-outline
    /// token.
    fn lasso_overlay_paints_a_closed_polyline() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let trace: Vec<(i32, i32)> = vec![(2, 2), (6, 2), (6, 6)];
        widget.set_overlay(CanvasOverlay::Lasso(trace.clone()));
        let mut camera = Camera::new();
        camera.set_zoom_percent(200);
        camera.pan_by(5, -3);
        let view = zoomed_view(None, DrawMode::Pen, Color::BLACK, None, None);
        let (_, shapes) = run_frame_with_view(&ctx, Vec::new(), &mut widget, view, camera);

        let expected_color = Theme::default_dark().colors.marching_ants32();
        let expected_points: Vec<egui::Pos2> = trace
            .iter()
            .map(|&(x, y)| egui::pos2(5.0 + x as f32 * 2.0, -3.0 + y as f32 * 2.0))
            .collect();
        let paths: Vec<&egui::epaint::PathShape> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Path(path)
                    if matches!(
                        &path.stroke.color,
                        egui::epaint::ColorMode::Solid(color) if *color == expected_color
                    ) =>
                {
                    Some(path)
                }
                _ => None,
            })
            .collect();

        assert_eq!(paths.len(), 1, "the lasso must paint exactly one polyline");
        assert!(paths[0].closed, "the lasso polyline must be closed");
        assert_eq!(
            paths[0].fill,
            egui::Color32::TRANSPARENT,
            "the lasso must not fill its trace"
        );
        assert_eq!(
            paths[0].points, expected_points,
            "the lasso must use the marquee's canvas→screen math"
        );
        assert_eq!(
            paths[0].stroke.width, 1.0,
            "the lasso stroke must be one point wide"
        );
    }

    #[test]
    /// Given three region cells, when the overlay paints at 100 % zoom, then
    /// each cell is its own plain solid, unfilled, 1 pt unit outline in the
    /// live-outline token at the cell's screen rect.
    fn region_cells_overlay_paints_each_cell_outline() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let cells = vec![
            Rect2i::new(0, 0, 16, 16),
            Rect2i::new(16, 0, 16, 16),
            Rect2i::new(0, 16, 16, 16),
        ];
        widget.set_overlay(CanvasOverlay::RegionCells {
            cells: cells.clone(),
            base: Vec::new(),
        });
        let view = zoomed_view(None, DrawMode::Pen, Color::BLACK, None, None);
        let (_, shapes) = run_frame_with_view(&ctx, Vec::new(), &mut widget, view, Camera::new());

        let expected_color = Theme::default_dark().colors.marching_ants32();
        let outlines: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.color == expected_color
                        && rect.stroke.width == 1.0
                        && rect.fill == egui::Color32::TRANSPARENT
                        && rect.stroke_kind == egui::StrokeKind::Middle =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        let expected: Vec<egui::Rect> = cells
            .iter()
            .map(|cell| {
                egui::Rect::from_min_size(
                    egui::pos2(cell.x as f32, cell.y as f32),
                    egui::vec2(cell.w as f32, cell.h as f32),
                )
            })
            .collect();

        assert_eq!(
            outlines, expected,
            "every region cell must paint its own unit outline"
        );
    }

    #[test]
    /// Given a live region-cells overlay carrying a base selection boundary
    /// and a swept cell, when the overlay paints at 100 % zoom, then the base
    /// keeps its committed pixel-perfect marching ants (theme-tinted ON dashes
    /// and OFF gaps) and both the ants and the swept cell's plain solid outline
    /// are painted, with the ants beneath the cell outlines.
    fn region_cells_overlay_paints_base_ants_under_the_cells() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let base = vec![
            ((2, 2), (6, 2)),
            ((6, 2), (6, 4)),
            ((6, 4), (2, 4)),
            ((2, 4), (2, 2)),
        ];
        let cells = vec![Rect2i::new(8, 8, 4, 4)];
        widget.set_overlay(CanvasOverlay::RegionCells {
            cells: cells.clone(),
            base: base.clone(),
        });
        let view = zoomed_view(None, DrawMode::Pen, Color::BLACK, None, None);
        let (_, shapes) = run_frame_with_view(&ctx, Vec::new(), &mut widget, view, Camera::new());

        let theme = Theme::default_dark().colors;
        let on_color = crate::ui::theme::SKIN_TINT;
        let dark_under = theme.marching_ants_contrast_dark32();
        let cell_color = theme.marching_ants32();

        // The base keeps its committed pixel-perfect ants: dark backing plus
        // bright ON sub-unit dashes.
        let on_lines = ant_line_points(&shapes, on_color);
        assert!(!on_lines.is_empty(), "expected the base's bright ON dashes");
        assert!(
            !ant_line_points(&shapes, dark_under).is_empty(),
            "expected the base's dark contrast backing"
        );
        assert_eq!(
            on_lines.len(),
            6 * PIXEL_ANT_SUBDIV as usize,
            "half of the 12 document edges × SUBDIV sub-edges are ON"
        );

        // The swept cell is still a plain solid unit outline (no fill, no
        // dashes), in the live-gesture stroke.
        let cell_outlines: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.color == cell_color
                        && rect.stroke.width == 1.0
                        && rect.fill == egui::Color32::TRANSPARENT
                        && rect.stroke_kind == egui::StrokeKind::Middle =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        let expected: Vec<egui::Rect> = cells
            .iter()
            .map(|cell| {
                egui::Rect::from_min_size(
                    egui::pos2(cell.x as f32, cell.y as f32),
                    egui::vec2(cell.w as f32, cell.h as f32),
                )
            })
            .collect();
        assert_eq!(
            cell_outlines, expected,
            "the swept cell must paint its plain solid unit outline"
        );

        // Paint order: the base ants are laid down before the cell outlines.
        let is_ant_line = |shape: &egui::Shape| {
            matches!(
                shape,
                egui::Shape::LineSegment { stroke, .. }
                    if stroke.color == on_color || stroke.color == dark_under
            )
        };
        let first_base = shapes.iter().position(is_ant_line);
        let first_cell = shapes.iter().position(|shape| {
            matches!(
                shape,
                egui::Shape::Rect(rect)
                    if rect.stroke.color == cell_color
                        && rect.fill == egui::Color32::TRANSPARENT
                        && rect.stroke_kind == egui::StrokeKind::Middle
            )
        });
        assert!(
            first_base.is_some() && first_cell.is_some(),
            "expected both the base ants and the cell outlines to paint"
        );
        assert!(
            first_base < first_cell,
            "the base ants must be painted beneath the swept cell outlines"
        );
    }

    #[test]
    fn overlay_survives_frame() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let overlay = CanvasOverlay::Selection(Rect2i::new(2, 2, 4, 4));
        widget.set_overlay(overlay.clone());
        run_frame(&ctx, vec![], &mut widget, (64, 64));
        assert_eq!(widget.overlay(), overlay);
    }

    #[test]
    fn ants_do_not_brightness_blink() {
        // Given: the marching-ants theme token at widely separated frame times.
        // When: the rendered color and phase are sampled.
        // Then: brightness stays low and invariant while phase advances at
        // the fixed `ANT_SPEED_PX_PER_MS`.
        let theme = Theme::default_dark();
        let base = theme.colors.marching_ants_core();
        let early = ant_stroke_color(base, 0);
        let later = ant_stroke_color(base, 120_000);
        assert_eq!(early, later, "the ants color must not vary over frames");
        assert!(
            early.a < 200,
            "the ants border must stay low contrast; got alpha {}",
            early.a
        );
        assert!(early.a > 0, "the ants border must remain visible");

        let cycle = ANT_DASH_PX + ANT_GAP_PX;
        assert!((ant_phase(1_000) - ANT_SPEED_PX_PER_MS * 1_000.0).abs() < 1e-3);
        // Round: the float division lands just under the exact period boundary.
        let full_turn_ms = (cycle / ANT_SPEED_PX_PER_MS).round() as u64;
        assert!((ant_phase(full_turn_ms) - 0.0).abs() < 1e-3);
        assert!(cycle > 0.0);
    }

    #[test]
    /// Given a 2×2 pen cursor and a committed selection covering only the
    /// bottom-left footprint cell, when the cursor preview paints, then only
    /// that cell is shown.
    fn brush_cursor_preview_is_clipped_to_the_selection_mask() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let scale = 4.0f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        // Pointer at screen (10,10) → even 2×2 brush anchors at canvas (3,3),
        // whose footprint is (2,2)…(3,3).
        let spec =
            crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square);
        let view = zoomed_view(
            Some(spec),
            DrawMode::Pen,
            primary,
            None,
            Some(rect_clip(Rect2i::new(2, 3, 1, 1))),
        );
        let (_, shapes) = run_frame_with_view(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            view,
            camera,
        );

        assert_eq!(
            preview_cells(&shapes, scale, primary32),
            vec![(2, 3)],
            "the preview must show only the cells inside the selection"
        );
    }

    #[test]
    /// Given a 2×2 eraser cursor and a selection covering one of its footprint
    /// cells, when the hollow silhouette paints, then it outlines only that
    /// cell (4 edges) instead of the whole 2×2 block (8 edges).
    fn eraser_cursor_preview_is_clipped_to_the_selection_mask() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let spec =
            crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square);
        let view = zoomed_view(
            Some(spec),
            DrawMode::Eraser,
            Color::BLACK,
            None,
            Some(rect_clip(Rect2i::new(2, 3, 1, 1))),
        );
        let (_, shapes) = run_frame_with_view(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
            &mut widget,
            view,
            camera,
        );
        let hollow = shapes
            .iter()
            .filter(|shape| {
                matches!(shape, egui::Shape::Rect(rect)
                    if rect.fill == Theme::default_dark().colors.selection_stroke32())
            })
            .count();
        assert_eq!(
            hollow, 4,
            "the clipped 1 px eraser silhouette has 4 edges, not the full 8"
        );
    }

    #[test]
    /// Given a LINE preview whose segment leaves a narrow selection, when the
    /// preview paints, then only the in-selection cells are shown.
    fn line_preview_is_clipped_to_the_selection_mask() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let camera = Camera::new();
        let scale = camera.scale() as f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let view = zoomed_view(
            Some(crate::core::brush::BrushSpec::new(
                1,
                crate::core::brush::BrushShape::Square,
            )),
            DrawMode::Pen,
            primary,
            Some(LinePreview {
                anchor: (2, 10),
                end: (20, 10),
            }),
            Some(rect_clip(Rect2i::new(6, 8, 4, 3))),
        );
        let (_, shapes) = run_frame_with_view(&ctx, Vec::new(), &mut widget, view, camera);

        assert_eq!(
            preview_cells(&shapes, scale, primary32),
            vec![(6, 10), (7, 10), (8, 10), (9, 10)],
            "the line preview must preview only the cells the commit will land"
        );
    }

    #[test]
    /// Given a curve-transform footprint and a masked selection, when the
    /// preview paints, then only cells both the footprint and the mask contain
    /// are shown.
    fn curve_footprint_preview_is_clipped_to_the_selection_mask() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let scale = 4.0f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let spec =
            crate::core::brush::BrushSpec::sanitize(1, crate::core::brush::BrushShape::Square);
        // A 1×1 brush makes the footprint exactly the samples (0,8)…(8,8), and
        // the mask keeps only the even-x cells in x 0..=8.
        let mask: Vec<bool> = (0..9).map(|x| x % 2 == 0).collect();
        widget.set_overlay(CanvasOverlay::Curve {
            points: vec![(0.0, 8.0), (8.0, 8.0)],
            samples: (0..=8).map(|x| (x, 8)).collect(),
            gizmos: [(1.0, 8.0), (3.0, 8.0), (5.0, 8.0), (7.0, 8.0)],
            hovered: None,
        });
        let view = zoomed_view(
            Some(spec),
            DrawMode::Pen,
            primary,
            None,
            Some(mask_clip(Rect2i::new(0, 8, 9, 1), mask)),
        );
        let (_, shapes) = run_frame_with_view(&ctx, Vec::new(), &mut widget, view, camera);

        assert_eq!(
            preview_cells(&shapes, scale, primary32),
            vec![(0, 8), (2, 8), (4, 8), (6, 8), (8, 8)],
            "the curve preview must hide the cells the selection mask rejects"
        );
    }

    #[test]
    /// Given no selection, when each preview paints, then the brush cursor
    /// stays window-clipped (unbounded), the pen LINE preview clips to the
    /// canvas, and the eraser LINE preview stays window-clipped.
    fn previews_without_a_selection_keep_their_existing_clips() {
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let scale = 4.0f32;
        let primary = Color::rgb(200, 30, 40);
        let primary32 = egui::Color32::from_rgba_unmultiplied(200, 30, 40, 255);
        let spec =
            crate::core::brush::BrushSpec::sanitize(2, crate::core::brush::BrushShape::Square);
        let one = crate::core::brush::BrushSpec::new(1, crate::core::brush::BrushShape::Square);

        // A fresh context per case: an egui context remembers the pointer
        // across frames, so a shared one would let each case's hover paint a
        // brush cursor into the next case's shapes.
        let mut widget = CanvasWidget::new();
        let ctx = egui::Context::default();
        let view = zoomed_view(Some(spec), DrawMode::Pen, primary, None, None);
        let (_, shapes) = run_frame_with_view(
            &ctx,
            vec![egui::Event::PointerMoved(egui::pos2(2.0, 10.0))],
            &mut widget,
            view,
            camera,
        );
        assert_eq!(
            preview_cells(&shapes, scale, primary32),
            painted_pixels(spec, (1, 3)),
            "without a selection the cursor preview keeps every footprint cell"
        );

        // The pen LINE preview clips to the canvas rect: a segment running past
        // the right edge stops at the last canvas column.
        let mut widget = CanvasWidget::new();
        let ctx = egui::Context::default();
        let view = zoomed_view(
            Some(one),
            DrawMode::Pen,
            primary,
            Some(LinePreview {
                anchor: (30, 10),
                end: (40, 10),
            }),
            None,
        );
        let (_, shapes) = run_frame_with_view(&ctx, Vec::new(), &mut widget, view, camera);
        assert_eq!(
            preview_cells(&shapes, scale, primary32),
            vec![(30, 10), (31, 10)],
            "the pen line preview must stay clipped to the canvas"
        );

        // The eraser LINE preview is window-clipped instead: the same segment
        // shows the full silhouette, canvas cells and beyond.
        let mut widget = CanvasWidget::new();
        let ctx = egui::Context::default();
        let view = zoomed_view(
            Some(one),
            DrawMode::Eraser,
            primary,
            Some(LinePreview {
                anchor: (30, 10),
                end: (40, 10),
            }),
            None,
        );
        let (_, shapes) = run_frame_with_view(&ctx, Vec::new(), &mut widget, view, camera);
        let hollow = shapes
            .iter()
            .filter(|shape| {
                matches!(shape, egui::Shape::Rect(rect)
                    if rect.fill == Theme::default_dark().colors.selection_stroke32())
            })
            .count();
        assert_eq!(
            hollow,
            brush_outline_segments(&(30..=40).map(|x| (x, 10)).collect::<Vec<_>>()).len(),
            "the eraser line preview must stay window-clipped, not canvas-clipped"
        );
    }

    /// A view for the drag-path tests: a 32×32 canvas with no brush, and the
    /// selection drag gesture flag set.
    fn drag_view(drag_active: bool) -> CanvasView {
        CanvasView {
            canvas_size: (32, 32),
            grid_visible: false,
            selection_drag_active: drag_active,
            ..Default::default()
        }
    }

    #[test]
    /// Given a live Select drag, when the pointer jumps far outside the canvas
    /// draw rect, then the drag still reports a point, clamped to the canvas
    /// edge, instead of freezing at the last in-canvas position.
    fn select_drag_uses_the_live_clamped_interact_position() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        let view = drag_view(true);

        let step = |ctx: &egui::Context,
                    widget: &mut CanvasWidget,
                    camera: &mut Camera,
                    events: Vec<egui::Event>| {
            let (result, _) = run_frame_with_view(ctx, events, widget, view.clone(), *camera);
            *camera = result.updated_camera;
            result
        };
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))],
        );
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![primary_press(egui::pos2(5.0, 5.0))],
        );
        let outside = step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(1200.0, 900.0))],
        );

        assert!(outside.stroke_started, "the far jump still starts the drag");
        assert_eq!(
            outside.stroke_point,
            Some((31, 31)),
            "a pointer outside the window must clamp to the canvas edge, not report nothing"
        );
    }

    #[test]
    /// Given a live Select drag whose pointer has already been clamped, when
    /// the interact position is gone (the pointer left the window), then the
    /// drag keeps reporting the last clamped point instead of dropping to None.
    fn select_drag_holds_the_last_point_when_the_interact_position_is_missing() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        let view = drag_view(true);

        let step = |ctx: &egui::Context,
                    widget: &mut CanvasWidget,
                    camera: &mut Camera,
                    events: Vec<egui::Event>| {
            let (result, _) = run_frame_with_view(ctx, events, widget, view.clone(), *camera);
            *camera = result.updated_camera;
            result
        };
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(5.0, 5.0))],
        );
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![primary_press(egui::pos2(5.0, 5.0))],
        );
        let clamped = step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(1200.0, 900.0))],
        );
        assert_eq!(clamped.stroke_point, Some((31, 31)));
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerGone],
        );
        let gone = step(&ctx, &mut widget, &mut camera, Vec::new());

        assert_eq!(
            gone.stroke_point,
            Some((31, 31)),
            "without an interact position the drag must hold its last clamped point"
        );
    }

    #[test]
    /// Given a live selection drag on a panned, zoomed canvas, when the pointer
    /// continues past the canvas, then the follow-up drag frame reports the
    /// clamped edge of that camera's draw rect — not the default camera's, and
    /// not nothing.
    fn selection_drag_clamps_to_the_live_camera_rect() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.pan_by(5, -3);
        camera.set_zoom_percent(200);
        let view = CanvasView {
            canvas_size: (16, 12),
            grid_visible: false,
            selection_drag_active: true,
            ..Default::default()
        };

        let step = |ctx: &egui::Context,
                    widget: &mut CanvasWidget,
                    camera: &mut Camera,
                    events: Vec<egui::Event>| {
            let (result, _) = run_frame_with_view(ctx, events, widget, view.clone(), *camera);
            *camera = result.updated_camera;
            result
        };
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 5.0))],
        );
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![primary_press(egui::pos2(10.0, 5.0))],
        );
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(11.0, 6.0))],
        );
        let continued = step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(900.0, 900.0))],
        );

        assert_eq!(
            continued.stroke_point,
            Some((15, 11)),
            "the drag must clamp into this camera's canvas rect: x 5..37, y -3..21 at 2x"
        );
    }

    #[test]
    /// Given no live selection drag, when a drag's footprint still overlaps
    /// the canvas from outside the draw rect, then the strict footprint anchor
    /// is reported unchanged, and a footprint clear of the canvas reports
    /// nothing.
    fn non_selection_drag_keeps_the_existing_footprint_mapping() {
        let ctx = egui::Context::default();
        let mut widget = CanvasWidget::new();
        let mut camera = Camera::new();
        camera.set_zoom_percent(400);
        let spec = crate::core::brush::BrushSpec::new(16, crate::core::brush::BrushShape::Square);
        let view = CanvasView {
            canvas_size: (32, 32),
            grid_visible: false,
            brush: Some(spec),
            selection_drag_active: false,
            ..Default::default()
        };

        let step = |ctx: &egui::Context,
                    widget: &mut CanvasWidget,
                    camera: &mut Camera,
                    events: Vec<egui::Event>| {
            let (result, _) = run_frame_with_view(ctx, events, widget, view.clone(), *camera);
            *camera = result.updated_camera;
            result
        };
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))],
        );
        step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![primary_press(egui::pos2(10.0, 10.0))],
        );
        // Screen (130,10) is past the 128 pt draw rect, but a 16 px footprint
        // anchored at (33,3) still spans 25..40 x -5..11 and touches the canvas.
        let overlapping = step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(130.0, 10.0))],
        );
        assert_eq!(
            overlapping.stroke_point,
            Some((33, 3)),
            "a non-selection tool must keep the footprint anchor, not the clamped edge"
        );

        let clear = step(
            &ctx,
            &mut widget,
            &mut camera,
            vec![egui::Event::PointerMoved(egui::pos2(400.0, 10.0))],
        );
        assert_eq!(
            clear.stroke_point, None,
            "a footprint clear of the canvas must still report nothing"
        );
    }

    /// Görev HH perf probe: measures the mask marching-ants helpers vs
    /// boundary perimeter length. Not run by default (it is a timing bench);
    /// run with `cargo test --release --lib profile_mask_hotspot -- --ignored
    /// --nocapture`.
    ///
    /// BEFORE (Task II, the old `paint_ants_mask`): `mask_polylines` for the
    /// baseline plus `mask_dash_segments`, which chained the same boundary
    /// internally *and* re-scanned every edge from zero per dash (O(n²/period)).
    /// AFTER (Task II): the cached polylines feed the black baseline and the
    /// single-cursor O(n) dash walk, so there is zero per-frame chaining.
    #[test]
    #[ignore = "timing micro-bench, run manually with --ignored --nocapture"]
    fn profile_mask_hotspot() {
        fn square_boundary(side: i32) -> Vec<((i32, i32), (i32, i32))> {
            let mut v = Vec::new();
            for x in 0..side {
                v.push(((x, 0), (x + 1, 0)));
                v.push(((x, side), (x + 1, side)));
            }
            for y in 0..side {
                v.push(((0, y), (0, y + 1)));
                v.push(((side, y), (side, y + 1)));
            }
            v
        }
        fn time<F: FnMut()>(iters: u32, mut f: F) -> f64 {
            let t = std::time::Instant::now();
            for _ in 0..iters {
                f();
            }
            t.elapsed().as_secs_f64() * 1.0e6 / f64::from(iters)
        }
        for side in [64_i32, 128, 256, 512, 1024] {
            let segs = square_boundary(side);
            let n = segs.len();
            // The cached baseline (computed once per selection, not per frame).
            let cached = mask_polylines(&segs);
            let pl = time(20, || {
                std::hint::black_box(mask_polylines(std::hint::black_box(&segs)));
            });
            let new_walk = time(20, || {
                std::hint::black_box(
                    crate::render::overlay::mask_dash_segments_from_polylines(
                        std::hint::black_box(&cached),
                        1234,
                    ),
                );
            });
            // BEFORE: baseline `mask_polylines` + `mask_dash_segments` (which
            // recomputed the polylines internally and ran the legacy quadratic
            // walk) = 2 chainings + O(n²/period) per frame.
            let before = time(20, || {
                std::hint::black_box(mask_polylines(std::hint::black_box(&segs)));
                let pl = mask_polylines(std::hint::black_box(&segs));
                std::hint::black_box(
                    crate::render::overlay::legacy_mask_dash_segments_from_polylines(
                        std::hint::black_box(&pl),
                        1234,
                    ),
                );
            });
            // AFTER: zero per-frame chaining; the cached polylines feed both the
            // baseline and the O(n) dash walk.
            let after = new_walk;
            println!(
                "side={side:5} perimeter={n:6}  mask_polylines={pl:10.2}us  BEFORE={before:10.2}us  AFTER(cached+O(n))={after:9.2}us  speedup={:6.1}x",
                before / after.max(1e-9)
            );
        }
    }

    /// HH: `set_overlay` memoises the chained mask polylines and only
    /// recomputes them when the boundary actually changes.
    #[test]
    fn set_overlay_memoises_ants_polylines_per_boundary() {
        let mut widget = CanvasWidget::new();
        let a = vec![
            ((2, 2), (6, 2)),
            ((6, 2), (6, 4)),
            ((6, 4), (2, 4)),
            ((2, 4), (2, 2)),
        ];
        widget.set_overlay(CanvasOverlay::AntsMask(a.clone()));
        assert_eq!(
            widget.cached_ants_polylines(),
            mask_polylines(&a).as_slice()
        );

        // Same boundary again: the cache still matches the painted boundary.
        widget.set_overlay(CanvasOverlay::AntsMask(a.clone()));
        assert_eq!(
            widget.cached_ants_polylines(),
            mask_polylines(&a).as_slice()
        );

        // A different boundary must recompute.
        let b = vec![
            ((0, 0), (3, 0)),
            ((3, 0), (3, 3)),
            ((3, 3), (0, 3)),
            ((0, 3), (0, 0)),
        ];
        widget.set_overlay(CanvasOverlay::AntsMask(b.clone()));
        assert_eq!(
            widget.cached_ants_polylines(),
            mask_polylines(&b).as_slice()
        );

        // A region-cell gesture with a base selection uses the same cache.
        let mut widget = CanvasWidget::new();
        widget.set_overlay(CanvasOverlay::RegionCells {
            cells: vec![Rect2i::new(0, 0, 1, 1)],
            base: a.clone(),
        });
        assert_eq!(
            widget.cached_ants_polylines(),
            mask_polylines(&a).as_slice()
        );

        // An overlay without a mask boundary leaves the cache untouched.
        widget.set_overlay(CanvasOverlay::None);
        assert_eq!(
            widget.cached_ants_polylines(),
            mask_polylines(&a).as_slice()
        );
    }

    #[test]
    fn animation_placer_colors_come_from_theme_tokens() {
        let colors = Theme::default_dark().colors;
        let (red, green, grey) = animation_placer_colors(&colors);
        // No hard-coded UI colour: the three strokes map to theme tokens.
        assert_eq!(red, colors.onion_prev_tint32());
        assert_eq!(green, colors.onion_next_tint32());
        assert_eq!(grey, colors.marching_ants_under32());
        // The three boundaries must be visually distinguishable.
        assert_ne!(red, green);
        assert_ne!(red, grey);
        assert_ne!(green, grey);
    }

    #[test]
    fn onion_multiply_tint_darkens_rgb_and_preserves_alpha() {
        // Given: a straight onion token (the theme's light orange at alpha 140)
        // and the approximate-multiply factor.
        // When: it is darkened for the onion overlay.
        // Then: only RGB is scaled by ONION_MULTIPLY; alpha is unchanged, and
        // the result is strictly darker than the raw token.
        let base = Color::rgba(255, 190, 120, 140);
        let dark = onion_multiply_tint(base);
        let expected = egui::Color32::from_rgba_unmultiplied(
            (255.0 * ONION_MULTIPLY).round() as u8,
            (190.0 * ONION_MULTIPLY).round() as u8,
            (120.0 * ONION_MULTIPLY).round() as u8,
            140,
        );
        assert_eq!(dark, expected);
        assert_eq!(dark.a(), 140, "alpha must be preserved (140)");
        let raw = egui::Color32::from_rgba_unmultiplied(255, 190, 120, 140);
        assert!(
            dark.r() < raw.r() && dark.g() < raw.g() && dark.b() < raw.b(),
            "the tint must be darker than the raw token"
        );

        // The theme tokens the overlay uses are the source of truth.
        let colors = Theme::default_dark().colors;
        for token in [colors.onion_prev_view_tint_core(), colors.onion_next_view_tint_core()] {
            let darkened = onion_multiply_tint(token);
            assert_eq!(darkened.a(), token.a, "token alpha must be preserved");
        }
    }

    #[test]
    fn region_viewport_maps_screen_to_absolute_canvas() {
        // Fix 1: a region-scoped widget draws only the sub-rect, but the
        // screen→canvas mapping must still produce ABSOLUTE canvas coordinates
        // (so editing writes the right pixels).
        let mut widget = CanvasWidget::new();
        widget.set_sheet_region(Some(Rect2i::new(16, 32, 8, 8)));
        widget.last_origin = egui::pos2(100.0, 50.0);
        let camera = Camera::new(); // pan (0,0), 100%

        // The region's top-left draws at the widget origin (local camera 0).
        assert_eq!(
            widget.canvas_rect_to_screen(camera, Rect2i::new(16, 32, 8, 8)),
            egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(8.0, 8.0))
        );
        // A point 2px right / 5px down inside the region is absolute (18, 37).
        assert_eq!(
            widget.screen_to_canvas(egui::pos2(102.0, 55.0), camera, (128, 128)),
            Some((18, 37))
        );
        // A point left of the region draw rect is outside it.
        assert_eq!(
            widget.screen_to_canvas(egui::pos2(99.0, 50.0), camera, (128, 128)),
            None
        );
        // An empty region is treated as "no region" (full canvas).
        widget.set_sheet_region(Some(Rect2i::new(0, 0, 0, 0)));
        assert_eq!(widget.sheet_region(), None);
    }
}
