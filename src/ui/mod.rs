//! UI layer — egui panels, canvas widget, skin system, preview manager.
//!
//! R0 (F3): bootstrap entry `run()` — manual winit + egui-winit + egui-wgpu.
//! R0 (F4): [`projection`] module — PreviewManager + View menu.
//! R1: the interactive App shell — document, brush, undo stack, canvas widget,
//! and renderer wired into one winit + egui-winit + egui-wgpu loop.
//! R2: tool state machine (pencil/eraser/fill/eyedropper/select), clipboard, and layer/toolbar panels wired into the App shell.
//!
//! The active [`project::ProjectSession`] owns project-scoped editing state
//! (tool, color, brush size, selection), while the App shell owns host/UI
//! state and applies panel events as undoable commands (D59, D62). Toolbar and
//! layer panels are pure views that only emit events.

pub mod canvas;
pub mod clipboard;
pub mod dock_color_palette_panel;
pub mod dock_color_picker_panel;
pub mod dock_hosts;
pub mod dock_layers_panel;
pub mod dock_layers_settings;
pub mod dock_tool_property_panel;
pub mod dock_toolbox_panel;
pub mod host_registry;
pub mod input_capture;
pub mod keybindings;
pub mod layers;
mod native_host;
pub mod panel_dock;
pub mod panel_layout;
pub mod panel_registry;
pub mod project;
pub mod project_tabs;
pub mod projection;
pub mod settings;
pub mod theme;
pub mod timeline;
pub mod toolbar;
pub mod workspace_persistence;
pub mod workspace_runtime;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::core::anim::AnimationController;
use crate::core::brush::{snap_line_angle, BrushShape, BrushSpec, DrawMode, DrawTool, Stroke};
use crate::core::buffer::PixelBuffer;
use crate::core::clip::PixelClip;
use crate::core::clipboard::ClipboardRegion;
use crate::core::color::Color;
use crate::core::document::{Document, FORMAT_VERSION};
use crate::core::fill::fill_command_clipped;
use crate::core::layer_commands::{
    apply_layer_commits_with, LayerStackController, TransformSource,
};
use crate::core::math::Rect2i;
use crate::core::model::frame::Frame;
use crate::core::model::region::Region;
use crate::core::model::sequence::AnimationSequence;
use crate::core::model::sequence::MAX_FRAMES;
use crate::core::model::{LayerId, LayerStack};
use crate::core::select::{
    delete_selected_command, flip_selected_command, rotate_selected_command, SelectMode, Selection,
};
use crate::core::stroke_command::StrokeSession;
use crate::core::transform::{
    CurveTransform, LayerBuffer, LayerCommit, TransformAlgorithm, TransformObject, GIZMO_HIT_RADIUS,
};
use crate::core::undo::{CommandContext, DeltaRecorder};
use crate::input::{FieldierChild, Keymap, Tool};
use crate::io::{
    autosave_dir_for, clear_recovery_journal, decode_png, encode_gif, encode_png, load_document,
    load_keymap, load_palettes, load_project_palettes, read_recovery_journal, save_document,
    save_keymap, sprite_sheet_meta_json, user_keymap_path, user_palette_dir,
    write_recovery_journal, GifFrameInput, RecoveryJournal,
};
use crate::render::gizmo::GizmoHit;
use crate::render::RendererState;
use canvas::{
    scroll_steps, CanvasInteractions, CanvasOverlay, CanvasView, CanvasWidget, HudState,
    LinePreview,
};
use dock_color_palette_panel::{palette_panel_spec, PalettePanelEvent};
use dock_hosts::{ColorPickerView, LayerRow, LayerView, PaletteView, ToolboxView};
use host_registry::{ContextGenerationService, ContextSnapshot, WindowHostRegistry};
use input_capture::{InputCapture, SurfaceId};
use keybindings::KeybindingsPanel;
use layers::{drop_placement, DropPlacement, LayerPanel, LayerPanelEvent};
use native_host::{NativeHostSpec, NativeWorkspaceHost};
use panel_dock::skin::{AtlasCache, PanelChrome, SkinAtlas};
use panel_layout::PanelLayout;
use panel_registry::{PanelId, PanelPlacement};
use project::{
    ActivationToken, AnchorEnd, CurveTransformSession, DrawSettings, MoveAxis, PendingAction,
    ProjectStore, SelectGesture, SelectionTransform, TransformSession,
};
use project_tabs::ProjectTabBar;
use settings::SettingsPanel;
use theme::{load_atlas_with_size, ThemeManager};
use timeline::{clamp_reorder_target, TimelineEvent, TimelinePanel};
use toolbar::{ToolbarEvent, ToolbarWidget};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};
use workspace_persistence::WorkspaceLayoutV1;
use workspace_runtime::{
    WorkspacePanelAction, WorkspacePanelCoordinator, WorkspacePanelState, WORKSPACE_PANEL_IDS,
};

struct NativeHostRetirement<T> {
    pending: Vec<T>,
}

impl<T> NativeHostRetirement<T> {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    fn enqueue(&mut self, host: T) {
        self.pending.push(host);
    }

    fn advance(&mut self) {
        self.pending.clear();
    }
}

/// Document size in pixels (the canvas is a 128×128 transparent buffer).
const CANVAS_WIDTH: usize = 128;
const CANVAS_HEIGHT: usize = 128;

/// R6 F3: autosave cadence (5 minutes).
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(300);

const TRANSFORM_PREVIEW_MAX_DIM: usize = 64;

/// Y2-B fast preview algorithm: while a transform gizmo is being DRAGGED the
/// live preview renders with Rotxel — the palette-pure, low-memory CPU path
/// (X2) — instead of the user-selected algorithm. Commit is unaffected and
/// always uses the selected algorithm (the preview==commit guarantee holds at
/// commit). Choosing through the existing [`TransformAlgorithm`] dispatch means
/// no new variant and exact 0/90/180/270° results stay byte-identical.
const TRANSFORM_DRAG_PREVIEW_ALGORITHM: TransformAlgorithm = TransformAlgorithm::Rotxel;

/// The rotation algorithm the live transform preview must use: the fast drag
/// algorithm while the object is dragged, else the selected (stored)
/// algorithm. Kept as a pure function so the two-tier choice is unit-testable
/// without a texture upload.
fn transform_preview_algorithm(dragging: bool, selected: TransformAlgorithm) -> TransformAlgorithm {
    if dragging {
        TRANSFORM_DRAG_PREVIEW_ALGORITHM
    } else {
        selected
    }
}

/// Dirty key for the transform preview texture (item 7): derived from the
/// live transform state so [`App::refresh_transform_preview`] only re-renders
/// and re-uploads when the float actually changed (pos / pivot / angle / scale
/// / flips). f32 state is compared by bit pattern, which is stable for an
/// unchanged object. `dragging` is part of the key (Y2-B) because it selects
/// the preview algorithm, so a release forces a re-render with the selected
/// algorithm even when the transform state itself is unchanged.
#[derive(Clone, Copy, PartialEq, Debug)]
struct TransformPreviewKey {
    pos: (u32, u32),
    pivot: (u32, u32),
    angle: u32,
    scale: (u32, u32),
    flips: (bool, bool),
    dragging: bool,
}

impl TransformPreviewKey {
    fn of(object: &TransformObject, dragging: bool) -> Self {
        let (sx, sy) = object.scale_xy();
        Self {
            pos: (object.pos.0.to_bits(), object.pos.1.to_bits()),
            pivot: (object.pivot().0.to_bits(), object.pivot().1.to_bits()),
            angle: object.angle_deg.to_bits(),
            scale: (sx.to_bits(), sy.to_bits()),
            flips: (object.flip_h, object.flip_v),
            dragging,
        }
    }
}

fn recovery_base_for(base: &std::path::Path, project: project::ProjectId) -> PathBuf {
    base.join(format!("project-{}", project.raw()))
}

fn workspace_layout_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("pyxross").join("workspace.json"))
}

fn preview_image_for_session(session: &project::ProjectSession) -> egui::ColorImage {
    let composite = session.layers.composite_layers();
    egui::ColorImage::from_rgba_unmultiplied(
        [composite.width(), composite.height()],
        composite.as_bytes(),
    )
}

fn native_render_succeeded(result: Result<(), ()>) -> bool {
    result.is_ok()
}

/// Uniform outer frame for every panel (F6): a thin inner margin with no
/// fill or stroke, so the four panels share one visual treatment.
fn panel_frame() -> egui::Frame {
    egui::Frame::new().inner_margin(4.0)
}

pub fn run() {
    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop
        .run_app(&mut App::default())
        .expect("Event loop failed");
}

struct App {
    window: Option<Window>,
    renderer: Option<RendererState>,
    egui_state: Option<egui_winit::State>,
    ctx: egui::Context,
    projects: ProjectStore,
    canvas_widget: CanvasWidget,
    canvas_texture: Option<egui::TextureHandle>,
    texture_dirty: bool,
    /// Host cache identity; a project switch forces the next sync to rebuild
    /// even when dimensions and layer dirty flags are unchanged.
    canvas_cache_token: Option<ActivationToken>,
    toolbar: ToolbarWidget,
    layer_panel: LayerPanel,
    /// Shared per-frame cells for the docked Toolbox panel (pure view).
    toolbox_host: dock_hosts::ToolboxHost,
    /// Shared per-frame cells for the docked Layers panel (pure view).
    layers_host: dock_hosts::LayerPanelHost,
    /// Shared per-frame cells for the floating Color Picker panel (pure view).
    color_picker_host: dock_hosts::ColorPickerHost,
    /// Shared per-frame cells for the docked Palette panel (pure view).
    palette_host: dock_hosts::PalettePanelHost,
    /// Theme browser panel (F4): switch/refresh themes and preview colors.
    settings: SettingsPanel,
    keybindings: KeybindingsPanel,
    panel_layout: PanelLayout,
    project_tabs: ProjectTabBar,
    workspace_layout: WorkspaceLayoutV1,
    host_registry: WindowHostRegistry,
    workspace_runtime: WorkspacePanelCoordinator,
    context_generation: ContextGenerationService,
    native_hosts: BTreeMap<host_registry::HostId, NativeWorkspaceHost>,
    native_host_retirement: NativeHostRetirement<NativeWorkspaceHost>,
    pending_native_hosts: BTreeSet<host_registry::HostId>,
    deferred_workspace_actions: Vec<WorkspacePanelAction>,
    keymap: Keymap,
    /// The active theme; owns the color tokens every panel reads.
    theme: ThemeManager,
    /// Loaded skin atlases for the active theme, keyed by (theme name, theme
    /// dir). `None` when the theme has no skins or no file dir.
    skin_atlas_cache: Option<(String, PathBuf, AtlasCache)>,
    // -- timeline + playback (U13, R3 wave, feature F3) ------
    timeline: TimelinePanel,
    panel_dock: panel_dock::DockManager,
    panel_dock_demo: bool,
    /// Gesture router shared by the canvas and every nest.
    input_capture: InputCapture,
    /// R4: current gizmo handle under the pointer (updated each frame).
    gizmo_hovered: GizmoHit,
    /// Live transform session (D32/D35/D36): a lifted floating object awaiting
    /// commit (double-click on empty space / tool switch) or cancel (Esc
    /// restores the lifted selection; a structural change drops it).
    /// `None` when idle.
    ///
    /// Whether the selection lifted into the live session had a mask. Recorded
    /// at lift time because the lift keeps only the bounding rect, and a masked
    /// commit deliberately leaves no selection behind instead of re-capturing a
    /// plain rect over the transformed bbox.
    lifted_selection_is_masked: bool,
    /// Per-frame texture backing the floating-pixels preview while a session
    /// is active (D32): re-uploaded from `TransformObject::render(None)`.
    transform_preview: Option<egui::TextureHandle>,
    /// Dirty key of the last uploaded [`Self::transform_preview`]: the preview
    /// is only re-rendered + re-uploaded when the live transform state changes
    /// (item 7).
    transform_preview_key: Option<TransformPreviewKey>,
    /// R6 F2: the directory of the currently open `.pyxross` project, if any.
    /// R6 F2: the last persistence error message, shown in the status bar.
    /// R6 F2: display name of the current project (from the save directory
    /// name, or "Untitled" until the first save).
    /// R6 F2: true when there are unsaved edits since the last save/load/new.
    /// R6 F3: base directory for autosaves and the recovery journal (None until
    /// resumed() resolves the config dir).
    autosave_base: Option<PathBuf>,
    /// Primary-button tool gesture state: armed when a press outside the canvas
    /// (on no other UI that wants the pointer) started the active tool's
    /// session without processing data. `tool_outside` tracks whether the
    /// pointer is currently outside the canvas draw rect while armed: painting
    /// pauses there and restarts as a new segment on re-entry.
    tool_armed: bool,
    tool_outside: bool,
    /// Last canvas point a Fieldier selection drag projected onto the canvas
    /// while the pointer was outside it, held so the gesture keeps following
    /// when the platform reports no pointer position at all.
    last_selection_drag_point: Option<(i32, i32)>,
    /// Line mode latched for the current primary gesture: a Shift+press on a
    /// draw tool previews an anchor→end line instead of painting, and release
    /// commits it as exactly one undoable stroke. `line_anchor` is the press
    /// point; `line_end` the live (Ctrl-snapped) end point.
    line_mode: bool,
    line_anchor: Option<(i32, i32)>,
    line_end: Option<(i32, i32)>,
    /// True while the latched line gesture is the Alt-driven dynamic variant,
    /// which becomes a transform object on release instead of a stroke.
    line_dynamic: bool,
    /// The dynamic-curve gizmo under the pointer this frame, if any.
    curve_hovered: Option<usize>,
    /// Persistent OS image clipboard owner (G3 finding #6). A single
    /// `arboard::Clipboard` is kept alive across frames so on Wayland the
    /// clipboard owner connection does not die and drop the copied image the
    /// moment a copy returns. Created lazily on first write.
    os_clipboard: crate::ui::clipboard::OsClipboard,
    /// In-flight asynchronous OS clipboard read for paste (G3 finding #6).
    /// `Some` while a background worker is reading the OS clipboard; polled
    /// once per frame so the render thread never blocks in `get_image()`.
    /// The wrapper also carries the bounded-wait frame counter, so a hung read
    /// is abandoned and paste falls back to the internal clipboard instead of
    /// wedging forever (Task H1, second round).
    os_paste_rx: Option<crate::ui::clipboard::PendingPasteRead>,
    /// The canvas pixel the pending async paste is centred on, captured when
    /// the read was requested so the result lands where Ctrl+V was pressed.
    os_paste_cursor: Option<(i32, i32)>,
    _project_cache_generation: u64,
}

impl Default for App {
    fn default() -> Self {
        let keymap = load_keymap(user_keymap_path().as_deref());
        let projects = ProjectStore::new();
        let toolbox_host = dock_hosts::ToolboxHost::default();
        let layers_host = dock_hosts::LayerPanelHost::default();
        let color_picker_host = dock_hosts::ColorPickerHost::default();
        let palette_host = dock_hosts::PalettePanelHost::default();
        let mut panel_dock = panel_dock::DockManager::demo();
        panel_dock
            .add_spec(dock_toolbox_panel::toolbox_panel_spec(
                &toolbox_host,
                panel_dock::PanelPlacement::DockedLeft,
            ))
            .expect("toolbox panel id must be unique");
        panel_dock
            .add_spec(dock_tool_property_panel::tool_property_panel_spec(
                &toolbox_host,
                panel_dock::PanelPlacement::DockedLeft,
            ))
            .expect("tool property panel id must be unique");
        panel_dock
            .add_spec(dock_layers_panel::layers_panel_spec(
                &layers_host,
                panel_dock::PanelPlacement::DockedRight,
            ))
            .expect("layers panel id must be unique");
        panel_dock
            .add_spec(palette_panel_spec(
                &palette_host,
                panel_dock::PanelPlacement::DockedRight,
            ))
            .expect("palette panel id must be unique");
        let mut app = Self {
            window: None,
            renderer: None,
            egui_state: None,
            ctx: egui::Context::default(),
            projects,
            canvas_widget: CanvasWidget::new(),
            canvas_texture: None,
            texture_dirty: true,
            canvas_cache_token: None,
            toolbar: ToolbarWidget::new(),
            layer_panel: LayerPanel::new(),
            toolbox_host,
            layers_host,
            color_picker_host,
            palette_host,
            settings: SettingsPanel::default(),
            keybindings: KeybindingsPanel::new(&keymap),
            panel_layout: PanelLayout::new(),
            project_tabs: ProjectTabBar::new(),
            workspace_layout: WorkspaceLayoutV1::default(),
            host_registry: WindowHostRegistry::new(),
            workspace_runtime: WorkspacePanelCoordinator::new(),
            context_generation: ContextGenerationService::new(),
            native_hosts: BTreeMap::new(),
            native_host_retirement: NativeHostRetirement::new(),
            pending_native_hosts: BTreeSet::new(),
            deferred_workspace_actions: Vec::new(),
            keymap,
            theme: ThemeManager::new(),
            skin_atlas_cache: None,
            gizmo_hovered: GizmoHit::None,
            lifted_selection_is_masked: false,
            transform_preview: None,
            transform_preview_key: None,
            autosave_base: None,
            timeline: TimelinePanel::new(),
            panel_dock,
            input_capture: InputCapture::new(),
            panel_dock_demo: true,
            tool_armed: false,
            tool_outside: false,
            last_selection_drag_point: None,
            line_mode: false,
            line_anchor: None,
            line_end: None,
            line_dynamic: false,
            curve_hovered: None,
            os_clipboard: crate::ui::clipboard::OsClipboard::new(),
            os_paste_rx: None,
            os_paste_cursor: None,
            _project_cache_generation: 0,
        };
        let project = app.new_project("Untitled", CANVAS_WIDTH, CANVAS_HEIGHT);
        app.projects
            .session_mut(project)
            .expect("new project must resolve")
            .palettes = load_palettes(user_palette_dir().as_deref());
        app
    }
}

/// True when this frame's primary press carries Shift, or Shift is held.
///
/// `InputState::modifiers` tracks held keys (winit emits `ModifiersChanged`),
/// but synthetic presses also carry the modifier on the event itself, so both
/// are checked — the same seam as `shift_wheel`/`ctrl_wheel`.
///
/// In a transform drag Shift means: **rotate** → snap the absolute angle to a
/// 45° multiple; **corner scale** → aspect-lock (uniform, dominant-axis ratio);
/// **edge scale** → ignored (edges are already single-axis).
fn shift_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.modifiers.shift
            || input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.shift
                )
            })
    })
}

/// True when this frame's primary press carries Alt, or Alt is held.
///
/// Mirrors [`shift_pressed`]: `InputState::modifiers` tracks held keys while
/// synthetic presses also carry the modifier on the event itself.
///
/// In a transform drag Alt means **resize from / about the centre**: a corner
/// or edge scale anchors at the transformed bbox centre (both opposite sides
/// move evenly) instead of the opposite corner/edge, and the pivot is forced
/// back onto the bbox centre afterwards. With Shift it is additionally a
/// uniform (dominant-axis) centre scale. Alt is no longer the pivot-only
/// modifier and Ctrl is no longer part of the transform map.
fn alt_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.modifiers.alt
            || input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.alt
                )
            })
    })
}

/// Shift-rotation snap points in degrees: 45° multiples only, so a Shift-held
/// rotate lands on the canonical diagonals. The list is NOT limited to 0–180:
/// the four quadrants are all present so wrapping across 0°/360° is natural.
/// (The old 26.565°/153.435° atan(0.5) points were removed — they are no longer
/// part of the transform modifier map.)
const ROTATE_SNAP_DEG: [f32; 8] = [0.0, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0];

/// Snap an absolute angle in degrees to the nearest [`ROTATE_SNAP_DEG`]
/// point, measured with wrap-around across 0°/360°.
fn snap_rotate_angle(deg: f32) -> f32 {
    let n = deg.rem_euclid(360.0);
    ROTATE_SNAP_DEG
        .iter()
        .copied()
        .min_by(|a, b| {
            let da = (n - *a).abs().min(360.0 - (n - *a).abs());
            let db = (n - *b).abs().min(360.0 - (n - *b).abs());
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(n)
}

/// The cursor for a gizmo hit at the object's CURRENT screen orientation.
///
/// The corner/edge resize cursors are display-only and must follow the rotated
/// bbox: a corner's diagonal flips between NWSE and NESW every 90° of rotation,
/// and an edge's axis flips between vertical and horizontal every 90°. The
/// check uses the nearest 90° quadrant (`angle_deg` in degrees, clockwise
/// positive, matching the transform convention). `Translate` is `Grab` (the
/// App overrides to `Grabbing` while the body is actually dragged); the rotate
/// zones stay a fixed `Crosshair`.
fn cursor_for_hit_at_angle(hit: GizmoHit, angle_deg: f32) -> Option<egui::CursorIcon> {
    let odd_quadrant = ((angle_deg / 90.0).round() as i64).rem_euclid(2) == 1;
    match hit {
        GizmoHit::None => None,
        GizmoHit::Translate => Some(egui::CursorIcon::Grab),
        GizmoHit::ScaleNW | GizmoHit::ScaleSE => Some(if odd_quadrant {
            egui::CursorIcon::ResizeNeSw
        } else {
            egui::CursorIcon::ResizeNwSe
        }),
        GizmoHit::ScaleNE | GizmoHit::ScaleSW => Some(if odd_quadrant {
            egui::CursorIcon::ResizeNwSe
        } else {
            egui::CursorIcon::ResizeNeSw
        }),
        GizmoHit::ScaleTop | GizmoHit::ScaleBottom => Some(if odd_quadrant {
            egui::CursorIcon::ResizeHorizontal
        } else {
            egui::CursorIcon::ResizeVertical
        }),
        GizmoHit::ScaleLeft | GizmoHit::ScaleRight => Some(if odd_quadrant {
            egui::CursorIcon::ResizeVertical
        } else {
            egui::CursorIcon::ResizeHorizontal
        }),
        GizmoHit::Rotate => Some(egui::CursorIcon::Crosshair),
    }
}

/// Signed per-axis ratio of the pointer's distance from the fixed anchor,
/// measured against the START pointer: `(cur − f) / (start_pointer − f)`.
///
/// Because the denominator is the START pointer (captured once), the target is
/// ABSOLUTE from the press and cannot compound float/`round` error across
/// frames. The sign is meaningful: a negative ratio means the pointer crossed
/// the anchor and the axis must FLIP. A pointer sitting exactly on the anchor
/// (zero denominator) leaves the axis unchanged.
fn absolute_axis_ratio(start_pointer: f32, cur: f32, anchor: f32) -> f32 {
    let denom = start_pointer - anchor;
    if denom.abs() > 1e-9 {
        (cur - anchor) / denom
    } else {
        1.0
    }
}

/// T2-B: the larger keyboard-move step used while Shift (or Alt) is held
/// during a transform session.
///
/// The spec prefers the project's region/tile grid size, but a
/// [`SelectionTransform`] session does not carry it, so the documented
/// fallback fixed step of 10 px is used; the unmodified step is exactly 1 px.
const TRANSFORM_KEY_MOVE_LARGE_STEP: f32 = 10.0;

/// Inverse-rotate a canvas-space vector by `angle_deg` (apply `Rᵀ`). Mirrors
/// the core pipeline's rotation with the sign flipped, including the
/// exact-90°-multiple integer coefficient tables.
fn rotate_inv(v: (f32, f32), angle_deg: f32) -> (f32, f32) {
    match angle_deg.rem_euclid(360.0) {
        0.0 => v,
        90.0 => (v.1, -v.0),
        180.0 => (-v.0, -v.1),
        270.0 => (-v.1, v.0),
        _ => {
            let theta = (angle_deg as f64).to_radians();
            let (c, s) = (theta.cos() as f32, theta.sin() as f32);
            (c * v.0 + s * v.1, -s * v.0 + c * v.1)
        }
    }
}

/// The source-space resize anchor for a grabbed handle: the point of the local
/// source rect `[0,w]×[0,h]` that must stay fixed, plus which side of the box
/// that anchor is on per axis.
///
/// Corners anchor at the OPPOSITE corner; edge handles at the OPPOSITE edge
/// midpoint; Alt (any handle) at the local CENTRE. Because the anchor is a
/// fixed point of the pixel content, it is expressed in the source's own local
/// coordinates and then mapped into canvas space by [`canvas_anchor`].
fn anchor_src_for(hit: GizmoHit, w: f32, h: f32, alt: bool) -> ((f32, f32), AnchorEnd, AnchorEnd) {
    use AnchorEnd::{Center, Max, Min};
    if alt {
        return ((w * 0.5, h * 0.5), Center, Center);
    }
    match hit {
        GizmoHit::ScaleNW => ((w, h), Max, Max),
        GizmoHit::ScaleNE => ((0.0, h), Min, Max),
        GizmoHit::ScaleSE => ((0.0, 0.0), Min, Min),
        GizmoHit::ScaleSW => ((w, 0.0), Max, Min),
        GizmoHit::ScaleTop => ((w * 0.5, h), Center, Max),
        GizmoHit::ScaleRight => ((0.0, h * 0.5), Min, Center),
        GizmoHit::ScaleBottom => ((w * 0.5, 0.0), Center, Min),
        GizmoHit::ScaleLeft => ((w, h * 0.5), Max, Center),
        _ => ((w * 0.5, h * 0.5), Center, Center),
    }
}

/// Map a source-space point `u ∈ [0,w]×[0,h]` to canvas space through the
/// object's rotated quad. The quad is a parallelogram, so the affine blend of
/// three corners is exact for any `u`.
fn canvas_anchor(corners: [(f32, f32); 4], w: f32, h: f32, u: (f32, f32)) -> (f32, f32) {
    let tx = if w.abs() > 1e-9 { u.0 / w } else { 0.0 };
    let ty = if h.abs() > 1e-9 { u.1 / h } else { 0.0 };
    let (tl, tr, bl) = (corners[0], corners[1], corners[3]);
    (
        tl.0 + tx * (tr.0 - tl.0) + ty * (bl.0 - tl.0),
        tl.1 + tx * (tr.1 - tl.1) + ty * (bl.1 - tl.1),
    )
}

/// The new LOCAL min of one axis for a start-referenced resize.
///
/// `anchor` is the fixed anchor's local coordinate, `size` the positive target
/// extent and `end` which side of the box the anchor is (`Min` keeps it, `Max`
/// places it at `anchor − size`, `Center` centres the box). `crossed` mirrors
/// the box to the other side of the anchor (`ratio < 0`); a `Center` anchor is
/// already symmetric so a crossing does not move it.
fn desired_local_min(anchor: f32, size: f32, end: AnchorEnd, crossed: bool) -> f32 {
    let normal = match end {
        AnchorEnd::Center => anchor - size * 0.5,
        AnchorEnd::Min => anchor,
        AnchorEnd::Max => anchor - size,
    };
    if crossed && end != AnchorEnd::Center {
        2.0 * anchor - (normal + size)
    } else {
        normal
    }
}

/// Resize the session object to an ABSOLUTE, start-referenced LOCAL target.
///
/// `start_local_dims` is the pre-rotation local size. A TRACKED axis targets
/// `round(start_local_dim * |ratio|)` (never the current size times a
/// per-frame ratio), so the dragged handle tracks the cursor and never drifts;
/// an UNTRACKED edge axis keeps its start local dim. A negative ratio on a
/// TRACKED axis means the pointer crossed the anchor: the local size stays
/// positive and the object FLIPS on that axis. An untracked axis never flips
/// (T2-A): its ratio is projection noise from a perpendicular drag and its
/// mirror must stay at `start_flip`. The new local min is computed relative to
/// the fixed anchor and applied with
/// [`crate::core::transform::TransformObject::resize_local_to_pixels_at_min`],
/// so the anchor maps to itself under the SCALE-THEN-ROTATE pipeline.
fn resize_selection_absolute(t: &mut SelectionTransform, rx: f32, ry: f32, tracked: (bool, bool)) {
    let th = t.start_angle;
    // Anchor in the object's SCALED-LOCAL frame (undo the rotation about the
    // CURRENT pivot). `pivot_cur` is the same pivot the placement uses, so the
    // anchor math and the placement stay consistent across per-frame
    // recentring.
    let pivot_cur = t.object.pivot();
    let a_l = rotate_inv(
        (
            t.start_anchor.0 - pivot_cur.0,
            t.start_anchor.1 - pivot_cur.1,
        ),
        th,
    );
    let (lw0, lh0) = t.start_local_dims;
    let tl_w = if tracked.0 {
        ((lw0 as f32) * rx.abs()).round().max(1.0) as u32
    } else {
        lw0 as u32
    };
    let tl_h = if tracked.1 {
        ((lh0 as f32) * ry.abs()).round().max(1.0) as u32
    } else {
        lh0 as u32
    };
    let (end_x, end_y) = t.start_anchor_end;
    let min_x = if tracked.0 {
        desired_local_min(a_l.0, tl_w as f32, end_x, rx < 0.0)
    } else {
        // Untracked axis: keep the (unchanged) box centred on the anchor's
        // local coordinate, i.e. its local min stays put.
        desired_local_min(a_l.0, lw0 as f32, AnchorEnd::Center, false)
    };
    let min_y = if tracked.1 {
        desired_local_min(a_l.1, tl_h as f32, end_y, ry < 0.0)
    } else {
        desired_local_min(a_l.1, lh0 as f32, AnchorEnd::Center, false)
    };
    // T2-A: FLIP only on a TRACKED axis. An edge handle has exactly one
    // tracked axis; the untracked axis is pinned to its start dim, so even
    // though the edge arm still feeds a projected ratio for it, that ratio
    // must never toggle the mirror. Without the `tracked` guard a
    // perpendicular drag across the opposite-edge-midpoint anchor pushes the
    // untracked ratio negative and flips the content on the wrong axis.
    let flip_h = t.start_flip_h ^ (tracked.0 && rx < 0.0);
    let flip_v = t.start_flip_v ^ (tracked.1 && ry < 0.0);
    t.object.set_flips(flip_h, flip_v);
    t.object
        .resize_local_to_pixels_at_min(tl_w, tl_h, (min_x, min_y));
}

/// Centre the transform pivot on the current transformed bbox WITHOUT moving
/// the rendered output.
///
/// M1 invariant: the pivot is ALWAYS the centre of the transformed pixels (the
/// gizmo bbox centre) and the user cannot move it. Rescaling anchors at the
/// opposite corner/edge (or the centre for Alt), so the pixel centre moves
/// while the stored pivot stays put; this helper re-derives the centre after
/// every scale.
///
/// `set_pivot` anchors the whole result to the pivot, so changing it alone
/// would shift the output. We therefore capture the desired bbox first, move
/// the pivot, then correct `pos` by the inverse of the SCALE-THEN-ROTATE
/// placement Jacobian `R(θ)·S` (whose inverse is `S⁻¹Rᵀ`) so `canvas_bbox()`
/// returns the desired box again exactly.
fn recenter_pivot_keeping_output(t: &mut SelectionTransform) {
    let (desired_x0, desired_y0, desired_x1, desired_y1) = t.object.canvas_bbox();
    let new_pivot = (
        (desired_x0 + desired_x1) * 0.5,
        (desired_y0 + desired_y1) * 0.5,
    );
    t.object.set_pivot(new_pivot);
    // Placement moved with the pivot; measure the shift and undo it via pos.
    let (after_x0, after_y0, _, _) = t.object.canvas_bbox();
    let shift_x = desired_x0 - after_x0;
    let shift_y = desired_y0 - after_y0;
    if shift_x == 0.0 && shift_y == 0.0 {
        return;
    }
    let (sx, sy) = t.object.scale_xy();
    let theta = t.object.angle_deg.to_radians();
    let (c, sn) = (theta.cos(), theta.sin());
    // (R·S)^-1 = S^-1 · R^T = [[c/sx, sn/sx], [-sn/sy, c/sy]].
    t.object.pos = (
        t.object.pos.0 + (c * shift_x + sn * shift_y) / sx,
        t.object.pos.1 + (-sn * shift_x + c * shift_y) / sy,
    );
}

/// Write a lifted selection's original pixels back into its source layer,
/// mask-aware and WITHOUT an undo entry (Part C).
///
/// Used by [`App::cancel_transform`] (Esc restores the pixels the lift cut) and
/// by the commit's transient restore (so [`apply_layer_commits_with`] sees the
/// original pixels before it records its single composite cut+paste step). A
/// masked source writes only mask-true cells (the snapshot already carries
/// zeros outside the mask); a rectangular source blits the whole rect. A paste
/// session has an empty snapshot and is a no-op.
fn restore_transform_source(layers: &mut LayerStack, source: &TransformSource) {
    if source.snapshot.is_empty() || source.rect.area() <= 0 {
        return;
    }
    let Some(layer) = layers.layer_mut(source.layer_id) else {
        return;
    };
    match source.mask.as_deref() {
        Some(mask) => {
            layer
                .buffer
                .blit_region_masked(source.rect, &source.snapshot, Some(mask));
        }
        None => {
            layer.buffer.blit_region(source.rect, &source.snapshot);
        }
    }
}

/// Derive the new MASKED selection after a masked transform commit (Part C):
/// the union destination rect of `commits` (clipped to the canvas), masked
/// where a COMMITTED pixel has alpha > 0. Returns `None` when the commits are
/// empty, fully off-canvas, or select nothing.
fn transformed_selection_for(
    layers: &LayerStack,
    target: LayerId,
    commits: &[LayerCommit],
) -> Option<Selection> {
    let canvas = Rect2i::new(0, 0, layers.width() as i32, layers.height() as i32);
    let mut union: Option<Rect2i> = None;
    for commit in commits {
        let rect = Rect2i::new(
            commit.dst.0.round() as i32,
            commit.dst.1.round() as i32,
            commit.w as i32,
            commit.h as i32,
        )
        .clamp_to(canvas);
        if rect.is_empty() {
            continue;
        }
        union = Some(match union {
            Some(u) => {
                let x = u.x.min(rect.x);
                let y = u.y.min(rect.y);
                let right = u.right().max(rect.right());
                let bottom = u.bottom().max(rect.bottom());
                Rect2i::new(x, y, right - x, bottom - y)
            }
            None => rect,
        });
    }
    let bbox = union?;
    let mut mask = vec![false; bbox.area() as usize];
    for commit in commits {
        let full = Rect2i::new(
            commit.dst.0.round() as i32,
            commit.dst.1.round() as i32,
            commit.w as i32,
            commit.h as i32,
        );
        let clipped = full.clamp_to(canvas);
        if clipped.is_empty() {
            continue;
        }
        for y in clipped.y..clipped.bottom() {
            for x in clipped.x..clipped.right() {
                let sx = (x - full.x) as usize;
                let sy = (y - full.y) as usize;
                let alpha = (sy * commit.w + sx) * 4 + 3;
                if commit.buf.get(alpha).copied().unwrap_or(0) == 0 {
                    continue;
                }
                let idx = ((y - bbox.y) as i64 * bbox.w as i64 + (x - bbox.x) as i64) as usize;
                mask[idx] = true;
            }
        }
    }
    let layer = layers.layer(target)?;
    Selection::capture_mask(&layer.buffer, bbox, mask)
}

/// Build the dock's layer rows in display order (row 0 = the TOPMOST layer).
///
/// The model's flat vec is bottom-to-top, so roots are walked from LAST to
/// FIRST and each node's children from LAST to FIRST. A group whose id is not
/// in `expanded_groups` still emits its own row but omits its whole subtree.
fn layer_rows_for_dock(layers: &LayerStack, expanded_groups: &HashSet<LayerId>) -> Vec<LayerRow> {
    fn emit(
        layers: &LayerStack,
        expanded_groups: &HashSet<LayerId>,
        id: LayerId,
        depth: usize,
        out: &mut Vec<LayerRow>,
    ) {
        let layer = layers.layer(id).expect("row id must exist");
        let children = layers.children_of(Some(id));
        let expanded = expanded_groups.contains(&id);
        out.push(LayerRow {
            id,
            name: layer.name.clone(),
            visible: layer.visible,
            opacity: layer.opacity,
            blend: layer.blend,
            is_group: layer.is_group,
            depth,
            has_children: !children.is_empty(),
            expanded,
        });
        if layer.is_group && expanded {
            for child in children.iter().rev() {
                emit(layers, expanded_groups, *child, depth + 1, out);
            }
        }
    }
    let mut rows = Vec::new();
    for root in layers.children_of(None).iter().rev() {
        emit(layers, expanded_groups, *root, 0, &mut rows);
    }
    rows
}

/// Whether the active layer can merge down: it is a leaf and the node directly
/// below it in the same parent exists and is a leaf (mirrors the `merge_down`
/// precondition).
fn merge_enabled_for(layers: &LayerStack, active: LayerId) -> bool {
    let Some(active_layer) = layers.layer(active) else {
        return false;
    };
    if active_layer.is_group {
        return false;
    }
    let siblings = layers.children_of(active_layer.parent);
    let Some(position) = siblings.iter().position(|&sibling| sibling == active) else {
        return false;
    };
    if position == 0 {
        return false;
    }
    !layers.is_group(siblings[position - 1])
}

impl App {
    fn canvas_rect(&self) -> Rect2i {
        Rect2i::new(
            0,
            0,
            self.projects.current().layers.width() as i32,
            self.projects.current().layers.height() as i32,
        )
    }

    fn invalidate_host_project_cache(&mut self) {
        self.texture_dirty = true;
        self.canvas_cache_token = None;
        self.transform_preview = None;
    }

    fn consume_activation(&mut self, outcome: project::ActivationOutcome) {
        if outcome.changed() {
            self.invalidate_host_project_cache();
            self.context_generation.advance();
            for host in self.native_hosts.values() {
                host.request_redraw();
            }
        }
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        self.projects
            .active_id()
            .map(|project| ContextSnapshot::new(project, self.context_generation.current()))
    }

    fn workspace_panel_controls(&mut self, ui: &mut egui::Ui) {
        let generation = self.context_generation.current();
        let mut actions = Vec::new();

        ui.separator();
        ui.strong("Workspace panels");
        ui.label("Workspace panels can render in native windows.");
        for panel in WORKSPACE_PANEL_IDS {
            let state =
                self.workspace_runtime
                    .state(panel, &self.panel_layout, &self.host_registry);
            let context = self
                .workspace_runtime
                .context_for(panel, &self.projects, generation);
            ui.horizontal(|ui| {
                ui.label(WorkspacePanelCoordinator::label(panel));
                match state {
                    Ok(WorkspacePanelState::Main {
                        placement,
                        native_host,
                        ..
                    }) if native_host.is_none() => {
                        ui.label(match placement {
                            PanelPlacement::Docked => "Docked",
                            PanelPlacement::FloatingInMainWindow => "Floating in main window",
                            PanelPlacement::PopOutWindow => "Pop-out requested",
                        });
                        if ui.button("Pop Out").clicked() {
                            actions.push(WorkspacePanelAction::PopOut(panel));
                        }
                    }
                    Ok(WorkspacePanelState::Main { .. }) => {
                        ui.label("Pop-out host requested");
                        if ui.button("Dock Back").clicked() {
                            actions.push(WorkspacePanelAction::DockBack(panel));
                        }
                    }
                    Err(error) => {
                        ui.label(format!("Unavailable: {error:?}"));
                    }
                }
                match context {
                    Some(context) => ui.label(format!("Project {}", context.project().raw())),
                    None => ui.label("No active project"),
                };
            });
        }
        for action in actions {
            let _ = self.apply_workspace_action(action);
        }
    }

    fn apply_workspace_action(
        &mut self,
        action: WorkspacePanelAction,
    ) -> Result<WorkspacePanelState, workspace_runtime::WorkspaceRuntimeError> {
        let host = match action {
            WorkspacePanelAction::DockBack(panel) => self
                .workspace_runtime
                .state(panel, &self.panel_layout, &self.host_registry)
                .ok()
                .and_then(WorkspacePanelState::host),
            WorkspacePanelAction::CloseNativeCopy { host, .. } => Some(host),
            WorkspacePanelAction::PopOut(_) => None,
        };
        let state = self.workspace_runtime.apply(
            action,
            &mut self.panel_layout,
            &mut self.host_registry,
        )?;
        match action {
            WorkspacePanelAction::PopOut(_) => {
                if let Some(host) = state.host() {
                    if !self.native_hosts.contains_key(&host) {
                        self.pending_native_hosts.insert(host);
                    }
                }
            }
            WorkspacePanelAction::DockBack(_) | WorkspacePanelAction::CloseNativeCopy { .. } => {
                if let Some(host) = host {
                    self.retire_native_host(host);
                }
            }
        }
        self.sync_workspace_layout();
        Ok(state)
    }

    fn retire_native_host(&mut self, host: host_registry::HostId) {
        self.pending_native_hosts.remove(&host);
        if let Some(native) = self.native_hosts.remove(&host) {
            if let Some(geometry) = native.geometry() {
                if let Some(record) = self
                    .workspace_layout
                    .panels
                    .iter_mut()
                    .find(|record| record.panel == native.panel())
                {
                    record.geometry = Some(geometry);
                }
            }
            self.native_host_retirement.enqueue(native);
        }
    }

    fn service_pending_native_hosts(&mut self, event_loop: &ActiveEventLoop) {
        let pending = std::mem::take(&mut self.pending_native_hosts);
        for host in pending {
            let Some(binding) = self.host_registry.resolve_host(host) else {
                continue;
            };
            if binding.window().is_some() || self.native_hosts.contains_key(&host) {
                continue;
            }
            let geometry = self
                .workspace_layout
                .panels
                .iter()
                .find(|record| record.panel == binding.panel())
                .and_then(|record| record.geometry);
            let native = match NativeWorkspaceHost::new(
                event_loop,
                NativeHostSpec {
                    panel: binding.panel(),
                    viewport: binding.viewport(),
                    geometry,
                },
            ) {
                Ok(native) => native,
                Err(error) => {
                    eprintln!(
                        "pyxross: native pop-out window creation failed: panel={:?} geometry={:?} error={error:?}",
                        binding.panel(),
                        geometry,
                    );
                    let _ = self.apply_workspace_action(WorkspacePanelAction::CloseNativeCopy {
                        panel: binding.panel(),
                        host,
                    });
                    continue;
                }
            };
            let window = native.window_id();
            if let Err(error) = self.host_registry.bind_window(host, window) {
                eprintln!(
                    "pyxross: native pop-out window bind failed: panel={:?} window={window:?} error={error:?}",
                    binding.panel(),
                );
                let _ = self.apply_workspace_action(WorkspacePanelAction::CloseNativeCopy {
                    panel: binding.panel(),
                    host,
                });
                continue;
            }
            native.request_redraw();
            self.native_hosts.insert(host, native);
        }
    }

    fn defer_workspace_action(&mut self, action: WorkspacePanelAction) {
        let panel = match action {
            WorkspacePanelAction::PopOut(panel) | WorkspacePanelAction::DockBack(panel) => panel,
            WorkspacePanelAction::CloseNativeCopy { panel, .. } => panel,
        };
        if self.deferred_workspace_actions.iter().any(|queued| {
            let queued_panel = match queued {
                WorkspacePanelAction::PopOut(queued_panel)
                | WorkspacePanelAction::DockBack(queued_panel) => *queued_panel,
                WorkspacePanelAction::CloseNativeCopy {
                    panel: queued_panel,
                    ..
                } => *queued_panel,
            };
            queued_panel == panel
                && match (queued, &action) {
                    (
                        WorkspacePanelAction::CloseNativeCopy {
                            host: queued_host, ..
                        },
                        WorkspacePanelAction::CloseNativeCopy { host, .. },
                    ) => queued_host == host,
                    (WorkspacePanelAction::CloseNativeCopy { .. }, _)
                    | (_, WorkspacePanelAction::CloseNativeCopy { .. }) => false,
                    _ => true,
                }
        }) {
            return;
        }
        self.deferred_workspace_actions.push(action);
    }

    fn apply_deferred_workspace_actions(&mut self) {
        for action in std::mem::take(&mut self.deferred_workspace_actions) {
            let _ = self.apply_workspace_action(action);
        }
    }

    fn workspace_panel_host_ui(
        &mut self,
        ui: &mut egui::Ui,
        panel: PanelId,
        host: host_registry::HostId,
        preview_texture: Option<egui::TextureId>,
    ) -> Option<WorkspacePanelAction> {
        let mut close = false;
        ui.push_id(panel, |ui| {
            ui.horizontal(|ui| {
                ui.strong(WorkspacePanelCoordinator::label(panel));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    close = ui.small_button("Close").clicked();
                });
            });
            let Some(project_id) = self.projects.active_id() else {
                return;
            };
            let Some(project) = self.projects.session(project_id) else {
                return;
            };
            match panel {
                PanelId::PanelA => {
                    ui.label("Panel A");
                    ui.label("Native workspace host");
                }
                PanelId::Preview => {
                    ui.label("Preview");
                    if let Some(texture) = preview_texture {
                        let available = ui.available_size();
                        let side = available.x.min(available.y).max(1.0);
                        ui.image((texture, egui::vec2(side, side)));
                    }
                }
                PanelId::Playback => {
                    ui.label(format!("Frame {}", project.animation.current_index() + 1));
                    ui.label(if project.animation.is_playing() {
                        "Playing"
                    } else {
                        "Stopped"
                    });
                    ui.label(if project.animation.looping() {
                        "Looping"
                    } else {
                        "Once"
                    });
                }
                PanelId::FrameEditor => {
                    ui.label(format!("Frame {}", project.animation.current_index() + 1));
                    if let Some(frame) = project.sequence.frame(project.animation.current_index()) {
                        let rect = frame.region().rect();
                        ui.label(format!(
                            "Region: {},{} {}×{}",
                            rect.x, rect.y, rect.w, rect.h
                        ));
                        ui.label(format!("Duration: {} ms", frame.delay_ms()));
                    }
                }
                PanelId::Layers
                | PanelId::ColorPalette
                | PanelId::TilePalette
                | PanelId::Animations
                | PanelId::Toolbox => {
                    ui.label("Workspace panel unavailable");
                }
            };
        });
        close.then_some(WorkspacePanelAction::CloseNativeCopy { panel, host })
    }

    fn redraw_native_host(&mut self, host: host_registry::HostId) {
        let Some(panel) = self.native_hosts.get(&host).map(NativeWorkspaceHost::panel) else {
            return;
        };
        let preview_image = (panel == PanelId::Preview && self.projects.active_id().is_some())
            .then(|| preview_image_for_session(self.projects.current()));
        let (ctx, raw_input, preview_texture) = {
            let native = self
                .native_hosts
                .get_mut(&host)
                .expect("native host exists");
            let preview_texture = preview_image.map(|image| native.set_preview_image(image));
            (native.context(), native.take_egui_input(), preview_texture)
        };
        let mut dock_back = None;
        let mut output = ctx.run_ui(raw_input, |ui| {
            egui::CentralPanel::default()
                .frame(panel_frame())
                .show(ui, |ui| {
                    dock_back = self.workspace_panel_host_ui(ui, panel, host, preview_texture);
                });
        });
        if let Some(action) = dock_back {
            output.textures_delta.clear();
            self.defer_workspace_action(action);
            return;
        }
        let paint_jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let Some(native) = self.native_hosts.get_mut(&host) else {
            return;
        };
        native.handle_platform_output(output.platform_output);
        let rendered = native.render_frame(
            &paint_jobs,
            &output.textures_delta,
            self.theme.current().colors.clear_color32(),
        );
        if native_render_succeeded(rendered) {
            native.request_redraw();
        }
        output.textures_delta.clear();
    }

    fn sync_workspace_layout(&mut self) {
        for record in &mut self.workspace_layout.panels {
            let Ok(panel) = self.panel_layout.registry().get(record.panel) else {
                continue;
            };
            record.placement = panel.placement();
            record.last_main_placement = panel.last_main_placement();
            record.desired_popout = panel.placement() == PanelPlacement::PopOutWindow;
        }
        for native in self.native_hosts.values() {
            if let Some(geometry) = native.geometry() {
                if let Some(record) = self
                    .workspace_layout
                    .panels
                    .iter_mut()
                    .find(|record| record.panel == native.panel())
                {
                    record.geometry = Some(geometry);
                }
            }
        }
    }

    fn save_workspace_layout(&self) {
        let Some(path) = workspace_layout_path() else {
            return;
        };
        let _ = self.workspace_layout.save_atomic(&path);
    }

    fn activate_project(
        &mut self,
        id: project::ProjectId,
    ) -> Result<project::ActivationOutcome, project::CloseError> {
        let outcome = self.projects.activate(id)?;
        self.consume_activation(outcome);
        Ok(outcome)
    }

    pub(crate) fn new_project(
        &mut self,
        name: impl Into<String>,
        width: usize,
        height: usize,
    ) -> project::ProjectId {
        let outcome = self.projects.new_project(name, width, height);
        self.consume_activation(outcome.activation());
        outcome.project()
    }

    pub(crate) fn close_project(
        &mut self,
        id: project::ProjectId,
        intent: project::CloseProject,
    ) -> Result<project::CloseOutcome, project::CloseError> {
        let can_close = self
            .projects
            .session(id)
            .is_some_and(|session| !session.is_dirty() || intent != project::CloseProject::Discard);
        if can_close && intent == project::CloseProject::Discard {
            self.clear_recovery_for(id);
        }
        let outcome = self.projects.close(id, intent)?;
        if let Some(activation) = outcome.activation() {
            self.consume_activation(activation);
        }
        Ok(outcome)
    }

    fn request_project_close(&mut self, id: project::ProjectId) {
        match self.close_project(id, project::CloseProject::Discard) {
            Ok(_) => {}
            Err(project::CloseError::Dirty { project })
                if self.projects.active_id() == Some(project) =>
            {
                self.projects.current_mut().pending_action = Some(PendingAction::Close { project });
            }
            Err(_) => {}
        }
    }

    /// Push document pixels into the canvas texture.
    ///
    /// Full `set()` on first upload, size change, or any layer-stack change;
    /// partial `set_partial()` uploads for the accumulated dirty rect
    /// otherwise.
    fn sync_texture(&mut self) {
        if self.projects.active_id().is_none() {
            return;
        }
        let active_token = self.projects.activation_token();
        if self.canvas_cache_token != active_token {
            self.invalidate_host_project_cache();
        }
        let Some(texture) = &mut self.canvas_texture else {
            return;
        };
        let (w, h) = (
            self.projects.current_mut().layers.width(),
            self.projects.current_mut().layers.height(),
        );
        let dirty = self
            .projects
            .current_mut()
            .layers
            .iter_mut()
            .filter_map(|layer| layer.buffer.take_pixels_changed())
            .reduce(|acc, region| acc.union(region));
        if self.texture_dirty
            || texture.size() != [w, h]
            || self.projects.current_mut().layers.changed()
        {
            let composite = self.projects.current_mut().layers.composite_layers();
            let image = egui::ColorImage::from_rgba_unmultiplied([w, h], composite.as_bytes());
            texture.set(image, egui::TextureOptions::NEAREST);
            self.texture_dirty = false;
            self.canvas_cache_token = active_token;
            self.projects.current_mut().layers.clear_changed();
            return;
        }
        if let Some(region) = dirty {
            // Recompositing the region across ALL layers keeps partial uploads
            // correct when layer visibility/opacity/blend vary (D44 identity:
            // a single NORMAL layer at opacity 1 is byte-identical to the old
            // `over()` path).
            if let Some(bytes) = self
                .projects
                .current_mut()
                .layers
                .composite_layers_region(region)
            {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [region.w as usize, region.h as usize],
                    bytes.as_bytes(),
                );
                texture.set_partial(
                    [region.x as usize, region.y as usize],
                    image,
                    egui::TextureOptions::NEAREST,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Toolbar + layer panel event application
    // -----------------------------------------------------------------------

    /// Apply toolbar gestures to the editing state (D62: deliberate switches
    /// only — every `ToolbarEvent` is deliberate).
    fn apply_toolbar_events(&mut self, events: Vec<ToolbarEvent>) {
        for event in events {
            match event {
                ToolbarEvent::ToolSelected(tool) => {
                    // A deliberate tool switch commits any in-flight transform
                    // session (Pyxel Edit: commit on deselect) but keeps the
                    // committed selection: the selection is a persistent region
                    // the user cancels explicitly (Esc, a click outside, or a
                    // new selection).
                    self.transform_session_commit();
                    self.projects.current_mut().tool_state.select_tool(tool);
                }
                ToolbarEvent::ColorChanged(color) => self.projects.current_mut().color = color,
                ToolbarEvent::SecondaryColorChanged(color) => {
                    self.projects.current_mut().secondary_color = color;
                }
                ToolbarEvent::BrushSizeChanged(size) => {
                    let session = self.projects.current_mut();
                    let tool = session.tool_state.tool();
                    session.draw_settings_mut(tool).size = size;
                }
                ToolbarEvent::BrushShapeChanged(shape) => {
                    let session = self.projects.current_mut();
                    let tool = session.tool_state.tool();
                    session.draw_settings_mut(tool).shape = shape;
                }
                ToolbarEvent::BrushScatterChanged(scatter) => {
                    let session = self.projects.current_mut();
                    let tool = session.tool_state.tool();
                    session.draw_settings_mut(tool).scatter = scatter;
                }
                ToolbarEvent::BrushScatterShapeChanged(shape) => {
                    let session = self.projects.current_mut();
                    let tool = session.tool_state.tool();
                    session.draw_settings_mut(tool).scatter_shape = shape;
                }
                ToolbarEvent::BrushTailChanged(tail) => {
                    let session = self.projects.current_mut();
                    let tool = session.tool_state.tool();
                    session.draw_settings_mut(tool).tail = tail;
                }
                ToolbarEvent::FieldierChildChanged(child) => {
                    // A deliberate child switch commits any in-flight transform
                    // session, exactly like `ToolSelected` (the child picker is
                    // a deliberate switch, D62/D75).
                    self.transform_session_commit();
                    self.projects.current_mut().tool_state.select_child(child);
                }
                ToolbarEvent::WandToleranceChanged(tolerance) => {
                    self.projects.current_mut().tool_state.wand_mut().tolerance = tolerance;
                }
                ToolbarEvent::WandRestrictToRegionChanged(restrict) => {
                    self.projects
                        .current_mut()
                        .tool_state
                        .wand_mut()
                        .restrict_to_region = restrict;
                }
                ToolbarEvent::TransformAlgorithmChanged(algorithm) => {
                    // Store the choice on the session so a later lift seeds
                    // from it, AND apply it to a live selection object so the
                    // current rotation/commit pipeline switches immediately.
                    let session = self.projects.current_mut();
                    session.transform_algorithm = algorithm;
                    if let Some(TransformSession::Selection(t)) = session.transform.as_mut() {
                        t.object.set_algorithm(algorithm);
                    }
                }
                ToolbarEvent::PaletteSelected(index) => {
                    if let Some(palette) = self.projects.current_mut().palettes.get(index) {
                        let next_color = palette.color(0);
                        let changed = self.projects.current_mut().active_palette != index
                            || next_color != Some(self.projects.current_mut().color);
                        self.projects.current_mut().active_palette = index;
                        if let Some(color) = next_color {
                            self.projects.current_mut().color = color;
                        }
                        if changed {
                            self.mark_dirty();
                        }
                    }
                }
                ToolbarEvent::SwapColors => {
                    let session = self.projects.current_mut();
                    std::mem::swap(&mut session.color, &mut session.secondary_color);
                }
            }
        }
    }

    /// Write the per-frame snapshots the dock panels render (Toolbox + Layers).
    ///
    /// The panels are pure views over typed cells; the App owns the model and
    /// publishes a fresh snapshot before the dock draws each frame.
    fn write_dock_snapshots(&mut self) {
        let session = self.projects.current();
        let active = session.layers.active_layer_id();
        let rows = layer_rows_for_dock(&session.layers, &self.layers_host.expanded_groups.borrow());
        let merge_enabled = merge_enabled_for(&session.layers, active);
        let delete_enabled = session.layers.children_of(None).len() > 1;
        *self.layers_host.view.borrow_mut() = LayerView {
            rows,
            active: Some(active),
            merge_enabled,
            delete_enabled,
        };
        let tool = session.tool_state.tool();
        let draw: DrawSettings = *session.draw_settings(tool);
        let child = session.tool_state.child();
        let wand = session.tool_state.wand();
        let wand_contiguous_effective = wand.contiguous && !self.ctx.input(|i| i.modifiers.alt);
        let transform_active = session.transform.is_some();
        let transform_algorithm = session.transform_algorithm;
        *self.toolbox_host.view.borrow_mut() = ToolboxView {
            tool,
            draw,
            child,
            wand,
            wand_contiguous_effective,
            transform_active,
            transform_algorithm,
        };
        let mut color_picker_view = self.color_picker_host.view.borrow_mut();
        if color_picker_view.open {
            if color_picker_view.color != session.color {
                // The live primary changed since the last snapshot. A preview
                // click targets the snapshot's Old or New color and must keep
                // `new_color` as the "back" target; every other change (a
                // picker edit, a palette pick, a swap) becomes the working
                // color the New preview restores.
                let from_preview = session.color == color_picker_view.old_color
                    || session.color == color_picker_view.new_color;
                if !from_preview {
                    color_picker_view.new_color = session.color;
                }
                color_picker_view.color = session.color;
            }
            color_picker_view.secondary_color = session.secondary_color;
        }
        let entries: Vec<Color> = session
            .palettes
            .get(session.active_palette)
            .map(|palette| palette.colors.iter().copied().map(Color::from).collect())
            .unwrap_or_default();
        {
            let mut selected = self.palette_host.selected.borrow_mut();
            if selected.is_some_and(|index| index >= entries.len()) {
                *selected = entries.len().checked_sub(1);
            }
        }
        *self.palette_host.view.borrow_mut() = PaletteView {
            entries,
            primary: session.color,
            secondary: session.secondary_color,
        };
    }

    /// Keep the per-layer settings floating panel in sync with the open layer:
    /// its title follows the layer name, and it is removed when the layer is
    /// gone (or when nothing is open).
    fn sync_layer_settings_panel(&mut self) {
        let settings_id = panel_dock::PanelId::new(dock_layers_panel::LAYER_SETTINGS_PANEL_ID);
        if self.panel_dock.placement(settings_id).is_none() {
            return;
        }
        let open = *self.layers_host.open_settings.borrow();
        let Some(id) = open else {
            self.panel_dock.remove_spec(settings_id);
            return;
        };
        let name = self
            .projects
            .current()
            .layers
            .layer(id)
            .map(|layer| layer.name.clone());
        let Some(name) = name else {
            self.panel_dock.remove_spec(settings_id);
            *self.layers_host.open_settings.borrow_mut() = None;
            self.layers_host.rename_buffer.borrow_mut().clear();
            return;
        };
        self.panel_dock
            .set_title(settings_id, format!("Layer: {name}"));
    }

    /// Applies the dock actions drained after a dock frame.
    fn handle_dock_actions(&mut self, actions: Vec<panel_dock::DockAction>) {
        for action in actions {
            match action {
                panel_dock::DockAction::ClosePanel(id) => self.close_panel(id),
                // Native pop-out is disabled until the feature is rebuilt.
                panel_dock::DockAction::PopOut(_) => {}
            }
        }
    }

    /// Closes a panel from its header's Close action: removes it from the dock
    /// and clears the host state of the panels that track their open flag.
    fn close_panel(&mut self, id: panel_dock::PanelId) {
        self.panel_dock.remove_spec(id);
        if id.raw() == dock_layers_panel::LAYER_SETTINGS_PANEL_ID {
            *self.layers_host.open_settings.borrow_mut() = None;
            self.layers_host.rename_buffer.borrow_mut().clear();
        }
        if id.raw() == dock_color_picker_panel::COLOR_PICKER_PANEL_ID {
            self.color_picker_host.view.borrow_mut().open = false;
        }
    }

    /// Opens or closes the floating Color Picker panel (C). Opening snapshots
    /// the current primary color as the panel's "Old" preview and reuses the
    /// rect the panel was last closed at.

    fn toggle_color_picker(&mut self) {
        let id = panel_dock::PanelId::new(dock_color_picker_panel::COLOR_PICKER_PANEL_ID);
        if self.panel_dock.placement(id).is_some() {
            self.close_panel(id);
            return;
        }
        let rect = self.panel_dock.last_floating_rect(id).unwrap_or_else(|| {
            egui::Rect::from_min_size(egui::pos2(120.0, 80.0), egui::vec2(540.0, 300.0))
        });
        let spec = dock_color_picker_panel::color_picker_panel_spec(&self.color_picker_host, rect);
        if self.panel_dock.add_spec(spec).is_ok() {
            let session = self.projects.current();
            let color = session.color;
            *self.color_picker_host.view.borrow_mut() = ColorPickerView {
                color,
                old_color: color,
                new_color: color,
                secondary_color: session.secondary_color,
                open: true,
            };
            self.panel_dock.raise_floating(id);
        }
    }

    /// Drain the dock panel events into the model/undo exactly once per frame.
    fn drain_dock_events(&mut self) {
        let toolbox_events: Vec<ToolbarEvent> =
            self.toolbox_host.events.borrow_mut().drain(..).collect();
        self.apply_toolbar_events(toolbox_events);
        let color_events: Vec<ToolbarEvent> = self
            .color_picker_host
            .events
            .borrow_mut()
            .drain(..)
            .collect();
        self.apply_toolbar_events(color_events);
        let layer_events: Vec<LayerPanelEvent> =
            self.layers_host.events.borrow_mut().drain(..).collect();
        self.apply_layer_events(layer_events);
        let palette_events: Vec<PalettePanelEvent> =
            self.palette_host.events.borrow_mut().drain(..).collect();
        self.apply_palette_events(palette_events);
    }

    /// Apply Palette panel gestures: color picks route through the toolbar
    /// events; add/remove edit the ACTIVE palette's colors in place.
    ///
    /// Palette edits are project state (they mark the project dirty), but they
    /// are deliberately not undoable: the shared undo stack is layer-scoped and
    /// the document palette has no command of its own.
    fn apply_palette_events(&mut self, events: Vec<PalettePanelEvent>) {
        for event in events {
            match event {
                PalettePanelEvent::Primary(color) => {
                    self.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(color)]);
                }
                PalettePanelEvent::Secondary(color) => {
                    self.apply_toolbar_events(vec![ToolbarEvent::SecondaryColorChanged(color)]);
                }
                PalettePanelEvent::Add => self.append_palette_entry(),
                PalettePanelEvent::Remove(index) => self.remove_palette_entry(index),
            }
        }
    }

    /// Append the current PRIMARY color to the active palette (Palette panel
    /// "Add"); the appended entry becomes the Remove target.
    fn append_palette_entry(&mut self) {
        let color = self.projects.current().color;
        let appended = {
            let session = self.projects.current_mut();
            let active = session.active_palette;
            let Some(palette) = session.palettes.get_mut(active) else {
                return;
            };
            palette.colors.push([color.r, color.g, color.b, color.a]);
            palette.colors.len() - 1
        };
        *self.palette_host.selected.borrow_mut() = Some(appended);
        self.mark_dirty();
    }

    /// Drop `index` from the active palette (Palette panel "Remove"); a palette
    /// with a single entry is left alone.
    fn remove_palette_entry(&mut self, index: usize) {
        let removed = {
            let session = self.projects.current_mut();
            let active = session.active_palette;
            let Some(palette) = session.palettes.get_mut(active) else {
                return;
            };
            if palette.colors.len() <= 1 || index >= palette.colors.len() {
                false
            } else {
                palette.colors.remove(index);
                true
            }
        };
        if removed {
            self.mark_dirty();
        }
    }

    fn persist_keymap(&mut self) {
        if let Some(path) = user_keymap_path() {
            if let Err(error) = save_keymap(&self.keymap, &path) {
                self.projects.current_mut().last_error = Some(error.to_string());
            }
        }
    }

    /// Apply layer-panel gestures as exactly one undoable command per gesture
    /// (D59). Structural changes and active-layer switches clear the selection
    /// (it is scoped to the active layer, D36).
    fn apply_layer_events(&mut self, events: Vec<LayerPanelEvent>) {
        let mut clear_selection = false;
        let session = self.projects.current_mut();
        let undo_len = session.undo.undo_len();
        {
            let mut controller = LayerStackController::new(&mut session.undo);
            for event in events {
                match event {
                    LayerPanelEvent::Select(id) => {
                        if session.layers.set_active(id) {
                            clear_selection = true;
                        }
                    }
                    LayerPanelEvent::Add => {
                        let name = format!("Layer {}", session.layers.len() + 1);
                        controller.add_layer(&mut session.layers, &name);
                        clear_selection = true;
                    }
                    LayerPanelEvent::Remove(id) => {
                        if controller.remove_layer(&mut session.layers, id) {
                            clear_selection = true;
                            // The settings popup must not dangle on a layer that
                            // no longer exists (a removed group takes its whole
                            // subtree with it).
                            let open = *self.layers_host.open_settings.borrow();
                            if let Some(open) = open {
                                if session.layers.layer(open).is_none() {
                                    *self.layers_host.open_settings.borrow_mut() = None;
                                }
                            }
                        }
                    }
                    LayerPanelEvent::ToggleVisible(id) => {
                        let visible = session.layers.layer(id).map(|l| l.visible).unwrap_or(true);
                        controller.set_visible(&mut session.layers, id, !visible);
                    }
                    LayerPanelEvent::Rename(id, name) => {
                        controller.set_name(&mut session.layers, id, name);
                    }
                    LayerPanelEvent::SetOpacity(id, opacity) => {
                        controller.set_opacity_coalesced(&mut session.layers, id, opacity);
                    }
                    LayerPanelEvent::SetBlend(id, blend) => {
                        controller.set_blend(&mut session.layers, id, blend);
                    }
                    LayerPanelEvent::MergeDown => {
                        let active = session.layers.active_layer_id();
                        if controller.merge_down(&mut session.layers, active) {
                            clear_selection = true;
                        }
                    }
                    LayerPanelEvent::CreateGroup => {
                        let active = session.layers.active_layer_id();
                        if controller
                            .create_group_around(&mut session.layers, &[active], "Group")
                            .is_some()
                        {
                            clear_selection = true;
                        }
                    }
                    LayerPanelEvent::Ungroup(id) => {
                        if controller.ungroup(&mut session.layers, id) {
                            clear_selection = true;
                        }
                    }
                    LayerPanelEvent::ReorderDropped { dragged, target } => {
                        // Flatten the CURRENT model into the view the dock
                        // would present (all groups expanded, row 0 = topmost)
                        // and resolve the placement with the pure
                        // drop_placement function.
                        let expanded: HashSet<LayerId> = session
                            .layers
                            .iter()
                            .filter(|layer| layer.is_group)
                            .map(|layer| layer.id)
                            .collect();
                        let rows = layer_rows_for_dock(&session.layers, &expanded);
                        let view = LayerView {
                            rows,
                            active: Some(session.layers.active_layer_id()),
                            merge_enabled: false,
                            delete_enabled: false,
                        };
                        let Some(placement) = drop_placement(dragged, target, &view) else {
                            continue;
                        };
                        let dragged_parent = session.layers.parent_of(dragged);
                        let target_parent = session.layers.parent_of(target);
                        let target_child_index = session
                            .layers
                            .children_of(target_parent)
                            .iter()
                            .position(|&id| id == target)
                            .expect("target is in the view, so it has a child index");
                        let dragged_child_index = session
                            .layers
                            .children_of(dragged_parent)
                            .iter()
                            .position(|&id| id == dragged)
                            .expect("dragged is in the view, so it has a child index");
                        // Child indices are in "after removing the dragged"
                        // coordinates: the target's index shifts down by one
                        // when the dragged was an earlier sibling of it.
                        let target_child_index_after_removal = target_child_index
                            - usize::from(
                                dragged_parent == target_parent
                                    && dragged_child_index < target_child_index,
                            );
                        let (new_parent, index) = match placement {
                            DropPlacement::Above => {
                                (target_parent, target_child_index_after_removal + 1)
                            }
                            DropPlacement::Below => {
                                (target_parent, target_child_index_after_removal)
                            }
                            DropPlacement::IntoGroup => {
                                let child_count = session.layers.children_of(Some(target)).len();
                                let after_removal =
                                    child_count - usize::from(dragged_parent == Some(target));
                                (Some(target), after_removal)
                            }
                        };
                        // No-op guard: same parent at the same child index
                        // means nothing moves — skip so no undo step is pushed.
                        if (new_parent != dragged_parent || index != dragged_child_index)
                            && controller.move_subtree(
                                &mut session.layers,
                                dragged,
                                new_parent,
                                index,
                            )
                        {
                            clear_selection = true;
                        }
                    }
                    // Host-cell state the panels toggle; not model-backed.
                    LayerPanelEvent::SettingsToggle(id) => {
                        let settings_id =
                            panel_dock::PanelId::new(dock_layers_panel::LAYER_SETTINGS_PANEL_ID);
                        let registered = self.panel_dock.placement(settings_id).is_some();
                        let open = *self.layers_host.open_settings.borrow();
                        if !registered {
                            // Open: register the floating settings panel for
                            // this layer and raise it above the dock chrome.
                            let spec = dock_layers_panel::layer_settings_panel_spec(
                                &self.layers_host,
                                egui::Rect::from_min_size(
                                    egui::pos2(360.0, 140.0),
                                    egui::vec2(280.0, 200.0),
                                ),
                            );
                            if self.panel_dock.add_spec(spec).is_ok() {
                                *self.layers_host.open_settings.borrow_mut() = Some(id);
                                if let Some(name) =
                                    session.layers.layer(id).map(|layer| layer.name.clone())
                                {
                                    *self.layers_host.rename_buffer.borrow_mut() = name;
                                }
                                self.panel_dock.raise_floating(settings_id);
                            }
                        } else if open == Some(id) {
                            // Close: remove the panel and clear the open layer.
                            self.panel_dock.remove_spec(settings_id);
                            *self.layers_host.open_settings.borrow_mut() = None;
                            self.layers_host.rename_buffer.borrow_mut().clear();
                        } else {
                            // Switch: retarget the existing panel at the new layer.
                            *self.layers_host.open_settings.borrow_mut() = Some(id);
                            if let Some(name) =
                                session.layers.layer(id).map(|layer| layer.name.clone())
                            {
                                *self.layers_host.rename_buffer.borrow_mut() = name.clone();
                                self.panel_dock
                                    .set_title(settings_id, format!("Layer: {name}"));
                            }
                            self.panel_dock.raise_floating(settings_id);
                        }
                    }
                    LayerPanelEvent::ToggleExpanded(id) => {
                        let mut expanded = self.layers_host.expanded_groups.borrow_mut();
                        if !expanded.remove(&id) {
                            expanded.insert(id);
                        }
                    }
                }
            }
        }
        if session.undo.undo_len() > undo_len {
            session.mark_dirty();
        }
        if clear_selection {
            self.clear_selection();
        }
    }

    // -----------------------------------------------------------------------
    // Selection state machine (Select tool, D50)
    // -----------------------------------------------------------------------

    /// Drop all selection state (marquee, move, committed selection) and cancel
    /// any live transform session. Part C: a lift CUT the selected pixels, so
    /// the cancel path writes them back byte-exactly (mask-aware) before
    /// dropping the session — the document is left as if the lift never
    /// happened. Pushes no undo entry.
    fn clear_selection(&mut self) {
        if let Some(TransformSession::Selection(t)) = self.projects.current_mut().transform.take() {
            restore_transform_source(&mut self.projects.current_mut().layers, &t.source);
        }
        let session = self.projects.current_mut();
        session.selection = None;
        session.gesture = SelectGesture::Idle;
        session.transform = None;
    }

    /// Deselect: commit any live floating transform (one undo step) then drop
    /// the selection.  Used by a click outside the selection.  A tool switch
    /// keeps the selection; Esc cancels a live selection transform (restoring
    /// the lifted pixels) before clearing it, and starting a new selection
    /// commits one too.
    fn deselect(&mut self) {
        self.transform_session_commit();
        self.clear_selection();
    }

    /// The current selection's marching-ants overlay.
    ///
    /// Every committed selection is drawn the same way (D76): the mask
    /// marching-ants over `Selection::outline_segments`, so a rectangle and a
    /// T-shape never look like two different features. The rectangular form has
    /// no per-cell storage, but `outline_segments` is O(boundary) and produces
    /// the same four edges, so there is no separate rectangular appearance.
    fn selection_overlay(selection: &Selection) -> CanvasOverlay {
        CanvasOverlay::AntsMask(selection.outline_segments())
    }

    /// The combining mode chosen at PRESS time (D78): the right button
    /// subtracts; otherwise Shift adds; otherwise an Alt region sweep adds when
    /// a selection already exists (otherwise it replaces, so the first sweep
    /// becomes the selection — D77 item 4); a plain left press replaces. Alt is
    /// deliberately NOT Subtract for the selection tools (D76): it drives the
    /// grid-cell region gesture instead, and the Wand keeps its own Alt
    /// contiguity override. The result is stored on the gesture so the release
    /// never re-derives it from a frame whose button state may be missing.
    fn select_mode(&self, right_button: bool, region: bool) -> SelectMode {
        if right_button {
            return SelectMode::Subtract;
        }
        if self.ctx.input(|i| i.modifiers.shift) {
            return SelectMode::Add;
        }
        if region && self.projects.current().selection.is_some() {
            return SelectMode::Add;
        }
        SelectMode::Replace
    }

    /// Combine a freshly captured selection with `base` (or, when the gesture
    /// captured none, the current selection) per `mode`.  `Replace` installs
    /// the new selection outright; the combining modes (`Add` / `Subtract` /
    /// `Intersect`) fold it into the existing selection and are a no-op when
    /// there is nothing to combine with (D78) — a base-less subtract must
    /// never fabricate a selection.
    fn combine_selection(
        &mut self,
        new: Option<Selection>,
        mode: SelectMode,
        base: Option<Selection>,
    ) {
        let Some(new) = new else {
            if mode == SelectMode::Replace {
                self.projects.current_mut().selection = None;
            }
            return;
        };
        let base = base.or_else(|| self.projects.current_mut().selection.take());
        let Some(current) = base else {
            if mode == SelectMode::Replace {
                self.projects.current_mut().selection = Some(new);
            }
            return;
        };
        let shape = match mode {
            SelectMode::Replace => Some(new.to_shape()),
            SelectMode::Add => current.union_shape(&new),
            SelectMode::Subtract => current.subtract_shape(&new),
            SelectMode::Intersect => current.intersect_shape(&new),
        };
        let Some((bbox, mask)) = shape else {
            self.projects.current_mut().selection = None;
            return;
        };
        let buf = &self.projects.current_mut().layers.active_layer().buffer;
        self.projects.current_mut().selection = Selection::capture_mask(buf, bbox, mask);
    }

    /// The dynamic-curve gizmo under the pointer this frame, if any. The hit
    /// radius is kept constant on screen by dividing by the camera scale.
    fn curve_hover_at(
        &self,
        ctx: &egui::Context,
        camera: crate::core::camera::Camera,
        canvas_size: (u32, u32),
    ) -> Option<usize> {
        let curve = self.projects.current().transform.as_ref()?.curve()?;
        let pos = ctx.input(|i| i.pointer.hover_pos())?;
        let pt = self
            .canvas_widget
            .screen_to_canvas(pos, camera, canvas_size)?;
        let radius = (GIZMO_HIT_RADIUS / camera.scale() as f32).max(1.0);
        curve.hit_gizmo((pt.0 as f32, pt.1 as f32), radius)
    }

    /// The visible canvas pixel under the pointer this frame, or `None` when
    /// the pointer is off-canvas. The live marquee outline inverts it per
    /// channel so the box stays legible on artwork of any brightness (D76).
    fn marquee_probe_at(
        &self,
        ctx: &egui::Context,
        camera: crate::core::camera::Camera,
        canvas_size: (u32, u32),
    ) -> Option<Color> {
        let pos = ctx.input(|i| i.pointer.latest_pos())?;
        let (x, y) = self
            .canvas_widget
            .screen_to_canvas(pos, camera, canvas_size)?;
        self.projects
            .current()
            .layers
            .active_layer()
            .buffer
            .get_pixel(x as usize, y as usize)
    }

    /// The overlay to paint on the canvas this frame: the transform session's
    /// gizmo + floating pixels first, then the live move preview, then the live
    /// marquee, then the committed selection.
    fn current_overlay(&self) -> CanvasOverlay {
        if let Some(t) = &self.projects.current().transform {
            match t {
                TransformSession::Curve(curve) => {
                    return CanvasOverlay::Curve {
                        points: curve.curve.polyline(),
                        samples: curve.curve.samples(),
                        gizmos: curve.curve.gizmos(),
                        hovered: self.curve_hovered,
                    };
                }
                TransformSession::Selection(selection) => {
                    let (min_x, min_y, max_x, max_y) = selection.object.canvas_bbox();
                    // O1: the gizmo is the object's rotated quad, so outline,
                    // handles and rotation hints rotate with the content.
                    let corners = selection.object.canvas_corners();
                    let preview = self.transform_preview.as_ref().map(|tex| {
                        (
                            tex.id(),
                            Rect2i::new(
                                min_x.round() as i32,
                                min_y.round() as i32,
                                (max_x - min_x).round() as i32,
                                (max_y - min_y).round() as i32,
                            ),
                        )
                    });
                    // While a handle is grabbed, surface it as `active` (for a
                    // future J1 `paint` highlight) and show the drag HUD.
                    let active = (selection.drag != GizmoHit::None).then_some(selection.drag);
                    let hud = active.map(|_| {
                        let (w, h) = selection.object.bounds_size();
                        HudState {
                            size: (w, h),
                            angle_deg: selection.object.angle_deg,
                            pos: (min_x.round() as i32, min_y.round() as i32),
                            scale: selection.object.scale_xy(),
                        }
                    });
                    return CanvasOverlay::Gizmo {
                        corners,
                        pivot: selection.object.pivot(),
                        hovered: self.gizmo_hovered,
                        active,
                        preview,
                        hud,
                    };
                }
            }
        }
        match &self.projects.current().gesture {
            SelectGesture::AreaMove { dest, preview, .. } => {
                return match preview {
                    Some(segments) => CanvasOverlay::AntsMask(segments.clone()),
                    None => CanvasOverlay::Marquee(*dest),
                };
            }
            SelectGesture::Marquee { rect, base, .. } => {
                return match base {
                    Some(base) => CanvasOverlay::RegionCells {
                        cells: vec![*rect],
                        base: base.outline_segments(),
                    },
                    None => CanvasOverlay::Marquee(*rect),
                };
            }
            SelectGesture::Lasso { points, .. } => return CanvasOverlay::Lasso(points.clone()),
            SelectGesture::TileBorder { cells, base, .. } => {
                return CanvasOverlay::RegionCells {
                    cells: cells.clone(),
                    base: base
                        .as_ref()
                        .map(Selection::outline_segments)
                        .unwrap_or_default(),
                };
            }
            SelectGesture::Idle => {}
        }
        if let Some(sel) = &self.projects.current().selection {
            return Self::selection_overlay(sel);
        }
        CanvasOverlay::None
    }

    /// Select-tool press: Ctrl+LMB inside the committed selection lifts it
    /// into a floating transform session as a cut (D32; Alt no longer lifts a
    /// copy — D76); a plain LMB press inside begins a move of the selection
    /// AREA only (D50/D70/D77 — the move never touches pixels); otherwise drop
    /// the selection and begin a gesture anchored at `pt`.
    ///
    /// The right button never clears or starts a selection (D77 item 3): with
    /// nothing selected it is a no-op, and with a selection it always begins a
    /// Subtract gesture — even when the press lands inside the selection. An
    /// Alt+left region press begins an Add sweep (D77 item 4).
    fn select_press(&mut self, pt: (i32, i32), right_button: bool) {
        // The right button with no selection must not clear, create, or start
        // anything (D77 item 3).
        if right_button && self.projects.current().selection.is_none() {
            return;
        }
        let ctrl = self.ctx.input(|i| i.modifiers.command);
        let alt = self.ctx.input(|i| i.modifiers.alt);
        let child = self.projects.current().tool_state.child();
        // Alt+left on the Rectangle child is the grid-cell region sweep, so it
        // takes precedence over the inside-hit area move (D77 item 4).
        let region = child == FieldierChild::Rectangle && alt;
        if !right_button && !region {
            let hit = self
                .projects
                .current()
                .selection
                .as_ref()
                .filter(|sel| sel.contains(pt.0, pt.1))
                .map(Selection::rect);
            if let Some(dest) = hit {
                if ctrl {
                    self.lift_transform_at(pt, true);
                } else {
                    let preview = self
                        .projects
                        .current()
                        .selection
                        .as_ref()
                        .filter(|sel| !sel.is_rectangular())
                        .map(Selection::outline_segments);
                    self.projects.current_mut().gesture = SelectGesture::AreaMove {
                        origin: pt,
                        dest,
                        preview,
                    };
                }
                return;
            }
        }
        // Starting a new selection commits any floating transform first.
        self.transform_session_commit();
        // The wand is click-driven and starts no press gesture.
        if child == FieldierChild::Wand {
            return;
        }
        // The combining mode is chosen ONCE here and carried on the gesture
        // (D78): the canvas reports the secondary release with
        // `eyedropper_point == None`, so re-deriving the mode at release would
        // fall back to Replace and turn a right-drag subtract into an add.
        let mode = self.select_mode(right_button, region);
        // A combining gesture keeps the pre-gesture selection in place so its
        // marching ants stay visible while the new area is swept; `base` is a
        // clone for the release merge (D77 item 2). A Replace gesture drops the
        // old selection outright.
        let tile_size = self.projects.current().tile_size;
        let session = self.projects.current_mut();
        let base = if mode != SelectMode::Replace {
            session.selection.clone()
        } else {
            session.selection = None;
            None
        };
        session.gesture = match child {
            FieldierChild::Lasso => SelectGesture::Lasso {
                points: vec![pt],
                base,
                mode,
            },
            FieldierChild::Rectangle if region => SelectGesture::TileBorder {
                origin: pt,
                current: pt,
                cells: crate::core::selection_ops::tile_cells_on_segment(pt, pt, tile_size),
                base,
                mode,
            },
            _ => SelectGesture::Marquee {
                origin: pt,
                rect: Rect2i::new(pt.0, pt.1, 1, 1),
                base,
                mode,
            },
        };
    }

    /// Select-tool drag: update the move preview destination or the marquee.
    fn select_drag(&mut self, pt: (i32, i32)) {
        let session = self.projects.current_mut();
        let tile_size = session.tile_size;
        let next = match std::mem::replace(&mut session.gesture, SelectGesture::Idle) {
            SelectGesture::AreaMove { origin, dest, .. } => match &session.selection {
                Some(sel) => {
                    let next_dest = sel.destination(pt.0 - origin.0, pt.1 - origin.1);
                    let preview = (!sel.is_rectangular()).then(|| {
                        let dx = next_dest.x - sel.rect().x;
                        let dy = next_dest.y - sel.rect().y;
                        sel.translated_shape(dx, dy)
                            .and_then(|(bbox, mask)| {
                                Selection::capture_mask(
                                    &session.layers.active_layer().buffer,
                                    bbox,
                                    mask,
                                )
                            })
                            .map(|moved| moved.outline_segments())
                            .unwrap_or_default()
                    });
                    SelectGesture::AreaMove {
                        origin,
                        dest: next_dest,
                        preview,
                    }
                }
                None => SelectGesture::AreaMove {
                    origin,
                    dest,
                    preview: None,
                },
            },
            SelectGesture::Marquee {
                origin,
                rect: _,
                base,
                mode,
            } => SelectGesture::Marquee {
                origin,
                rect: rect_from_points(origin, pt),
                base,
                mode,
            },
            SelectGesture::Lasso {
                mut points,
                base,
                mode,
            } => {
                if points.last() != Some(&pt) {
                    points.push(pt);
                }
                SelectGesture::Lasso { points, base, mode }
            }
            SelectGesture::TileBorder {
                origin,
                current,
                mut cells,
                base,
                mode,
            } => {
                // Extend the sweep by the new segment only: every grid cell the
                // pointer crossed since the last sample joins the accumulated,
                // path-ordered set (D77 item 4).
                for cell in
                    crate::core::selection_ops::tile_cells_on_segment(current, pt, tile_size)
                {
                    if !cells.contains(&cell) {
                        cells.push(cell);
                    }
                }
                SelectGesture::TileBorder {
                    origin,
                    current: pt,
                    cells,
                    base,
                    mode,
                }
            }
            other => other,
        };
        session.gesture = next;
    }

    /// Select-tool release: the area move translates ONLY the selection — the
    /// mask shifts with it, but no pixels are cut or pasted and no undo step
    /// is pushed (D77 re-affirms D50/D70 and supersedes D76 item 2). Every
    /// other gesture captures its swept shape and combines it per the mode
    /// locked at press (D78), never re-derived from the release frame: Shift
    /// adds, the right button subtracts, and the Alt region sweep adds on the
    /// left button and subtracts on the right (D77 item 4).
    fn select_release(&mut self) {
        let gesture = std::mem::replace(
            &mut self.projects.current_mut().gesture,
            SelectGesture::Idle,
        );
        match gesture {
            SelectGesture::AreaMove { dest, .. } => {
                let session = self.projects.current_mut();
                let Some(sel) = session.selection.take() else {
                    return;
                };
                let dx = dest.x - sel.rect().x;
                let dy = dest.y - sel.rect().y;
                let buf = &session.layers.active_layer().buffer;
                session.selection = sel.translated_to(dx, dy, buf);
            }
            SelectGesture::Marquee {
                origin,
                rect,
                base,
                mode,
            } => {
                let rect = match rect.is_empty() {
                    true => Rect2i::new(origin.0, origin.1, 1, 1),
                    false => rect,
                };
                let captured =
                    Selection::capture(&self.projects.current().layers.active_layer().buffer, rect);
                self.combine_selection(captured, mode, base);
            }
            SelectGesture::Lasso { points, base, mode } => {
                let captured = {
                    let buf = &self.projects.current().layers.active_layer().buffer;
                    crate::core::selection_ops::lasso_shape(buf, &points)
                        .and_then(|(bbox, mask)| Selection::capture_mask(buf, bbox, mask))
                };
                self.combine_selection(captured, mode, base);
            }
            SelectGesture::TileBorder {
                cells, base, mode, ..
            } => {
                let captured = {
                    let buf = &self.projects.current().layers.active_layer().buffer;
                    crate::core::selection_ops::tile_region_shape(buf, &cells)
                        .and_then(|(bbox, mask)| Selection::capture_mask(buf, bbox, mask))
                };
                self.combine_selection(captured, mode, base);
            }
            SelectGesture::Idle => {}
        }
    }

    /// Select-tool interactions: marquee/move drags plus click-to-dismiss.
    fn handle_select_interactions(&mut self, interactions: CanvasInteractions) {
        if self.projects.current().tool_state.child() == FieldierChild::Wand {
            self.handle_wand_interactions(interactions);
            return;
        }
        let right_button =
            interactions.eyedropper_started || interactions.eyedropper_point.is_some();
        // The canvas widget raises `stroke_started` on the first drag delta, so
        // it only means "press" while no gesture is live; otherwise it is this
        // same drag continuing. `stroke_segment_started` is the App's own
        // deliberate re-entry signal and always counts.
        let press = interactions.stroke_segment_started
            || interactions.eyedropper_started
            || (interactions.stroke_started && self.projects.current().gesture.is_idle());
        if press {
            if let Some(pt) = interactions.stroke_point.or(interactions.eyedropper_point) {
                self.select_press(pt, right_button);
            }
        } else if let Some(pt) = interactions.stroke_point.or(interactions.eyedropper_point) {
            self.select_drag(pt);
        }
        if interactions.stroke_ended || interactions.eyedropper_ended {
            self.select_release();
        }
        // A plain click (no drag): dismiss the selection when it lands outside.
        // A Ctrl click INSIDE the selection lifts it into a transform session
        // instead: `clicked()` only fires on the release frame, so a click that
        // never moved never reached `select_press` above. Alt no longer lifts
        // (D76).
        if let Some(pt) = interactions.clicked {
            let lifting = self.ctx.input(|i| i.modifiers.command)
                && self
                    .projects
                    .current()
                    .selection
                    .as_ref()
                    .is_some_and(|sel| sel.contains(pt.0, pt.1));
            if lifting {
                self.select_press(pt, right_button);
                return;
            }
            let outside = self
                .projects
                .current()
                .selection
                .as_ref()
                .is_some_and(|sel| !sel.contains(pt.0, pt.1));
            if outside {
                self.deselect();
            }
        }
        // A double-click outside the selection dismisses it too, so a second
        // click on empty canvas clears without reaching for Esc.
        if let Some(pt) = interactions.double_clicked {
            let outside = self
                .projects
                .current()
                .selection
                .as_ref()
                .is_some_and(|sel| !sel.contains(pt.0, pt.1));
            if outside {
                self.deselect();
            }
        }
    }

    /// Magic-Wand interactions: a primary click selects the seed's matching
    /// region, a secondary press subtracts it, and a double-click selects its
    /// opaque alpha component. The Wand is click-driven (D78): its shape is
    /// applied exactly once per definite click event — a primary `clicked` with
    /// `shift ? Add : Replace`, a secondary `eyedropper_started` with
    /// `Subtract` — and never re-fires on the `eyedropper_point`-only drag
    /// frames in between. A double-click (primary only in the canvas event
    /// data) combines its alpha-connected component with the modifiers too, so
    /// Shift+double-click unions with the existing selection instead of wiping
    /// it. A secondary double-click is indistinguishable from two secondary
    /// presses there, so it falls back to the single-click subtract path.
    fn handle_wand_interactions(&mut self, interactions: CanvasInteractions) {
        // The pre-gesture selection is the explicit merge base, so Add/Subtract
        // never depend on `combine_selection` taking the live selection (D78).
        let base = self.projects.current().selection.clone();
        if let Some(seed) = interactions.double_clicked {
            let captured = {
                let buf = &self.projects.current().layers.active_layer().buffer;
                crate::core::selection_ops::alpha_neighbors_shape(buf, seed)
                    .and_then(|(bbox, mask)| Selection::capture_mask(buf, bbox, mask))
            };
            let mode = if self.ctx.input(|i| i.modifiers.shift) {
                SelectMode::Add
            } else {
                SelectMode::Replace
            };
            self.combine_selection(captured, mode, base);
            return;
        }
        // A primary click arrives as `clicked`; a secondary press arrives as
        // `eyedropper_started` with its seed point. Only these definite events
        // apply the shape; `eyedropper_point` alone marks a drag frame.
        let (seed, mode) = if interactions.eyedropper_started {
            (interactions.eyedropper_point, SelectMode::Subtract)
        } else if let Some(seed) = interactions.clicked {
            let mode = if self.ctx.input(|i| i.modifiers.shift) {
                SelectMode::Add
            } else {
                SelectMode::Replace
            };
            (Some(seed), mode)
        } else {
            return;
        };
        let Some(seed) = seed else {
            return;
        };
        let wand = self.projects.current().tool_state.wand();
        let alt_held = self.ctx.input(|i| i.modifiers.alt);
        let effective_contiguous = wand.contiguous && !alt_held;
        let tile = self.projects.current().tile_size.max(1) as i32;
        let seed_cell = Rect2i::new(
            seed.0.div_euclid(tile) * tile,
            seed.1.div_euclid(tile) * tile,
            tile,
            tile,
        );
        let clip = wand
            .restrict_to_region
            .then(|| PixelClip::from_rect(seed_cell));
        let captured = {
            let buf = &self.projects.current().layers.active_layer().buffer;
            crate::core::selection_ops::magic_wand_shape(
                buf,
                seed,
                wand.tolerance,
                effective_contiguous,
                clip.as_ref(),
            )
            .and_then(|(bbox, mask)| Selection::capture_mask(buf, bbox, mask))
        };
        self.combine_selection(captured, mode, base);
    }

    // -----------------------------------------------------------------------
    // Transform session (D32/D35/D36)
    // -----------------------------------------------------------------------

    /// Lift the committed selection into a floating transform session (D32):
    /// snapshot the source rect, build a [`TransformObject`] from the buffer
    /// bytes, and drop the selection — the pixels now float over the canvas.
    /// The source is NOT cut until commit, so a cancel loses nothing.
    fn lift_transform(&mut self, rect: Rect2i) {
        self.lift_transform_with(rect, true);
    }

    /// Lift the committed selection at `pt` into a floating transform session
    /// as a cut.  `cut_source` is retained for the transform-session contract
    /// but the Fieldier only ever lifts a cut now (D76 removed the Alt copy
    /// lift).
    fn lift_transform_at(&mut self, pt: (i32, i32), cut_source: bool) {
        let Some(rect) = self
            .projects
            .current()
            .selection
            .as_ref()
            .map(Selection::rect)
        else {
            return;
        };
        let _ = pt;
        self.lift_transform_with(rect, cut_source);
    }

    /// Shared lift path: snapshot `rect`, build the floating object, and drop
    /// the selection.  `cut_source` selects move (true) vs copy (false).  A
    /// masked selection is exported with zeros outside the mask and carries
    /// its mask into the session source, so the commit only cuts and pastes
    /// the selected pixels.
    ///
    /// Part C lifecycle: for a REAL selection lift (`cut_source`) the selected
    /// pixels are CUT from the source layer IMMEDIATELY so the hole is visible
    /// at once. The snapshot + mask are kept on the session so Esc restores
    /// them byte-exactly and the commit can transiently restore them before
    /// running its single composite cut+paste step. NO undo entry is pushed at
    /// lift time.
    fn lift_transform_with(&mut self, rect: Rect2i, cut_source: bool) {
        let selection = self.projects.current().selection.as_ref();
        self.lifted_selection_is_masked = selection.is_some_and(|sel| !sel.is_rectangular());
        let Some(bytes) = self
            .projects
            .current()
            .layers
            .active_layer()
            .buffer
            .export_region(rect, selection)
        else {
            return;
        };
        // Owned copy of the selection's per-cell flags (rectangular selections
        // and no selection carry none); the immutable borrow ends here.
        let mask = selection.and_then(|sel| sel.mask().map(<[bool]>::to_vec));
        let buffer = LayerBuffer {
            layer_id: 0,
            w: rect.w as usize,
            h: rect.h as usize,
            buf: bytes.clone(),
        };
        let mut object = TransformObject::lift(buffer, (rect.x as f32, rect.y as f32));
        // Seed the fresh object from the session's stored algorithm so a
        // lifted transform honours the Tool Property selector.
        object.set_algorithm(self.projects.current().transform_algorithm);
        let start_local_dims = object.local_scaled_dims();
        let start_pivot = object.pivot();
        let source = TransformSource {
            layer_id: self.projects.current_mut().layers.active_layer_id(),
            rect,
            snapshot: bytes.clone(),
            mask,
        };
        // Part C: cut the lifted pixels out of the source layer now (no undo).
        // A masked lift clears only the mask-true cells; a rectangular lift
        // clears the whole rect.
        if cut_source && !bytes.is_empty() {
            let layer = self.projects.current_mut().layers.active_layer_mut();
            match &source.mask {
                Some(mask) => {
                    let zeros = vec![0u8; bytes.len()];
                    layer.buffer.blit_region_masked(rect, &zeros, Some(mask));
                }
                None => {
                    let zeros = vec![0u8; bytes.len()];
                    layer.buffer.blit_region(rect, &zeros);
                }
            }
        }
        self.projects.current_mut().transform =
            Some(TransformSession::Selection(SelectionTransform {
                source,
                object,
                drag: GizmoHit::None,
                last_pt: (rect.x as f32, rect.y as f32),
                cut_source,
                start_angle: 0.0,
                start_pointer_angle: 0.0,
                start_drag: GizmoHit::None,
                start_pointer: (rect.x as f32, rect.y as f32),
                start_pos: (rect.x as f32, rect.y as f32),
                start_local_dims,
                start_anchor: (rect.x as f32, rect.y as f32),
                start_anchor_end: (AnchorEnd::Center, AnchorEnd::Center),
                start_pivot,
                start_flip_h: false,
                start_flip_v: false,
                start_alt: false,
                move_axis: None,
                drag_initialized: false,
                preview_dragging: false,
            }));
        self.projects.current_mut().selection = None;
        self.projects.current_mut().gesture = SelectGesture::Idle;
    }

    /// Transform-session pointer routing (D32): stroke drags drive the floating
    /// object; a release or a plain click only ENDS the drag. The session
    /// commits on a double-click on empty space (or a deliberate tool switch /
    /// deselect); Esc cancels and restores it. No other interaction reaches
    /// the tools mid-session.
    fn handle_transform_interactions(&mut self, interactions: CanvasInteractions) {
        if self
            .projects
            .current()
            .transform
            .as_ref()
            .is_some_and(|t| matches!(t, TransformSession::Curve(_)))
        {
            self.handle_curve_transform_interactions(interactions);
            return;
        }
        if interactions.stroke_started || interactions.stroke_segment_started {
            if let Some(pt) = interactions.stroke_point {
                // Latch the handle only on the initial press. egui re-fires a
                // drag start (and the tool machine re-fires a segment start on
                // canvas re-entry) AFTER the pointer has already moved; an
                // already-latched grab must not be clobbered by the current
                // hover, nor have `last_pt` reset to the moved point. Instead
                // those re-fires continue the in-flight drag.
                let already_grabbed = self.transform_session_begin_drag(pt);
                if already_grabbed {
                    self.transform_session_drag(pt);
                }
            }
        } else if let Some(pt) = interactions.stroke_point {
            self.transform_session_drag(pt);
        }
        if interactions.stroke_ended || interactions.clicked.is_some() {
            if let Some(t) = self
                .projects
                .current_mut()
                .transform
                .as_mut()
                .and_then(TransformSession::selection_mut)
            {
                t.drag = GizmoHit::None;
                // Y2-B: the drag is over — the next preview re-renders with the
                // selected algorithm (the key includes `preview_dragging`).
                t.preview_dragging = false;
            }
        }
        if let Some(pt) = interactions.double_clicked {
            self.selection_transform_double_click(pt);
        } else {
            // The canvas widget only reports double-clicks inside its draw
            // rect. A double-click in the letterbox / empty area around the
            // canvas must still commit the floating selection.
            self.commit_transform_from_global_double_click();
        }
    }

    /// Global fallback for the transform commit gesture: a primary double-click
    /// that the canvas widget did NOT already report (it landed outside the
    /// canvas draw rect) still commits the floating selection. The point is
    /// mapped with the CLAMP-FREE mapping so an outside-canvas click stays
    /// outside the object's bbox and commits.
    ///
    /// Guards: only reached for a `TransformSession::Selection` (the curve
    /// variant is routed to `handle_curve_transform_interactions`), and skipped
    /// while egui is actively using the pointer for a drag
    /// ([`egui::Context::egui_is_using_pointer`]) so a drag on another widget
    /// cannot be hijacked. It deliberately does NOT gate on
    /// `egui_wants_pointer_input` / pointer-over-egui: every point inside the
    /// window is "over egui", which would disable the required letterbox /
    /// empty-area commit.
    fn commit_transform_from_global_double_click(&mut self) {
        if self.ctx.egui_is_using_pointer() {
            return;
        }
        let Some(pos) = self.ctx.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
                .then(|| i.pointer.latest_pos())
                .flatten()
        }) else {
            return;
        };
        let camera = self.projects.current().camera;
        let canvas_size = (
            self.projects.current().layers.width() as u32,
            self.projects.current().layers.height() as u32,
        );
        if let Some(pt) = self
            .canvas_widget
            .screen_to_canvas_unclamped(pos, camera, canvas_size)
        {
            self.selection_transform_double_click(pt);
        }
    }

    /// A double-click commits the lifted selection only on empty space: no
    /// gizmo under the pointer this frame, and the point outside the
    /// transformed bbox inflated by the gizmo hit radius (kept constant on
    /// screen by dividing through the camera scale, as `curve_hover_at` does).
    fn selection_transform_double_click(&mut self, pt: (i32, i32)) {
        if !matches!(self.gizmo_hovered, GizmoHit::None) {
            return;
        }
        let radius = GIZMO_HIT_RADIUS / self.projects.current().camera.scale() as f32;
        let on_object = self
            .projects
            .current()
            .transform
            .as_ref()
            .and_then(TransformSession::selection)
            .is_some_and(|t| {
                let (min_x, min_y, max_x, max_y) = t.object.canvas_bbox();
                let (x, y) = (pt.0 as f32, pt.1 as f32);
                x > min_x - radius && x < max_x + radius && y > min_y - radius && y < max_y + radius
            });
        if !on_object {
            self.transform_session_commit();
        }
    }

    /// Dynamic-curve pointer routing: a stroke drag moves the grabbed gizmo and
    /// release ends the drag without committing. A double-click on empty space
    /// (neither a gizmo nor the curve itself) rasterizes the curve as one stroke.
    fn handle_curve_transform_interactions(&mut self, interactions: CanvasInteractions) {
        if interactions.stroke_segment_started {
            if let Some(pt) = interactions.stroke_point {
                self.curve_transform_begin_drag(pt);
            }
        } else if let Some(pt) = interactions.stroke_point {
            // The canvas widget reports `stroke_started` on the first moved
            // frame of a drag; by then the gizmo is already grabbed, so this
            // path moves it rather than re-grabbing under the moved pointer.
            self.curve_transform_drag(pt);
        }
        if interactions.stroke_ended {
            if let Some(curve) = self
                .projects
                .current_mut()
                .transform
                .as_mut()
                .and_then(TransformSession::curve_mut)
            {
                curve.drag = None;
            }
        }
        if let Some(pt) = interactions.double_clicked {
            self.curve_transform_double_click(pt);
        }
    }

    /// Grab the curve gizmo under `pt` (if any) and anchor the drag.
    fn curve_transform_begin_drag(&mut self, pt: (i32, i32)) {
        let canvas_pt = (pt.0 as f32, pt.1 as f32);
        let radius = (GIZMO_HIT_RADIUS / self.projects.current().camera.scale() as f32).max(1.0);
        if let Some(curve) = self
            .projects
            .current_mut()
            .transform
            .as_mut()
            .and_then(TransformSession::curve_mut)
        {
            curve.drag = curve.curve.hit_gizmo(canvas_pt, radius);
            curve.last_pt = canvas_pt;
        }
    }

    /// Move the grabbed gizmo to `pt` (a null grab moves nothing).
    fn curve_transform_drag(&mut self, pt: (i32, i32)) {
        let canvas_pt = (pt.0 as f32, pt.1 as f32);
        if let Some(curve) = self
            .projects
            .current_mut()
            .transform
            .as_mut()
            .and_then(TransformSession::curve_mut)
        {
            if let Some(index) = curve.drag {
                curve.curve.set_gizmo(index, canvas_pt);
            }
            curve.last_pt = canvas_pt;
        }
    }

    /// A double-click commits the curve only when it lands on empty space
    /// (neither a gizmo nor the curve itself).
    fn curve_transform_double_click(&mut self, pt: (i32, i32)) {
        let canvas_pt = (pt.0 as f32, pt.1 as f32);
        let on_object = self
            .projects
            .current()
            .transform
            .as_ref()
            .and_then(TransformSession::curve)
            .is_some_and(|curve| curve.hits_object(canvas_pt));
        if !on_object {
            self.commit_curve_transform();
        }
    }

    /// Rasterize the dynamic curve as exactly one undoable stroke: walk the
    /// flattened spline and feed it to the existing stroke machinery, so
    /// scatter, the stepped tail, paint-once and the undo bbox all apply.
    fn commit_curve_transform(&mut self) {
        let Some(session) = self.projects.current_mut().transform.take() else {
            return;
        };
        let TransformSession::Curve(curve) = session else {
            self.projects.current_mut().transform = Some(session);
            return;
        };
        let samples = curve.curve.samples();
        let Some(&first) = samples.first() else {
            return;
        };
        self.begin_stroke(first);
        for point in &samples[1..] {
            self.continue_stroke(*point);
        }
        self.end_stroke();
    }

    /// Begin a session drag: lock the gizmo element under the pointer and
    /// anchor the pointer delta (the canvas widget reports it per frame).
    ///
    /// Returns `true` when a handle was ALREADY grabbed before this call, i.e.
    /// the caller is seeing a re-fired start (egui's drag threshold or a
    /// canvas re-entry) and must continue the existing drag rather than
    /// re-latch. On the initial press (`t.drag == None`) it latches
    /// `gizmo_hovered` and returns `false`.
    ///
    /// **Drag lock (K2 problem 3):** the `t.drag != None` early return is the
    /// whole latch — while a handle is live this method NEVER overwrites it,
    /// so no other gizmo element (rotate zone, another corner/edge) can start.
    /// The latch is cleared only by the release handler
    /// (`stroke_ended`/`clicked`) in `handle_transform_interactions`, and
    /// start-state re-capture in `transform_session_drag` only fires while the
    /// same `t.drag` is held, so a gesture cannot swap handles mid-drag.
    fn transform_session_begin_drag(&mut self, pt: (i32, i32)) -> bool {
        if let Some(t) = self
            .projects
            .current_mut()
            .transform
            .as_mut()
            .and_then(TransformSession::selection_mut)
        {
            if t.drag != GizmoHit::None {
                return true;
            }
            t.drag = self.gizmo_hovered;
            t.last_pt = (pt.0 as f32, pt.1 as f32);
            // Y2-B: a latched handle means a drag is in flight — the preview
            // switches to the fast algorithm until the release clears this.
            t.preview_dragging = t.drag != GizmoHit::None;
            // A fresh gesture re-captures its start state on the first drag
            // frame (see `transform_session_drag`); clear the MOVE axis latch.
            t.drag_initialized = false;
            t.move_axis = None;
        }
        false
    }

    /// Continue a session drag: the grabbed gizmo (`t.drag`) maps to a
    /// transform of the floating object, with `cur` the current canvas point.
    ///
    /// On the first frame of a gesture the start state is captured
    /// ([`SelectionTransform::start_angle`], `start_pointer_angle`,
    /// `start_pointer`, `start_pos`, `start_local_dims`, `start_anchor`,
    /// `start_anchor_end`, `start_pivot`, `start_flip_h/v`); every arm computes
    /// ABSOLUTE from that state, so a grab-then-release never accumulates error
    /// or jumps.
    ///
    /// Handle semantics:
    /// - **Translate**: MOVE. No dead zone (Q1 problem 2): the object AND its
    ///   (centred) pivot follow the pointer (spec C2) from the FIRST pixel of
    ///   movement, snapped to integer pixels. Shift locks to the horizontal or
    ///   vertical axis chosen from the initial movement direction (latched on
    ///   the first non-zero move).
    /// - **Rotate**: `angle = start_angle + (θ(cur) − θ(start_pointer))` around
    ///   the centre pivot — absolute, never accumulated. Shift snaps the
    ///   resulting absolute angle to [`ROTATE_SNAP_DEG`] (45° multiples).
    /// - **Corners/Edges (P1, local)**: the pointer is projected into the
    ///   object's LOCAL (pre-rotation) frame about the fixed physical anchor, so
    ///   both ratios are true local dimension factors and a rotated non-uniform
    ///   scale stays a rectangle (no shear). A TRACKED axis targets
    ///   `round(start_local_dim * |ratio|)`; an edge's UNTRACKED axis keeps its
    ///   start local dim. Alt anchors at the local CENTRE (`Alt+Shift` is a
    ///   uniform dominant-axis centre scale); Shift alone is a uniform
    ///   opposite-anchor scale. Crossing the anchor on an axis FLIPS it
    ///   (`start_flip XOR ratio < 0`) about the fixed anchor. Shift is IGNORED
    ///   on edges (they are already single-axis).
    ///
    /// **Modifier matrix (scale arms):** corners — `(Alt && Shift)` centre +
    /// uniform; `Alt` centre + free; `Shift` opposite + uniform; else opposite
    /// + free. Edges — `Alt` centre; else opposite. Ctrl is no longer part of
    /// the transform map.
    ///
    /// After a corner or edge scale, [`recenter_pivot_keeping_output`] always
    /// moves the pivot back to the centre of the transformed pixels WITHOUT
    /// moving the rendered output (M1): the pivot is never user-movable.
    fn transform_session_drag(&mut self, pt: (i32, i32)) {
        let Some(t) = self
            .projects
            .current_mut()
            .transform
            .as_mut()
            .and_then(TransformSession::selection_mut)
        else {
            return;
        };
        let cur = (pt.0 as f32, pt.1 as f32);
        let ctx = &self.ctx;
        // Capture / re-capture the start state for this gesture. `begin_drag`
        // clears `drag_initialized`; the `start_drag` check also catches
        // gestures that swap handles without going through it (tests or a
        // programmatic drag).
        if !t.drag_initialized || t.start_drag != t.drag {
            t.start_pointer = t.last_pt;
            t.start_pos = t.object.pos;
            t.start_angle = t.object.angle_deg;
            let (px, py) = t.object.pivot();
            t.start_pointer_angle = if t.last_pt != (px, py) {
                (t.last_pt.1 - py).atan2(t.last_pt.0 - px).to_degrees()
            } else {
                0.0
            };
            // Start-referenced scale state: the LOCAL dims, the physical canvas
            // anchor A (opposite corner/edge midpoint, or the quad centre for
            // Alt) and which side of the local box it is on, the mirror flags
            // and the Alt latch. The scale arms project the pointer into the
            // local frame and compute an ABSOLUTE local target; an edge's
            // untracked axis keeps its start local dim.
            let alt = alt_pressed(ctx);
            let corners = t.object.canvas_corners();
            let (src_w, src_h) = t.object.source_dims();
            let (src_w, src_h) = (src_w as f32, src_h as f32);
            let (a_src, end_x, end_y) = anchor_src_for(t.drag, src_w, src_h, alt);
            t.start_local_dims = t.object.local_scaled_dims();
            t.start_anchor = canvas_anchor(corners, src_w, src_h, a_src);
            t.start_anchor_end = (end_x, end_y);
            t.start_pivot = t.object.pivot();
            t.start_flip_h = t.object.flip_h;
            t.start_flip_v = t.object.flip_v;
            t.start_alt = alt;
            t.move_axis = None;
            t.drag_initialized = true;
            t.start_drag = t.drag;
        }
        match t.drag {
            GizmoHit::Translate => {
                // MOVE: absolute from the press, integer-snapped, with NO dead
                // zone (Q1 problem 2): the object follows the FIRST pixel of
                // pointer movement. Shift axis-locks to the movement direction;
                // the axis latches on the first non-zero movement and stays for
                // the rest of the gesture so small jitters cannot re-pick it.
                let dx = cur.0 - t.start_pointer.0;
                let dy = cur.1 - t.start_pointer.1;
                // Latch the lock axis from the FIRST non-zero movement and keep
                // it for the rest of the gesture (small jitters never re-pick).
                if t.move_axis.is_none() && (dx != 0.0 || dy != 0.0) {
                    t.move_axis = Some(if dx.abs() >= dy.abs() {
                        MoveAxis::Horizontal
                    } else {
                        MoveAxis::Vertical
                    });
                }
                let (mut mx, mut my) = (dx, dy);
                if shift_pressed(ctx) {
                    match t.move_axis {
                        Some(MoveAxis::Horizontal) => my = 0.0,
                        Some(MoveAxis::Vertical) => mx = 0.0,
                        None => {}
                    }
                }
                // Live pixel snapping: integer canvas position, no sub-pixel.
                // ABSOLUTE from the press, so a delta of zero restores the
                // start position.
                let new_pos = ((t.start_pos.0 + mx).round(), (t.start_pos.1 + my).round());
                let delta = (new_pos.0 - t.object.pos.0, new_pos.1 - t.object.pos.1);
                t.object.pos = new_pos;
                t.object
                    .set_pivot((t.object.pivot().0 + delta.0, t.object.pivot().1 + delta.1));
            }
            GizmoHit::Rotate => {
                let (px, py) = t.object.pivot();
                // Guard: with the pointer ON the pivot there is no angle to
                // measure (atan2(0, 0) is meaningless).
                if cur != (px, py) {
                    let theta_cur = (cur.1 - py).atan2(cur.0 - px).to_degrees();
                    // Shortest-path delta from the START pointer angle, wrapped
                    // into (-180, 180] so crossing ±180° never spins backwards.
                    let mut delta = theta_cur - t.start_pointer_angle;
                    while delta > 180.0 {
                        delta -= 360.0;
                    }
                    while delta <= -180.0 {
                        delta += 360.0;
                    }
                    // Absolute from the start angle (never accumulated). Shift
                    // snaps the resulting absolute angle; a single `set_angle`.
                    let angle = t.start_angle + delta;
                    let angle = if shift_pressed(ctx) {
                        snap_rotate_angle(angle)
                    } else {
                        angle
                    };
                    t.object.set_angle(angle);
                }
            }
            GizmoHit::ScaleNW | GizmoHit::ScaleNE | GizmoHit::ScaleSE | GizmoHit::ScaleSW => {
                // Project the pointer into the object's LOCAL (pre-rotation)
                // frame ABOUT THE FIXED CANVAS ANCHOR, so both ratios are true
                // local dimension factors. The anchor was captured once at drag
                // start (opposite corner, or the centre for Alt).
                let anchor = t.start_anchor;
                let sp = t.start_pointer;
                let th = t.start_angle;
                let v = rotate_inv((cur.0 - anchor.0, cur.1 - anchor.1), th);
                let v0 = rotate_inv((sp.0 - anchor.0, sp.1 - anchor.1), th);
                let kx = absolute_axis_ratio(v0.0, v.0, 0.0);
                let ky = absolute_axis_ratio(v0.1, v.1, 0.0);
                if shift_pressed(ctx) {
                    // Shift: uniform (dominant-axis magnitude and sign) about
                    // the fixed anchor. A crossed axis flips both.
                    let k = if kx.abs() >= ky.abs() { kx } else { ky };
                    resize_selection_absolute(t, k, k, (true, true));
                } else {
                    // Free per-axis LOCAL resize; each axis flips independently.
                    resize_selection_absolute(t, kx, ky, (true, true));
                }
                // M1: the pivot is always the centre of the resized pixels.
                recenter_pivot_keeping_output(t);
            }
            GizmoHit::ScaleTop
            | GizmoHit::ScaleBottom
            | GizmoHit::ScaleLeft
            | GizmoHit::ScaleRight => {
                // Edge resize controls exactly ONE LOCAL axis; the other axis
                // keeps its start local dim. The anchor (opposite edge
                // midpoint, or the CENTRE for Alt) is captured once at drag
                // start; both ratios are projected into the local frame about
                // it. Shift is IGNORED on edges.
                let anchor = t.start_anchor;
                let sp = t.start_pointer;
                let th = t.start_angle;
                let v = rotate_inv((cur.0 - anchor.0, cur.1 - anchor.1), th);
                let v0 = rotate_inv((sp.0 - anchor.0, sp.1 - anchor.1), th);
                let rx = absolute_axis_ratio(v0.0, v.0, 0.0);
                let ry = absolute_axis_ratio(v0.1, v.1, 0.0);
                let tracked = match t.drag {
                    GizmoHit::ScaleTop | GizmoHit::ScaleBottom => (false, true),
                    GizmoHit::ScaleLeft | GizmoHit::ScaleRight => (true, false),
                    _ => unreachable!("edge arm matched a non-edge hit"),
                };
                resize_selection_absolute(t, rx, ry, tracked);
                // M1: edge scales recentre the pivot too (single-axis).
                recenter_pivot_keeping_output(t);
            }
            GizmoHit::None => {}
        }
        t.last_pt = cur;
    }

    /// Commit the selection session (D35/D36): the combined cut + paste is
    /// applied to the layer active at commit time as exactly one "Transform"
    /// undo step. A no-op session pushes nothing.
    ///
    /// Part C lifecycle: the lift already CUT the source pixels, so the commit
    /// FIRST transiently restores them (mask-aware, no undo) to give
    /// [`apply_layer_commits_with`] the original before-state, then runs the
    /// normal composite step (which cuts and pastes as ONE undoable command).
    /// A paste-as-transform session (`cut_source == false`, empty snapshot)
    /// skips the restore and only pastes.
    ///
    /// A RECTANGULAR lift stays selected: the transformed destination is
    /// re-captured on the target layer. A MASKED lift derives a NEW mask over
    /// the union destination bbox of the committed pixels (alpha > 0), so the
    /// transformed area stays selected instead of being dropped.
    fn transform_session_commit(&mut self) {
        let Some(session) = self.projects.current_mut().transform.take() else {
            return;
        };
        let TransformSession::Selection(t) = session else {
            self.projects.current_mut().transform = Some(session);
            return;
        };
        let target = self.projects.current_mut().layers.active_layer_id();
        let commits = t.object.commit();
        // Part C: put the original pixels back (no undo) so the composite step
        // below records the correct cut before-state. A paste session has an
        // empty snapshot and is a no-op here.
        if t.cut_source {
            restore_transform_source(&mut self.projects.current_mut().layers, &t.source);
        }
        if let Some(cmd) = apply_layer_commits_with(
            &t.source,
            &commits,
            target,
            &mut self.projects.current_mut().layers,
            t.cut_source,
        ) {
            self.projects.current_mut().undo.push(Box::new(cmd));
            self.mark_dirty();
        }
        if self.lifted_selection_is_masked {
            // A masked lift's new selection is the transformed pixels' shape:
            // a mask over the union destination bbox (alpha > 0 after paste).
            let selection =
                transformed_selection_for(&self.projects.current().layers, target, &commits);
            self.projects.current_mut().selection = selection;
            return;
        }
        let (min_x, min_y, max_x, max_y) = t.object.canvas_bbox();
        let destination = Rect2i::new(
            min_x.round() as i32,
            min_y.round() as i32,
            (max_x - min_x).round() as i32,
            (max_y - min_y).round() as i32,
        );
        let project = self.projects.current_mut();
        let captured = project
            .layers
            .layer(target)
            .and_then(|layer| Selection::capture(&layer.buffer, destination));
        project.selection = captured;
    }

    /// Cancel the session and restore the document (Esc). Part C: the lift CUT
    /// the selected pixels, so they are written back byte-exactly (mask-aware,
    /// NO undo entry) before the original selection is re-captured from the
    /// restored layer. A paste session has an empty snapshot, so it is simply
    /// dropped. The curve variant is dropped (it never touched the document).
    fn cancel_transform(&mut self) {
        let Some(session) = self.projects.current_mut().transform.take() else {
            return;
        };
        let TransformSession::Selection(t) = session else {
            return;
        };
        restore_transform_source(&mut self.projects.current_mut().layers, &t.source);
        let restored = self
            .projects
            .current_mut()
            .layers
            .layer(t.source.layer_id)
            .and_then(|layer| match &t.source.mask {
                None => Selection::capture(&layer.buffer, t.source.rect),
                Some(mask) => Selection::capture_mask(&layer.buffer, t.source.rect, mask.clone()),
            });
        self.projects.current_mut().selection = restored;
    }

    /// Re-upload the floating-pixels preview when the selection session's
    /// transform state changed (D32), so the canvas shows the live transformed
    /// result; drops the texture once the session ends. The curve variant has
    /// no pixel preview (it paints vectorially) and clears any stale texture.
    ///
    /// **Two-tier algorithm (Y2-B):** while the session is being DRAGGED
    /// (`SelectionTransform::preview_dragging`) the preview renders with the
    /// FAST algorithm ([`TRANSFORM_DRAG_PREVIEW_ALGORITHM`], Rotxel); when idle
    /// it renders with the selected algorithm (`object.algorithm()`, kept in
    /// sync with `ProjectSession::transform_algorithm` by the W2 wiring). The
    /// override never mutates the object, so commit still uses the selected
    /// algorithm.
    ///
    /// Item 7 dirty-gate: the render + upload runs only when
    /// [`TransformPreviewKey`] (pos / pivot / angle / scale / flips / dragging)
    /// differs from the last upload, so an idle hover frame no longer pays the
    /// render + texture-upload cost. `dragging` is part of the key so a release
    /// re-renders with the selected algorithm even when the transform state is
    /// unchanged. A missing texture always forces a re-upload, so a cleared key
    /// can never leave the preview blank.
    fn refresh_transform_preview(&mut self) {
        let current = self
            .projects
            .current()
            .transform
            .as_ref()
            .and_then(TransformSession::selection)
            .map(|t| TransformPreviewKey::of(&t.object, t.preview_dragging));
        let Some(current) = current else {
            // No selection session: clear any stale preview.
            self.transform_preview = None;
            self.transform_preview_key = None;
            return;
        };
        if self.transform_preview.is_some() && self.transform_preview_key == Some(current) {
            return; // unchanged this frame — skip the render + upload
        }
        let rendered = self
            .projects
            .current_mut()
            .transform
            .as_mut()
            .and_then(TransformSession::selection_mut)
            .and_then(|t| {
                let alg = transform_preview_algorithm(t.preview_dragging, t.object.algorithm());
                t.object
                    .render_with_algorithm(Some(TRANSFORM_PREVIEW_MAX_DIM), alg)
                    .into_iter()
                    .next()
            });
        let Some(r) = rendered else {
            self.transform_preview = None;
            self.transform_preview_key = None;
            return;
        };
        let image = egui::ColorImage::from_rgba_unmultiplied([r.w, r.h], &r.buf);
        // Take the old handle so `set()` (which borrows mutably) never
        // conflicts with the upload at the end of this call.
        let tex = match self.transform_preview.take() {
            Some(mut handle) => {
                handle.set(image, egui::TextureOptions::NEAREST);
                Some(handle)
            }
            None => Some(self.ctx.load_texture(
                "transform_preview",
                image,
                egui::TextureOptions::NEAREST,
            )),
        };
        self.transform_preview = tex;
        self.transform_preview_key = Some(current);
    }

    // -----------------------------------------------------------------------
    // Per-tool dispatch
    // -----------------------------------------------------------------------

    /// Drive the primary-button tool gesture from the global pointer.
    ///
    /// A press that starts outside the canvas draw rect — on no other UI that
    /// wants the pointer — arms the active tool without processing data. While
    /// the button is held the tool processes only while the pointer is inside
    /// the canvas; leaving pauses the session and re-entering restarts it as a
    /// new segment (no interpolation across the gap). A press on any widget
    /// that wants the pointer (dock chrome, buttons, sliders, …) never arms
    /// the tool. A press that arms inside the canvas starts its stroke right
    /// there (the brush stamps the press position without any movement);
    /// presses that start inside the canvas also keep flowing through the
    /// canvas widget's own reporting. This machine adds the outside-armed
    /// path, the press-stamp, and the pause/resume/no-teleport behavior.
    ///
    /// "Inside" is the pointer's brush FOOTPRINT overlapping the canvas: the
    /// cursor pixel itself may be outside the draw rect while a large brush
    /// still paints its in-bounds part.
    fn update_tool_gesture(
        &mut self,
        ctx: &egui::Context,
        interactions: CanvasInteractions,
    ) -> CanvasInteractions {
        let camera = self.projects.current().camera;
        let canvas_size = (
            self.projects.current().layers.width() as u32,
            self.projects.current().layers.height() as u32,
        );
        let tool = self.projects.current().tool_state.tool();
        let selection_tool = tool == Tool::Fieldier;
        // A live transform session owns the gesture surface, so a Shift+press
        // must not latch a draw-tool line over it (mirrors the dispatch guard).
        let transform_active = self.projects.current().transform.is_some();
        // L1-A: a live SELECTION transform owns the ENTIRE primary gesture.
        // The tool arming / inside-outside machine below is what breaks a
        // transform drag when the pointer crosses the canvas edge: leaving
        // flips `tool_outside`, re-entering re-fires `stroke_segment_started`,
        // and the footprint-mapped `stroke_point` overwrites the clamp-free one
        // — all of which corrupt the in-flight transform gesture. Bypass the
        // machine and pass the canvas widget's own (clamp-free) stroke stream
        // straight through to `handle_transform_interactions`.
        //
        // The ONLY thing the machine must still provide is the press-time
        // handle latch: egui fires `drag_started` only after the pointer has
        // crossed its drag threshold, by which time it has left the small
        // handle hit square, so latching from the drag-start hover would grab
        // the wrong zone (Issue #1). Re-emit the initial press as a clamp-free
        // `stroke_started`; every later frame is untouched, so inside/outside
        // transitions can no longer inject phantom start/end events.
        //
        // Fieldier/tool behaviour is untouched on every frame without a
        // selection transform (`TransformSession::Curve` keeps its own routing,
        // which relies on `stroke_segment_started`).
        let selection_transform_active = self
            .projects
            .current()
            .transform
            .as_ref()
            .is_some_and(|t| matches!(t, TransformSession::Selection(_)));
        let stroke_size = match tool {
            Tool::Pencil | Tool::Eraser | Tool::Draw => {
                let settings = self.projects.current().draw_settings(tool);
                settings
                    .size
                    .clamp(BrushSpec::MIN_SIZE, BrushSpec::MAX_SIZE)
            }
            // The other tools act on the cursor pixel itself; a 1 px footprint
            // keeps their inside test the strict point-in-draw-rect test.
            Tool::Fill | Tool::Eyedropper | Tool::Fieldier => 1,
        };
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        let down = ctx.input(|i| i.pointer.primary_down());
        let released = ctx.input(|i| i.pointer.primary_released());
        let pos = ctx.input(|i| i.pointer.latest_pos());
        let pt = pos.and_then(|p| {
            self.canvas_widget
                .screen_to_canvas_footprint(p, camera, canvas_size, stroke_size)
        });
        // Another UI wants the pointer when egui is using it for a widget other
        // than the canvas itself (the canvas claims presses in its letterbox).
        let other_ui = ctx.egui_is_using_pointer() && !interactions.primary_down_on_canvas;

        if selection_transform_active {
            let mut out = interactions;

            // M1 / EK SORUN: the transform owns the primary gesture GLOBALLY.
            // The canvas widget's own stroke stream is scoped to the canvas
            // draw rect (`primary_response`), so a press/drag/release in the
            // letterbox or entirely outside the widget never reaches it — a
            // grab there could latch but never move, and the drag would die at
            // the widget boundary. DISCARD the widget stream and rebuild it
            // from the global pointer below, so the transform sees exactly one
            // source of truth whether the pointer is inside or outside.
            out.stroke_started = false;
            out.stroke_segment_started = false;
            out.stroke_point = None;
            out.stroke_ended = false;

            // The gizmo quad to hit-test the press against, taken from the live
            // session (detached so the later mutable borrow is unobstructed).
            // O1: the quad rotates with the content.
            let gizmo_corners = self
                .projects
                .current()
                .transform
                .as_ref()
                .and_then(TransformSession::selection)
                .map(|t| t.object.canvas_corners());

            // Latch the grabbed handle on the initial press, before egui's
            // drag threshold moves the pointer off the handle (Issue #1). The
            // hit-test runs at the GLOBAL pointer position through the new
            // `CanvasWidget::gizmo_hit_at`, so a press in the letterbox or
            // outside the widget latches the handle too.
            if pressed && !other_ui {
                if let Some(p) = pos {
                    if let (Some(pt), Some(corners)) = (
                        self.canvas_widget
                            .screen_to_canvas_unclamped(p, camera, canvas_size),
                        gizmo_corners,
                    ) {
                        self.gizmo_hovered = self.canvas_widget.gizmo_hit_at(p, corners, camera);
                        out.stroke_started = true;
                        out.stroke_point = Some(pt);
                    }
                }
            }

            // While the button is held and a handle is latched, keep driving
            // the drag from the GLOBAL pointer with the clamp-free mapping,
            // whatever widget the pointer is over (letterbox, another panel or
            // off the widget entirely). This is what lets a grab that started
            // OUTSIDE the draw rect follow the pointer back toward the canvas;
            // the preview stays clipped (L1) but the mapping is never clamped.
            let dragging = self
                .projects
                .current()
                .transform
                .as_ref()
                .and_then(TransformSession::selection)
                .is_some_and(|t| t.drag != GizmoHit::None);
            if down && dragging {
                if let Some(p) = pos {
                    if let Some(pt) =
                        self.canvas_widget
                            .screen_to_canvas_unclamped(p, camera, canvas_size)
                    {
                        out.stroke_point = Some(pt);
                    }
                }
            }

            // Never let the arming / re-entry machine revive mid-gesture.
            self.tool_armed = false;
            self.tool_outside = false;
            self.last_selection_drag_point = None;
            // A mouseup ANYWHERE ends the drag, including outside the widget.
            if released {
                out.stroke_ended = true;
            }
            return out;
        }

        let mut out = interactions;

        if released {
            if self.tool_armed {
                out.stroke_ended = true;
            }
            self.tool_armed = false;
            self.tool_outside = false;
            self.last_selection_drag_point = None;
            return out;
        }

        if pressed {
            if other_ui {
                self.tool_armed = false;
                self.tool_outside = false;
                self.last_selection_drag_point = None;
                self.reset_line_gesture();
            } else {
                self.tool_armed = true;
                self.tool_outside = pt.is_none();
                self.last_selection_drag_point = None;
                if matches!(tool, Tool::Pencil | Tool::Eraser | Tool::Draw) {
                    if shift_pressed(ctx) && !transform_active {
                        // Shift+press latches LINE mode for the whole gesture:
                        // nothing is painted until release. The anchor is the
                        // press point (or the first in-canvas point if the
                        // press landed outside).
                        self.line_mode = true;
                        self.line_dynamic = false;
                        self.line_anchor = pt;
                        self.line_end = pt;
                    } else if alt_pressed(ctx) && !transform_active {
                        // Alt+press latches the dynamic line: the same live
                        // anchor→end preview, but release turns it into a
                        // bendable transform object instead of a stroke.
                        self.line_mode = true;
                        self.line_dynamic = true;
                        self.line_anchor = pt;
                        self.line_end = pt;
                    } else {
                        self.reset_line_gesture();
                        if let Some(pt) = pt {
                            // Press paints immediately: start the stroke at the
                            // press position so the brush stamps without moving.
                            out.stroke_segment_started = true;
                            out.stroke_point = Some(pt);
                        }
                    }
                } else if selection_tool {
                    // A selection gesture may begin outside the canvas: the press
                    // is projected onto the canvas and clamped, so the drag only
                    // ever measures pixels that exist.
                    if let Some(p) = pt.or_else(|| {
                        pos.map(|p| {
                            self.canvas_widget
                                .screen_to_canvas_clamped(p, camera, canvas_size)
                        })
                    }) {
                        self.tool_outside = false;
                        out.stroke_started = true;
                        out.stroke_point = Some(p);
                    }
                }
            }
            return out;
        }

        if down && self.tool_armed {
            if self.line_mode {
                // Preview only: track the live end point (Ctrl snaps the angle
                // to 22.5° increments) without touching the layer.
                if let Some(pt) = pt {
                    self.tool_outside = false;
                    let anchor = self.line_anchor.get_or_insert(pt);
                    self.line_end = Some(if ctx.input(|i| i.modifiers.ctrl) {
                        snap_line_angle(*anchor, pt)
                    } else {
                        pt
                    });
                } else {
                    self.tool_outside = true;
                }
                return out;
            }
            match pt {
                Some(pt) if self.tool_outside => {
                    self.tool_outside = false;
                    out.stroke_segment_started = true;
                    out.stroke_point = Some(pt);
                    if matches!(
                        self.projects.current().tool_state.tool(),
                        Tool::Fill | Tool::Eyedropper
                    ) {
                        out.clicked = Some(pt);
                    }
                }
                Some(pt) => {
                    out.stroke_point = Some(pt);
                }
                None if selection_tool => {
                    // A selection gesture follows the pointer past the canvas
                    // edge: the position is projected onto the canvas and
                    // clamped, and the last edge point is held while the
                    // platform reports no pointer position at all.
                    self.tool_outside = false;
                    let edge = pos
                        .map(|p| {
                            self.canvas_widget
                                .screen_to_canvas_clamped(p, camera, canvas_size)
                        })
                        .or(self.last_selection_drag_point);
                    if let Some(edge) = edge {
                        out.stroke_point = Some(edge);
                    }
                }
                None => {
                    self.tool_outside = true;
                }
            }
            if selection_tool {
                if let Some(point) = out.stroke_point {
                    self.last_selection_drag_point = Some(point);
                }
            }
        }

        out
    }

    /// Route canvas interactions to the active tool. A live transform session
    /// owns the gesture surface (D32): stroke drags drive the floating object
    /// and release commits it, so the temporary eyedropper and tool dispatch
    /// are suspended until the session ends.
    fn handle_interactions(&mut self, interactions: CanvasInteractions) {
        if self.projects.current_mut().transform.is_some() {
            self.handle_transform_interactions(interactions);
            return;
        }
        // Shift+scroll resizes the Draw tool's brush; Ctrl+scroll sets its
        // brush shape directionally (canvas zoom is plain scroll).
        if interactions.brush_scroll != 0.0 {
            let session = self.projects.current_mut();
            let tool = session.tool_state.tool();
            if matches!(tool, Tool::Pencil | Tool::Eraser | Tool::Draw) {
                let steps = scroll_steps(interactions.brush_scroll);
                if steps != 0 {
                    let settings = session.draw_settings_mut(tool);
                    let next = settings.size as i32 + steps;
                    settings.size =
                        next.clamp(BrushSpec::MIN_SIZE as i32, BrushSpec::MAX_SIZE as i32) as u8;
                }
            }
        }
        if let Some(shape) = BrushShape::from_scroll_steps(interactions.shape_cycle) {
            let session = self.projects.current_mut();
            let tool = session.tool_state.tool();
            if matches!(tool, Tool::Pencil | Tool::Eraser | Tool::Draw) {
                session.draw_settings_mut(tool).shape = shape;
            }
        }
        // Ctrl+scroll over the Fieldier wand adjusts the tolerance; the draw
        // tools keep their brush-shape cycle above.
        if interactions.shape_cycle != 0 {
            let session = self.projects.current_mut();
            if session.tool_state.tool() == Tool::Fieldier
                && session.tool_state.child() == FieldierChild::Wand
            {
                let wand = session.tool_state.wand_mut();
                let next = i32::from(wand.tolerance) + interactions.shape_cycle;
                wand.tolerance = next.clamp(0, i32::from(u8::MAX)) as u8;
            }
        }
        // Alt+scroll adjusts the Draw tool's scatter amount; like the
        // Shift/Ctrl paths it never reaches the camera.
        if interactions.scatter_scroll != 0.0 {
            let session = self.projects.current_mut();
            let tool = session.tool_state.tool();
            if matches!(tool, Tool::Pencil | Tool::Eraser | Tool::Draw) {
                let steps = scroll_steps(interactions.scatter_scroll);
                if steps != 0 {
                    let settings = session.draw_settings_mut(tool);
                    let next = i32::from(settings.scatter) + steps;
                    settings.scatter = next.clamp(0, i32::from(DrawTool::MAX_SCATTER)) as u8;
                }
            }
        }
        // Selection tools own the right button: it means "subtract from the
        // selection", not the temporary eyedropper. Route them before the
        // eyedropper so the right-button gesture reaches the selection handler.
        let active_tool = self.projects.current().tool_state.tool();
        if active_tool == Tool::Fieldier {
            self.handle_select_interactions(interactions);
            return;
        }
        if interactions.eyedropper_started {
            self.projects
                .current_mut()
                .tool_state
                .temporary_eyedropper();
        }
        if let Some(pt) = interactions.eyedropper_point {
            self.sample_color(pt);
        }
        if interactions.eyedropper_ended {
            self.projects.current_mut().tool_state.release_temporary();
        }

        match self.projects.current_mut().tool_state.tool() {
            Tool::Pencil | Tool::Eraser | Tool::Draw => {
                self.handle_stroke_interactions(interactions)
            }
            Tool::Fill => {
                if let Some(pt) = interactions.clicked {
                    self.apply_fill(pt);
                }
            }
            Tool::Eyedropper => {
                if let Some(pt) = interactions.clicked {
                    self.sample_color(pt);
                }
            }
            Tool::Fieldier => {}
        }
    }

    fn handle_stroke_interactions(&mut self, interactions: CanvasInteractions) {
        if self.line_mode {
            // LINE mode: the live endpoint is tracked by `update_tool_gesture`;
            // release turns the anchor→end segment into one stroke, or — for
            // the Alt-driven dynamic variant — into a transform object.
            if interactions.stroke_ended {
                if self.line_dynamic {
                    self.commit_dynamic_line_gesture();
                } else {
                    self.commit_line_gesture();
                }
            }
            return;
        }
        if interactions.stroke_segment_started {
            if let Some(pt) = interactions.stroke_point {
                self.start_stroke_segment(pt);
            }
        } else if interactions.stroke_started {
            if let Some(pt) = interactions.stroke_point {
                // The press already started the stroke (press paints
                // immediately), so a drag that begins later extends it instead
                // of dropping its first point.
                if self.projects.current().stroke.is_some() {
                    self.continue_stroke(pt);
                } else {
                    self.begin_stroke(pt);
                }
            }
        } else if let Some(pt) = interactions.stroke_point {
            self.continue_stroke(pt);
        }
        if interactions.stroke_ended {
            self.end_stroke();
        }
    }

    /// Start (or restart) a stroke segment at `pt`: a fresh [`Stroke::start`]
    /// on an existing session (no interpolation from the previous point), or a
    /// new session when none is in flight.
    fn start_stroke_segment(&mut self, pt: (i32, i32)) {
        if self.projects.current().stroke.is_none() {
            self.begin_stroke(pt);
            return;
        }
        let project = self.projects.current_mut();
        let Some(mut stroke) = project.stroke.take() else {
            return;
        };
        let lid = stroke.layer_id();
        if let Some(layer) = project.layers.layer_mut(lid) {
            stroke.stroke_mut().start(&mut layer.buffer, pt.0, pt.1);
        }
        project.stroke = Some(stroke);
    }

    /// The committed selection as a pixel clip every writing tool honours, or
    /// `None` when nothing is selected (no clip at all — the full canvas).
    fn selection_pixel_clip(selection: Option<&Selection>) -> Option<PixelClip> {
        selection.map(PixelClip::from_selection)
    }

    fn begin_stroke(&mut self, pt: (i32, i32)) {
        let canvas_rect = self.canvas_rect();
        let session = self.projects.current_mut();
        if session.stroke.is_some() {
            return;
        }
        let mode = match session.tool_state.tool() {
            Tool::Pencil | Tool::Draw => DrawMode::Pen,
            Tool::Eraser => DrawMode::Eraser,
            _ => return,
        };
        let layer_id = session.layers.active_layer_id();
        let settings = *session.draw_settings(session.tool_state.tool());
        let brush = BrushSpec::sanitize(settings.size, settings.shape);
        // The selection clip is the single hook every pixel-writing path shares:
        // pen/eraser/draw strokes, LINE commits and dynamic-curve commits all
        // begin here, so they all write only inside the selection.
        let clip = Self::selection_pixel_clip(session.selection.as_ref());
        let stroke = Stroke::new(brush, mode, session.color)
            .with_scatter(settings.scatter)
            .with_scatter_shape(settings.scatter_shape)
            .with_tail(settings.tail)
            .with_clip(clip);
        let Some(mut stroke_session) = StrokeSession::begin(
            &session.layers.active_layer().buffer,
            stroke,
            canvas_rect,
            layer_id,
        ) else {
            return;
        };
        stroke_session.stroke_mut().start(
            &mut session.layers.active_layer_mut().buffer,
            pt.0,
            pt.1,
        );
        session.stroke = Some(stroke_session);
    }

    fn continue_stroke(&mut self, pt: (i32, i32)) {
        let project = self.projects.current_mut();
        let Some(mut stroke) = project.stroke.take() else {
            return;
        };
        let lid = stroke.layer_id();
        if let Some(layer) = project.layers.layer_mut(lid) {
            stroke
                .stroke_mut()
                .continue_to(&mut layer.buffer, pt.0, pt.1);
        }
        project.stroke = Some(stroke);
    }

    fn end_stroke(&mut self) {
        let project = self.projects.current_mut();
        if let Some(session) = project.stroke.take() {
            let lid = session.layer_id();
            let Some(layer) = project.layers.layer(lid) else {
                return;
            };
            if let Some(cmd) = session.finish(&layer.buffer) {
                project.undo.push(Box::new(cmd));
                project.mark_dirty();
            }
        }
    }

    /// Commit a latched LINE gesture as exactly one stroke: begin at the
    /// anchor, extend to the end (the existing [`Stroke`] Bresenham-interpolates
    /// the whole segment, so scatter, tail, paint-once and the undo bbox all
    /// apply), then finish. A gesture that painted nothing pushes no undo step.
    fn commit_line_gesture(&mut self) {
        if let (Some(anchor), Some(end)) = (self.line_anchor, self.line_end) {
            self.begin_stroke(anchor);
            self.continue_stroke(end);
            self.end_stroke();
        }
        self.reset_line_gesture();
    }

    /// Commit the Alt-driven dynamic line as a transform object instead of a
    /// stroke: the anchor→end segment becomes a [`CurveTransform`] whose four
    /// gizmos split it into five equal parts. Nothing is written to the canvas
    /// until the object is committed by a double-click on empty space.
    fn commit_dynamic_line_gesture(&mut self) {
        if let (Some(anchor), Some(end)) = (self.line_anchor, self.line_end) {
            self.projects.current_mut().transform =
                Some(TransformSession::Curve(CurveTransformSession {
                    curve: CurveTransform::line(anchor, end),
                    drag: None,
                    last_pt: (anchor.0 as f32, anchor.1 as f32),
                }));
        }
        self.reset_line_gesture();
    }

    fn reset_line_gesture(&mut self) {
        self.line_mode = false;
        self.line_dynamic = false;
        self.line_anchor = None;
        self.line_end = None;
    }

    /// One fill gesture = one undoable command (D48), bounded by the
    /// committed selection.
    fn apply_fill(&mut self, pt: (i32, i32)) {
        let project = self.projects.current_mut();
        let layer_id = project.layers.active_layer_id();
        let color = project.color;
        let clip = Self::selection_pixel_clip(project.selection.as_ref());
        let cmd = fill_command_clipped(
            layer_id,
            &mut project.layers.active_layer_mut().buffer,
            pt.0,
            pt.1,
            color,
            false,
            clip.as_ref(),
        );
        if let Some(cmd) = cmd {
            project.undo.push(Box::new(cmd));
            project.mark_dirty();
        }
    }

    /// Sample the composite color under the cursor (what the user sees).
    fn sample_color(&mut self, pt: (i32, i32)) {
        if pt.0 < 0 || pt.1 < 0 {
            return;
        }
        let project = self.projects.current_mut();
        let composite = project.layers.composite_layers();
        if let Some(color) = composite.get_pixel(pt.0 as usize, pt.1 as usize) {
            project.color = color;
        }
    }

    // -----------------------------------------------------------------------
    // Clipboard (D51)
    // -----------------------------------------------------------------------

    /// Copy the committed selection into the internal clipboard, respecting the
    /// mask shape (holes stay transparent), and mirror it to the OS image
    /// clipboard when the host supports it.
    fn copy_selection(&mut self) {
        let Some(sel) = &self.projects.current_mut().selection else {
            return;
        };
        let (bbox, mask) = sel.to_shape();
        let Some(clip) = ClipboardRegion::from_pixel_buffer_masked(
            &self.projects.current_mut().layers.active_layer().buffer,
            bbox,
            &mask,
        ) else {
            return;
        };
        self.projects.current_mut().clipboard = Some(clip.clone());
        self.push_os_clipboard_image(&clip);
    }

    /// Cut the committed selection: copy to clipboard, clear the selected
    /// pixels via an undoable DeltaRecorder, drop the selection.
    fn cut_selection(&mut self) {
        let Some(sel) = &self.projects.current_mut().selection else {
            return;
        };
        let (bbox, mask) = sel.to_shape();
        let Some(clip) = ClipboardRegion::from_pixel_buffer_masked(
            &self.projects.current_mut().layers.active_layer().buffer,
            bbox,
            &mask,
        ) else {
            return;
        };
        let lid = self.projects.current_mut().layers.active_layer_id();
        let Some(recorder) = DeltaRecorder::begin(
            "Cut",
            lid,
            &self.projects.current_mut().layers.active_layer().buffer,
            bbox,
        ) else {
            return;
        };
        let mut clear = vec![0u8; bbox.area() as usize * 4];
        for (idx, selected) in mask.iter().enumerate() {
            if !selected {
                let src = &self.projects.current_mut().layers.active_layer().buffer;
                let x = bbox.x + (idx as i32 % bbox.w);
                let y = bbox.y + (idx as i32 / bbox.w);
                if let Some(px) = src.get_pixel(x as usize, y as usize) {
                    clear[idx * 4] = px.r;
                    clear[idx * 4 + 1] = px.g;
                    clear[idx * 4 + 2] = px.b;
                    clear[idx * 4 + 3] = px.a;
                }
            }
        }
        self.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .blit_region(bbox, &clear);
        let cmd = recorder.finish(&self.projects.current_mut().layers.active_layer().buffer);
        self.projects.current_mut().undo.push(Box::new(cmd));
        self.projects.current_mut().clipboard = Some(clip.clone());
        self.push_os_clipboard_image(&clip);
        self.projects.current_mut().selection = None;
        self.mark_dirty();
    }

    /// Clears the committed selection's pixels as one undoable step and leaves
    /// the selection in place, so it can be moved or cleared again.  A no-op
    /// when nothing is selected.
    fn delete_selection(&mut self) {
        let session = self.projects.current_mut();
        let Some(sel) = session.selection.take() else {
            return;
        };
        let lid = session.layers.active_layer_id();
        let Some(cmd) =
            delete_selected_command(&sel, lid, &mut session.layers.active_layer_mut().buffer)
        else {
            session.selection = Some(sel);
            return;
        };
        session.selection = sel.recaptured(&session.layers.active_layer().buffer);
        session.undo.push(Box::new(cmd));
        self.mark_dirty();
    }

    /// Mirrors the committed selection's pixels as one undoable step and keeps
    /// the same pixels selected.  A no-op when nothing is selected.
    fn flip_selection(&mut self, horizontal: bool) {
        let session = self.projects.current_mut();
        let Some(sel) = session.selection.take() else {
            return;
        };
        let lid = session.layers.active_layer_id();
        let Some(cmd) = flip_selected_command(
            &sel,
            horizontal,
            lid,
            &mut session.layers.active_layer_mut().buffer,
        ) else {
            session.selection = Some(sel);
            return;
        };
        let (bbox, mask) = sel.mirrored_shape(horizontal);
        session.selection =
            Selection::capture_mask(&session.layers.active_layer().buffer, bbox, mask);
        session.undo.push(Box::new(cmd));
        self.mark_dirty();
    }

    /// Rotates the committed selection's pixels 90° clockwise as one undoable
    /// step and keeps the same pixels selected.  A no-op when nothing is selected.
    fn rotate_selection(&mut self) {
        let session = self.projects.current_mut();
        let Some(sel) = session.selection.take() else {
            return;
        };
        let lid = session.layers.active_layer_id();
        let Some(cmd) =
            rotate_selected_command(&sel, lid, &mut session.layers.active_layer_mut().buffer)
        else {
            session.selection = Some(sel);
            return;
        };
        let (bbox, mask) = sel.rotate_shape();
        session.selection =
            Selection::capture_mask(&session.layers.active_layer().buffer, bbox, mask);
        session.undo.push(Box::new(cmd));
        self.mark_dirty();
    }

    /// Replaces the committed selection with its complement over the canvas, so
    /// a later fill or delete hits everything the selection did not cover.  A
    /// no-op when nothing is selected.
    fn invert_selection(&mut self) {
        let Some(sel) = &self.projects.current().selection else {
            return;
        };
        let (bbox, mask) = sel.inverted_shape();
        let Some(inverted) = Selection::capture_mask(
            &self.projects.current().layers.active_layer().buffer,
            bbox,
            mask,
        ) else {
            return;
        };
        self.projects.current_mut().selection = Some(inverted);
    }

    /// Paste the clipboard image AS A TRANSFORM OBJECT (Part B): the pasted
    /// pixels float on the gizmo and are only written to the layer on commit,
    /// exactly like a lifted selection but without cutting a source.
    ///
    /// Source priority: the OS image clipboard first (arboard), else the
    /// internal project clipboard. No image → no-op. The object is placed
    /// CENTRED on the cursor's canvas pixel (exact when inside the canvas,
    /// nearest-edge when outside).
    ///
    /// The OS read is ASYNCHRONOUS (G3 finding #6): `get_image()` can block for
    /// seconds on Linux, so this only captures the cursor and starts a worker
    /// thread; [`Self::poll_os_paste`] completes the paste on a later frame.
    fn paste_clipboard_as_transform(&mut self) {
        let Some(cursor) = self.paste_cursor_canvas() else {
            return;
        };
        self.begin_os_paste(cursor);
    }

    /// Start an asynchronous OS clipboard read for paste (G3 finding #6).
    ///
    /// A background thread owns its OWN `arboard::Clipboard` (created on that
    /// thread) and sends the decoded RGBA8 image — or `None` when there is no
    /// image / no clipboard service — back through a channel. While a read is
    /// already in flight the request is ignored so channels cannot stack.
    fn begin_os_paste(&mut self, cursor: (i32, i32)) {
        if self.os_paste_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::ui::clipboard::read_image());
        });
        self.os_paste_rx = Some(crate::ui::clipboard::PendingPasteRead::start(rx));
        self.os_paste_cursor = Some(cursor);
    }

    /// Poll the in-flight OS clipboard read once per frame; when it lands,
    /// complete the paste-as-transform at the captured cursor. Falls back to
    /// the internal project clipboard when the OS read yields no image.
    ///
    /// The wait is bounded by
    /// [`crate::ui::clipboard::PendingPasteRead`]: if `arboard::get_image()`
    /// has not delivered within `PASTE_WAIT_FRAMES` the read resolves as
    /// no-image, the receiver is dropped, and the internal clipboard fallback
    /// runs exactly as for a completed read. This is what keeps a hung/slow OS
    /// read on Wayland from wedging paste forever — clearing `os_paste_rx` also
    /// releases the stacking guard in [`Self::begin_os_paste`] so the next
    /// Ctrl+V starts a fresh read. A disconnected sender (worker panic /
    /// dropped sender) is mapped to a completed no-image read by
    /// [`crate::ui::clipboard::poll_read`], so that path falls back too.
    fn poll_os_paste(&mut self) {
        // Nothing in flight: nothing to resolve.
        let Some(pending) = self.os_paste_rx.as_mut() else {
            return;
        };
        // `None` means still pending within the bounded budget.
        let read = match pending.poll() {
            Some(read) => read,
            None => return,
        };
        // Resolved (image / no image / disconnect / timeout): consume the
        // request and release the "read in flight" guard.
        self.os_paste_rx = None;
        let Some(cursor) = self.os_paste_cursor.take() else {
            return;
        };
        let region = read
            .and_then(|(pixels, w, h)| ClipboardRegion::try_new(w, h, pixels))
            .or_else(|| self.projects.current().clipboard.clone());
        let Some(region) = region else {
            return;
        };
        self.start_transform_from_region(&region, cursor);
    }

    /// The paste cursor in canvas coordinates: the exact pixel under the
    /// pointer, or the nearest canvas pixel when the pointer is outside the
    /// draw rect. `None` only when there is no pointer position at all.
    fn paste_cursor_canvas(&self) -> Option<(i32, i32)> {
        let pos = self.ctx.input(|i| i.pointer.latest_pos())?;
        let camera = self.projects.current().camera;
        let canvas_size = (
            self.projects.current().layers.width() as u32,
            self.projects.current().layers.height() as u32,
        );
        Some(
            self.canvas_widget
                .screen_to_canvas(pos, camera, canvas_size)
                .unwrap_or_else(|| {
                    self.canvas_widget
                        .screen_to_canvas_clamped(pos, camera, canvas_size)
                }),
        )
    }

    /// Begin a paste-as-transform session from `region`, centred on `cursor`.
    /// The session has NO real source (empty snapshot, `cut_source = false`),
    /// so committing it only pastes and Esc just drops it.
    fn start_transform_from_region(&mut self, region: &ClipboardRegion, cursor: (i32, i32)) {
        if region.is_empty() {
            return;
        }
        let pos = paste_origin_for(cursor, (region.width() as i32, region.height() as i32));
        let buffer = LayerBuffer {
            layer_id: 0,
            w: region.width(),
            h: region.height(),
            buf: region.pixels().to_vec(),
        };
        let mut object = TransformObject::lift(buffer, (pos.0 as f32, pos.1 as f32));
        // Paste-as-transform is still a selection transform: seed the algorithm
        // from the session so it matches the selector.
        object.set_algorithm(self.projects.current().transform_algorithm);
        let start_local_dims = object.local_scaled_dims();
        let start_pivot = object.pivot();
        let source = TransformSource {
            layer_id: self.projects.current_mut().layers.active_layer_id(),
            rect: Rect2i::new(0, 0, 0, 0),
            snapshot: Vec::new(),
            mask: None,
        };
        self.projects.current_mut().transform =
            Some(TransformSession::Selection(SelectionTransform {
                source,
                object,
                drag: GizmoHit::None,
                last_pt: (pos.0 as f32, pos.1 as f32),
                cut_source: false,
                start_angle: 0.0,
                start_pointer_angle: 0.0,
                start_drag: GizmoHit::None,
                start_pointer: (pos.0 as f32, pos.1 as f32),
                start_pos: (pos.0 as f32, pos.1 as f32),
                start_local_dims,
                start_anchor: (pos.0 as f32, pos.1 as f32),
                start_anchor_end: (AnchorEnd::Center, AnchorEnd::Center),
                start_pivot,
                start_flip_h: false,
                start_flip_v: false,
                start_alt: false,
                move_axis: None,
                drag_initialized: false,
                preview_dragging: false,
            }));
        self.projects.current_mut().selection = None;
        self.projects.current_mut().gesture = SelectGesture::Idle;
        self.lifted_selection_is_masked = false;
    }

    /// Mirror a clipboard region to the OS image clipboard when the host
    /// supports it.  Two best-effort paths: the egui/winit `copy_image` bridge
    /// and a direct arboard write (so an external paste works even without the
    /// egui bridge). Errors are ignored.
    fn push_os_clipboard_image(&mut self, clip: &ClipboardRegion) {
        if !self.projects.current().os_clipboard_available {
            return;
        }
        // Straight RGBA must pass through unchanged: `from_rgba_premultiplied`
        // stores the bytes verbatim (it does NOT re-scale them), whereas
        // `from_rgba_unmultiplied` would premultiply them and double-darken the
        // translucent pixels, since the OS clipboard expects straight alpha.
        let image =
            egui::ColorImage::from_rgba_premultiplied([clip.width(), clip.height()], clip.pixels());
        self.ctx.copy_image(image);
        // Reuse the persistent owner so the Wayland clipboard connection (which
        // IS the selection owner) stays alive past this call (G3 finding #6).
        let _ = self
            .os_clipboard
            .set_image(clip.pixels(), clip.width(), clip.height());
    }

    // -----------------------------------------------------------------------
    // Frame plumbing
    // -----------------------------------------------------------------------

    fn undo_document(&mut self) {
        self.clear_selection();
        let project = self.projects.current_mut();
        let mut ctx = CommandContext {
            layers: &mut project.layers,
        };
        if project.undo.undo(&mut ctx) {
            project.mark_dirty();
        }
    }

    fn redo_document(&mut self) {
        self.clear_selection();
        let project = self.projects.current_mut();
        let mut ctx = CommandContext {
            layers: &mut project.layers,
        };
        if project.undo.redo(&mut ctx) {
            project.mark_dirty();
        }
    }

    /// Handle keyboard shortcuts. While a transform session is live (D32) only
    /// its keys apply — R / Shift+R rotate ±90°, + / − scale, Esc cancels;
    /// the Ctrl/Cmd+Z/Y/C/X/V document shortcuts are suspended. Otherwise:
    /// Ctrl/Cmd+Z undo, Ctrl/Cmd+Y or Ctrl/Cmd+Shift+Z redo, Ctrl/Cmd+C/X/V
    /// clipboard.
    ///
    /// Clipboard note: `egui-winit` INTERCEPTS the raw Ctrl/Cmd+C/X/V key
    /// press and turns it into `Event::Copy` / `Event::Cut` / `Event::Paste`
    /// instead of forwarding a `Event::Key{pressed:true}` (see
    /// `egui-winit-0.36.2/src/lib.rs` `on_keyboard_input`). `Event::Paste` is
    /// only emitted when the OS clipboard holds TEXT, which an image payload
    /// never does, so the keymap's `pressed` check would never fire for the
    /// default bindings. We therefore also detect copy/cut from their events
    /// and paste from the Ctrl/Cmd+V key RELEASE (which egui-winit does not
    /// swallow).
    ///
    /// Why the release is the reliable image-paste signal: the press yields
    /// `Event::Paste(text)` only for a text clipboard and yields nothing at all
    /// for an image clipboard, so a press-based trigger cannot see image
    /// pastes. The matching key RELEASE, however, is forwarded as a normal
    /// `Event::Key{pressed:false}` carrying the still-held modifier, which
    /// covers both text and image payloads with exactly one trigger.
    /// `Event::Paste(_)` is deliberately NOT added as a second trigger: for a
    /// text clipboard the press would fire it and the release would fire the
    /// release path, double-pasting. `command || ctrl` is checked because
    /// egui-winit maps `command = ctrl` on Linux but a backend/build where only
    /// `ctrl` is set must still work. The keymap path is kept so custom
    /// non-Command bindings work.
    fn handle_shortcuts(&mut self) {
        if self.projects.current_mut().transform.is_some() {
            let (
                rotate_cw,
                rotate_ccw,
                grow,
                shrink,
                flip_h,
                flip_v,
                esc,
                commit,
                move_up,
                move_down,
                move_left,
                move_right,
                step_large,
            ) = self.ctx.input(|i| {
                // T2-B: raw arrow/WASD keys move the floating transform by an
                // integer pixel step. They are read directly (like Enter) so
                // the tool/child keybindings below (W→wand, S→rectangle,
                // D→pen) stay inert while a transform is live — this branch
                // `return`s before them. Shift OR Alt selects the larger step.
                (
                    self.keymap
                        .pressed(crate::input::Action::TransformRotateCw, i),
                    self.keymap
                        .pressed(crate::input::Action::TransformRotateCcw, i),
                    self.keymap.pressed(crate::input::Action::TransformGrow, i),
                    self.keymap
                        .pressed(crate::input::Action::TransformShrink, i),
                    self.keymap.pressed(crate::input::Action::FlipHorizontal, i),
                    self.keymap.pressed(crate::input::Action::FlipVertical, i),
                    self.keymap
                        .pressed(crate::input::Action::CancelTransform, i),
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::W),
                    i.key_pressed(egui::Key::ArrowDown) || i.key_pressed(egui::Key::S),
                    i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::A),
                    i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::D),
                    i.modifiers.shift || i.modifiers.alt,
                )
            });
            if let Some(t) = self
                .projects
                .current_mut()
                .transform
                .as_mut()
                .and_then(TransformSession::selection_mut)
            {
                if rotate_cw {
                    t.object.rotate_by((90f32).to_radians());
                }
                if rotate_ccw {
                    t.object.rotate_by((-90f32).to_radians());
                }
                if grow {
                    let (sx, sy) = t.object.scale_xy();
                    t.object
                        .resize((sx * 1.25).min(64.0), (sy * 1.25).min(64.0));
                }
                if shrink {
                    let (sx, sy) = t.object.scale_xy();
                    t.object
                        .resize((sx / 1.25).max(0.01), (sy / 1.25).max(0.01));
                }
                if flip_h {
                    t.object.set_flips(!t.object.flip_h, t.object.flip_v);
                }
                if flip_v {
                    t.object.set_flips(t.object.flip_h, !t.object.flip_v);
                }
                // T2-B: arrow keys / WASD translate the floating object by an
                // integer pixel step, moving `pos` AND the pivot together so
                // the transform is a pure Translate (no re-anchor, no scale).
                // Opposite keys cancel; a larger grid step is used while Shift
                // or Alt is held.
                let mut mdx = 0.0f32;
                let mut mdy = 0.0f32;
                if move_right {
                    mdx += 1.0;
                }
                if move_left {
                    mdx -= 1.0;
                }
                if move_down {
                    mdy += 1.0;
                }
                if move_up {
                    mdy -= 1.0;
                }
                if mdx != 0.0 || mdy != 0.0 {
                    // The step is always an integer, so the pixel-snapped
                    // position stays integer after every press.
                    let step = if step_large {
                        TRANSFORM_KEY_MOVE_LARGE_STEP
                    } else {
                        1.0
                    };
                    let (dx, dy) = (mdx * step, mdy * step);
                    t.object.pos = (t.object.pos.0 + dx, t.object.pos.1 + dy);
                    let pivot = t.object.pivot();
                    t.object.set_pivot((pivot.0 + dx, pivot.1 + dy));
                }
            }
            if esc {
                // Esc CANCELS and restores both variants: a lifted selection
                // goes back where it came from (nothing was cut yet, so no
                // undo entry is pushed), and a dynamic curve, which is still
                // being shaped, is dropped.
                self.cancel_transform();
            }
            if commit {
                // Enter CONFIRMS the active transform, mirroring the
                // double-click-on-empty-space gesture.
                self.transform_session_commit();
            }
            return;
        }
        let (
            undo,
            redo,
            copy,
            cut,
            paste,
            toggle_grid,
            select_pen,
            select_eraser,
            toggle_color_picker,
            swap_colors,
            cancel_selection,
            select_delete,
            invert_selection,
            flip_selection_h,
            flip_selection_v,
            rotate_selection,
            select_rectangle,
            select_wand,
            select_lasso,
        ) = self.ctx.input(|i| {
            // Clipboard shortcuts: egui-winit swallows the raw Ctrl/Cmd+C/X/V
            // press, so detect them from the events it emits instead. Copy/Cut
            // arrive as `Event::Copy`/`Event::Cut`; Ctrl/Cmd+V is detected from
            // the V key release (the press is swallowed and `Event::Paste` only
            // fires for TEXT clipboards). The keymap path is retained for
            // custom non-Command bindings.
            let copy = self.keymap.pressed(crate::input::Action::Copy, i)
                || i.events.iter().any(|e| matches!(e, egui::Event::Copy));
            let cut = self.keymap.pressed(crate::input::Action::Cut, i)
                || i.events.iter().any(|e| matches!(e, egui::Event::Cut));
            let paste = self.keymap.pressed(crate::input::Action::Paste, i)
                || (i.key_released(egui::Key::V) && (i.modifiers.command || i.modifiers.ctrl));
            (
                self.keymap.pressed(crate::input::Action::Undo, i),
                self.keymap.pressed(crate::input::Action::Redo, i),
                copy,
                cut,
                paste,
                self.keymap.pressed(crate::input::Action::ToggleGrid, i),
                self.keymap.pressed(crate::input::Action::SelectPen, i),
                self.keymap.pressed(crate::input::Action::SelectEraser, i),
                self.keymap
                    .pressed(crate::input::Action::ToggleColorPicker, i),
                self.keymap.pressed(crate::input::Action::SwapColors, i),
                self.keymap
                    .pressed(crate::input::Action::CancelTransform, i),
                self.keymap.pressed(crate::input::Action::SelectDelete, i),
                self.keymap
                    .pressed(crate::input::Action::InvertSelection, i),
                self.keymap
                    .pressed(crate::input::Action::FlipSelectionHorizontal, i),
                self.keymap
                    .pressed(crate::input::Action::FlipSelectionVertical, i),
                self.keymap
                    .pressed(crate::input::Action::RotateSelection, i),
                self.keymap
                    .pressed(crate::input::Action::SelectRectangle, i),
                self.keymap.pressed(crate::input::Action::SelectWand, i),
                self.keymap.pressed(crate::input::Action::SelectLasso, i),
            )
        });
        if cancel_selection {
            // Esc cancels the selection.  A live transform is handled by the
            // branch above (its Esc cancels the floating object), so here the
            // selection is a plain committed region.
            self.clear_selection();
        }
        if undo {
            self.undo_document();
        }
        if redo {
            self.redo_document();
        }
        if copy {
            self.copy_selection();
        }
        if cut {
            self.cut_selection();
        }
        if paste {
            self.paste_clipboard_as_transform();
        }
        if select_delete {
            self.delete_selection();
        }
        if invert_selection {
            self.invert_selection();
        }
        if flip_selection_h {
            self.flip_selection(true);
        }
        if flip_selection_v {
            self.flip_selection(false);
        }
        if rotate_selection {
            self.rotate_selection();
        }
        if toggle_grid {
            self.projects.current_mut().grid_visible = !self.projects.current_mut().grid_visible;
        }
        if select_pen {
            self.projects
                .current_mut()
                .tool_state
                .select_tool(Tool::Pencil);
        }
        if select_eraser {
            self.projects
                .current_mut()
                .tool_state
                .select_tool(Tool::Eraser);
        }
        if toggle_color_picker {
            self.toggle_color_picker();
        }
        if swap_colors {
            self.apply_toolbar_events(vec![ToolbarEvent::SwapColors]);
        }
        if select_rectangle {
            self.select_fieldier_child(FieldierChild::Rectangle);
        }
        if select_wand {
            self.select_fieldier_child(FieldierChild::Wand);
        }
        if select_lasso {
            self.select_fieldier_child(FieldierChild::Lasso);
        }
    }

    /// A deliberate Fieldier child switch from a keybinding: commit any live
    /// floating transform first, then select the child.
    fn select_fieldier_child(&mut self, child: FieldierChild) {
        self.transform_session_commit();
        self.projects.current_mut().tool_state.select_child(child);
    }

    fn app_to_document(&self) -> Document {
        self.projects.current().to_document()
    }

    fn document_to_app(&mut self, doc: &Document) {
        self.projects.current_mut().load_document(doc);
        self.invalidate_host_project_cache();
    }

    fn file_new(&mut self) {
        if self.projects.current().is_dirty() {
            self.projects.current_mut().pending_action = Some(PendingAction::New {
                project: self.projects.current().id,
                width: self.projects.current_mut().new_draft_w,
                height: self.projects.current_mut().new_draft_h,
                tile: self.projects.current().tile_size,
            });
        } else {
            self.projects.current_mut().dialog_open = true;
        }
    }

    fn file_open(&mut self) {
        let Some(path) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        if self.projects.current().is_dirty() {
            self.projects.current_mut().pending_action = Some(PendingAction::Open {
                project: self.projects.current().id,
                path,
            });
        } else {
            self.load_project(path);
        }
    }

    fn file_save(&mut self) -> bool {
        if let Some(path) = self.projects.current().path.clone() {
            self.save_to(path)
        } else {
            self.file_save_as()
        }
    }

    fn file_save_as(&mut self) -> bool {
        let Some(path) = rfd::FileDialog::new().pick_folder() else {
            return false;
        };
        self.save_to(path)
    }

    fn save_to(&mut self, path: PathBuf) -> bool {
        let doc = self.app_to_document();
        match save_document(&doc, &path) {
            Ok(()) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| String::from("Untitled"));
                let session = self.projects.current_mut();
                session.path = Some(path.clone());
                session.name = name;
                session.mark_saved();
                self.projects.current_mut().last_error = None;
                self.update_title();
                self.clear_active_recovery();
                true
            }
            Err(err) => {
                self.projects.current_mut().last_error = Some(format!("Failed to save: {err}"));
                false
            }
        }
    }

    fn load_project(&mut self, path: PathBuf) {
        if let Some(project) = self.projects.find_by_path(&path) {
            let _ = self.activate_project(project);
            self.update_title();
            return;
        }
        match load_document(&path) {
            Ok(doc) => {
                let project = self.new_project(
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| String::from("Untitled")),
                    doc.canvas_width.max(1) as usize,
                    doc.canvas_height.max(1) as usize,
                );
                debug_assert_eq!(self.projects.active_id(), Some(project));
                self.document_to_app(&doc);
                let project_palettes = load_project_palettes(&path);
                self.projects
                    .current_mut()
                    .palettes
                    .extend(project_palettes);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| String::from("Untitled"));
                let session = self.projects.current_mut();
                session.path = Some(path.clone());
                session.name = name;
                session.mark_saved();
                self.projects.current_mut().last_error = None;
                self.update_title();
                self.clear_active_recovery();
            }
            Err(err) => {
                self.projects.current_mut().last_error = Some(format!("Failed to open: {err}"));
            }
        }
    }

    fn create_new_canvas(&mut self, width: u32, height: u32, tile: u32) {
        let w = width.max(1) as usize;
        let h = height.max(1) as usize;
        self.projects.current_mut().layers = LayerStack::new(w, h);
        self.projects.current_mut().undo.clear();
        self.clear_selection();
        self.projects.current_mut().clipboard = None;
        self.transform_preview = None;
        self.projects.current_mut().sequence = {
            let mut seq = AnimationSequence::new("Animation 1");
            seq.set_looping(true);
            seq.push(Frame::new(
                Region::new(Rect2i::new(0, 0, w as i32, h as i32), "Frame 1"),
                100,
            ));
            seq
        };
        self.projects.current_mut().animation = AnimationController::new_from_delays(1, vec![100]);
        {
            let session = self.projects.current_mut();
            session.path = None;
            session.name = String::from("Untitled");
            session.mark_saved();
            session.tile_size = tile.max(1);
        }
        self.projects.current_mut().new_draft_w = width.max(1);
        self.projects.current_mut().new_draft_h = height.max(1);
        self.projects.current_mut().last_error = None;
        self.invalidate_host_project_cache();
        self.update_title();
        self.clear_active_recovery();
    }

    fn clear_recovery_for(&mut self, project: project::ProjectId) {
        if let Some(base) = self.autosave_base.as_ref() {
            let journal_base = recovery_base_for(base, project);
            clear_recovery_journal(&journal_base);
        }
        if let Some(session) = self.projects.session_mut(project) {
            session.recovery = None;
            session.recovery_pending = false;
            session.last_autosave = None;
            session.autosave_generation = 0;
            session.dialog_open = false;
            session.pending_action = None;
        }
    }

    fn clear_active_recovery(&mut self) {
        let project = self.projects.current().id;
        self.clear_recovery_for(project);
    }

    fn execute_pending(&mut self) {
        let Some(action) = self.projects.current_mut().pending_action.take() else {
            return;
        };
        match action {
            PendingAction::New {
                project,
                width,
                height,
                tile,
            } => {
                if self.projects.active_id() != Some(project) {
                    return;
                }
                self.create_new_canvas(width, height, tile);
            }
            PendingAction::Open { project, path } => {
                if self.projects.active_id() != Some(project) {
                    return;
                }
                self.load_project(path);
            }
            PendingAction::Close { project } => {
                if self.projects.active_id() != Some(project) {
                    return;
                }
                // The document is going away; clear the dirty flag so the
                // follow-up CloseRequested skips the guard.
                self.projects.current_mut().mark_saved();
                self.close_project(project, project::CloseProject::Discard)
                    .expect("saved pending close must remove its project");
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Guard "Save": persist first, then run the deferred action only if the
    /// write succeeded (so a cancelled Save As keeps the guard open).
    fn guard_save(&mut self) -> bool {
        let pending = self.projects.current_mut().pending_action.take();
        let saved = self.file_save();
        if saved {
            self.projects.current_mut().pending_action = pending;
            self.execute_pending();
        } else {
            self.projects.current_mut().pending_action = pending;
        }
        saved
    }

    /// Guard "Discard": drop the edits and run the deferred action.
    fn guard_discard(&mut self) {
        self.execute_pending();
    }

    /// Guard "Cancel": keep the current document, drop the deferred action.
    fn guard_cancel(&mut self) {
        self.projects.current_mut().pending_action = None;
    }

    fn mark_dirty(&mut self) {
        self.projects.current_mut().mark_dirty();
        self.update_title();
    }

    fn update_title(&mut self) {
        if self.projects.active_id().is_none() {
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let marker = if self.projects.current().is_dirty() {
            " *"
        } else {
            ""
        };
        window.set_title(&format!(
            "Pyxross Workshop — {}{}",
            self.projects.current().name,
            marker
        ));
    }

    /// Modal for the New… dialog: width/height/tile fields + Create/Cancel.
    fn new_canvas_dialog(&mut self) {
        let ctx = self.ctx.clone();
        egui::Modal::new(egui::Id::new("new_canvas_dialog")).show(&ctx, |ui| {
            ui.heading("New Canvas");
            ui.add(
                egui::DragValue::new(&mut self.projects.current_mut().new_draft_w)
                    .range(1..=4096)
                    .prefix("Width: "),
            );
            ui.add(
                egui::DragValue::new(&mut self.projects.current_mut().new_draft_h)
                    .range(1..=4096)
                    .prefix("Height: "),
            );
            ui.add(
                egui::DragValue::new(&mut self.projects.current_mut().tile_size)
                    .range(1..=128)
                    .prefix("Tile: "),
            );
            ui.horizontal(|ui| {
                if ui.button("Create").clicked() {
                    let (w, h, tile) = (
                        self.projects.current_mut().new_draft_w,
                        self.projects.current_mut().new_draft_h,
                        self.projects.current().tile_size,
                    );
                    self.create_new_canvas(w, h, tile);
                    self.projects.current_mut().dialog_open = false;
                }
                if ui.button("Cancel").clicked() {
                    self.projects.current_mut().dialog_open = false;
                }
            });
        });
    }

    /// Modal for the unsaved-changes guard: Save / Discard / Cancel.
    fn unsaved_guard_dialog(&mut self) {
        let ctx = self.ctx.clone();
        egui::Modal::new(egui::Id::new("unsaved_guard_dialog")).show(&ctx, |ui| {
            ui.heading("Unsaved Changes");
            ui.label(format!(
                "{} has unsaved changes.",
                self.projects.current().name
            ));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.guard_save();
                }
                if ui.button("Discard").clicked() {
                    self.guard_discard();
                }
                if ui.button("Cancel").clicked() {
                    self.guard_cancel();
                }
            });
        });
    }

    /// R6 F3: write a separate autosave copy under `autosave_base` and record
    /// it in the recovery journal. Never touches `current_path` or `modified`.
    fn perform_autosave(&mut self) {
        let Some(base) = self.autosave_base.clone() else {
            return;
        };
        let doc = self.app_to_document();
        let project = self.projects.current();
        let project_id = project.id;
        let project_name = project.name.clone();
        let project_dir = project.path.clone();
        let journal_base = recovery_base_for(&base, project_id);
        let autosave_dir =
            autosave_dir_for(&base, &format!("{}-{}", project_id.raw(), project_name));
        match save_document(&doc, &autosave_dir) {
            Ok(()) => {
                let journal = RecoveryJournal {
                    format_version: FORMAT_VERSION,
                    project_dir,
                    autosave_dir,
                    project_name,
                    saved_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0),
                };
                match write_recovery_journal(&journal_base, &journal) {
                    Ok(()) => {
                        let session = self.projects.current_mut();
                        session.recovery = Some(journal);
                        session.recovery_pending = true;
                        session.autosave_generation = session.autosave_generation.wrapping_add(1);
                        session.last_autosave = Some(Instant::now());
                        session.last_error = None;
                    }
                    Err(err) => {
                        self.projects.current_mut().last_error =
                            Some(format!("Autosave failed: {err}"));
                    }
                }
            }
            Err(err) => {
                self.projects.current_mut().last_error = Some(format!("Autosave failed: {err}"));
            }
        }
    }

    /// R6 F4: modal offering to restore the journaled autosave after a crash.
    fn recovery_dialog(&mut self) {
        let ctx = self.ctx.clone();
        egui::Modal::new(egui::Id::new("recovery_dialog")).show(&ctx, |ui| {
            ui.heading("Recover Unsaved Work?");
            let name = self
                .projects
                .current()
                .recovery
                .as_ref()
                .map(|r| r.project_name.clone())
                .unwrap_or_default();
            ui.label(format!("Pyxross found unsaved work from {name}."));
            ui.horizontal(|ui| {
                if ui.button("Restore").clicked() {
                    self.recovery_restore();
                }
                if ui.button("Discard").clicked() {
                    self.recovery_discard();
                }
            });
        });
    }

    /// R6 F4: load the journaled autosave, restore project identity, keep the
    /// document modified, and delete the journal.
    fn recovery_restore(&mut self) {
        let Some(journal) = self.projects.current_mut().recovery.clone() else {
            return;
        };
        match load_document(&journal.autosave_dir) {
            Ok(doc) => {
                self.document_to_app(&doc);
                {
                    let session = self.projects.current_mut();
                    session.path = journal.project_dir.clone();
                    session.name = journal.project_name.clone();
                }
                self.projects.current_mut().mark_dirty();
                self.projects.current_mut().last_error = None;
                self.clear_active_recovery();
                self.update_title();
            }
            Err(err) => {
                self.projects.current_mut().last_error = Some(format!("Recovery failed: {err}"));
            }
        }
    }

    /// R6 F4: delete the journal and close the prompt, keeping the document.
    fn recovery_discard(&mut self) {
        self.clear_active_recovery();
    }

    // -----------------------------------------------------------------------
    // R7: export suite + PNG import
    // -----------------------------------------------------------------------

    fn export_selected_bytes(&self) -> Result<(usize, usize, Vec<u8>), String> {
        let Some(sel) = &self.projects.current().selection else {
            return Err("No selection to export".to_string());
        };
        let rect = sel.rect();
        let composite = self
            .projects
            .current()
            .layers
            .composite_layers_region(rect)
            .ok_or_else(|| "Selection is empty".to_string())?;
        Ok((
            composite.width(),
            composite.height(),
            composite.as_bytes().to_vec(),
        ))
    }

    fn import_png_bytes(&mut self, img: &crate::io::PngImage) -> Result<(), String> {
        let project = self.projects.current();
        let canvas_w = project.layers.width();
        let canvas_h = project.layers.height();
        let w = img.width.min(canvas_w);
        let h = img.height.min(canvas_h);
        if w == 0 || h == 0 {
            return Err("Imported image is empty".to_string());
        }
        let source_stride = img
            .width
            .checked_mul(4)
            .ok_or_else(|| "Imported image is too large".to_string())?;
        let source_len = img
            .width
            .checked_mul(img.height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "Imported image is too large".to_string())?;
        if img.rgba.len() != source_len {
            return Err("Imported image has invalid pixel data".to_string());
        }
        let clipped_len = w
            .checked_mul(h)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "Imported image is too large".to_string())?;
        let clipped_stride = w
            .checked_mul(4)
            .ok_or_else(|| "Imported image is too large".to_string())?;
        let mut clipped = Vec::with_capacity(clipped_len);
        for row in img.rgba.chunks_exact(source_stride).take(h) {
            clipped.extend_from_slice(&row[..clipped_stride]);
        }
        if project.sequence.len() >= MAX_FRAMES {
            return Err("Project frame limit reached".to_string());
        }
        let rect = Rect2i::new(0, 0, w as i32, h as i32);
        let frame_name = format!("Frame {}", project.sequence.len() + 1);
        let mut next_sequence = project.sequence.clone();
        if !next_sequence.push(Frame::new(Region::new(rect, frame_name), 100)) {
            return Err("Project frame limit reached".to_string());
        }
        let delays: Vec<u32> = next_sequence.iter().map(|f| f.delay_ms()).collect();
        let next_animation = AnimationController::new_from_delays(next_sequence.len(), delays);
        let mut imported_buffer = PixelBuffer::new(canvas_w, canvas_h);
        if !imported_buffer.blit_region(rect, &clipped) {
            return Err("Failed to stage imported layer".to_string());
        }
        let selection = Selection::capture(&imported_buffer, rect)
            .ok_or_else(|| "Failed to capture imported selection".to_string())?;
        let layer_id = self
            .projects
            .current_mut()
            .layers
            .add_layer_with_region("Imported", rect, &clipped)
            .ok_or_else(|| "Failed to create imported layer".to_string())?;
        self.projects.current_mut().layers.set_active(layer_id);
        let project = self.projects.current_mut();
        project.sequence = next_sequence;
        project.animation = next_animation;
        project.selection = Some(selection);
        project.mark_dirty();
        self.invalidate_host_project_cache();
        Ok(())
    }

    fn file_export_sprite_sheet(&mut self) {
        let composite = self.projects.current_mut().layers.composite_layers();
        let (w, h) = (composite.width(), composite.height());
        let png = match encode_png(w, h, composite.as_bytes()) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to export sprite sheet: {err}"));
                return;
            }
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name("sprite_sheet.png")
            .save_file()
        else {
            return;
        };
        let meta = match sprite_sheet_meta_json(&self.app_to_document()) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to export sprite sheet metadata: {err}"));
                return;
            }
        };
        match std::fs::write(&path, png)
            .and_then(|_| std::fs::write(path.with_extension("json"), meta))
        {
            Ok(()) => self.projects.current_mut().last_error = None,
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to export sprite sheet: {err}"));
            }
        }
    }

    fn file_export_gif(&mut self) {
        let mut frames = Vec::new();
        let project = self.projects.current_mut();
        for frame in project.sequence.iter() {
            let rect = frame.region().rect();
            let Some(composite) = project.layers.composite_layers_region(rect) else {
                project.last_error = Some("Frame has no pixels".to_string());
                return;
            };
            frames.push(GifFrameInput {
                width: composite.width(),
                height: composite.height(),
                rgba: composite.as_bytes().to_vec(),
                delay_ms: frame.delay_ms(),
            });
        }
        let gif = match encode_gif(&frames) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to export GIF: {err}"));
                return;
            }
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("GIF", &["gif"])
            .set_file_name("animation.gif")
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, gif) {
            Ok(()) => self.projects.current_mut().last_error = None,
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to export GIF: {err}"));
            }
        }
    }

    fn file_export_selected(&mut self) {
        let (w, h, rgba) = match self.export_selected_bytes() {
            Ok(v) => v,
            Err(msg) => {
                self.projects.current_mut().last_error = Some(msg);
                return;
            }
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name("selection.png")
            .save_file()
        else {
            return;
        };
        match encode_png(w, h, &rgba)
            .and_then(|bytes| std::fs::write(&path, bytes).map_err(crate::io::PngError::Io))
        {
            Ok(()) => self.projects.current_mut().last_error = None,
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to export selection: {err}"));
            }
        }
    }

    /// Records the import error in `last_error`; every other session and host
    /// field remains unchanged when decode or staging fails.
    fn file_import_png_path(&mut self, path: &std::path::Path) {
        match std::fs::read(path) {
            Ok(bytes) => self.file_import_png_bytes(&bytes),
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to import PNG: {err}"));
            }
        }
    }

    fn file_import_png_image(&mut self, image: &crate::io::PngImage) {
        if let Err(msg) = self.import_png_bytes(image) {
            self.projects.current_mut().last_error = Some(msg);
        }
    }

    fn file_import_png_bytes(&mut self, bytes: &[u8]) {
        match decode_png(bytes) {
            Ok(img) => self.file_import_png_image(&img),
            Err(err) => {
                self.projects.current_mut().last_error =
                    Some(format!("Failed to import PNG: {err}"));
            }
        }
    }

    fn file_import_png(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .pick_file()
        else {
            return;
        };
        self.file_import_png_path(&path);
    }

    fn ui_frame(&mut self, ui: &mut egui::Ui) {
        debug_assert!(self.panel_layout.validate().is_ok());
        debug_assert!(self.workspace_layout.validate().is_ok());
        debug_assert!(self.host_registry.is_consistent());
        let context_generation = self.context_generation.current();
        debug_assert!(self.context_generation.accepts(context_generation));
        if let Some(snapshot) = self.context_snapshot() {
            debug_assert!(snapshot.accepts(snapshot.project(), snapshot.generation()));
        }
        if self.projects.active_id().is_none() {
            return;
        }
        // Consume any async OS clipboard read (G3 finding #6) before the
        // shortcuts run, so a Ctrl+V result opens its paste session promptly
        // without the render thread ever blocking on `get_image()`.
        self.poll_os_paste();
        self.sync_skin_atlas();
        self.handle_shortcuts();

        let frame_rect = ui.max_rect();
        let pointer = ui.ctx().input(|input| input.pointer.latest_pos());
        let pointer_inside_window = pointer.is_some_and(|p| {
            ui.ctx()
                .input(|input| input.raw.screen_rect.is_some_and(|rect| rect.contains(p)))
        });
        self.input_capture.release_if_outside(pointer_inside_window);
        let target = pointer.filter(|p| frame_rect.contains(*p)).map(|p| {
            if self.panel_dock_demo {
                self.panel_dock
                    .surface_at(p, frame_rect)
                    .map_or(SurfaceId::Canvas, SurfaceId::Panel)
            } else {
                SurfaceId::Canvas
            }
        });
        self.input_capture.set_target(target);

        self.refresh_transform_preview();
        let canvas_size = (
            self.projects.current_mut().layers.width() as u32,
            self.projects.current_mut().layers.height() as u32,
        );
        let texture_id = self.canvas_texture.as_ref().map(|t| t.id());
        if let Some(texture_id) = texture_id {
            let camera = self.projects.current().camera;
            let tool = self.projects.current().tool_state.tool();
            let selection_clip =
                Self::selection_pixel_clip(self.projects.current().selection.as_ref());
            // A Fieldier box gesture is live while it is dragging a marquee, an
            // area move or a grid-cell region; the canvas then clamps its drag
            // so the gesture keeps extending after the pointer leaves the window.
            let selection_drag_active = {
                let session = self.projects.current();
                matches!(tool, Tool::Fieldier) && session.gesture.is_dragging_box()
            };
            // A floating SELECTION transform is the live canvas gesture
            // whenever the session exists: every primary canvas drag is routed
            // to it (the curve variant is handled separately), so the canvas
            // must map the pointer CLAMP-FREE and let the object leave the
            // canvas bounds. Fieldier marquee drags keep the clamped mapping.
            let transform_drag_active = self
                .projects
                .current()
                .transform
                .as_ref()
                .is_some_and(|t| matches!(t, TransformSession::Selection(_)));
            let marquee_probe = self.marquee_probe_at(ui.ctx(), camera, canvas_size);
            let view = CanvasView {
                canvas_size,
                grid_visible: self.projects.current().grid_visible,
                tile_size: self.projects.current().tile_size,
                brush: matches!(tool, Tool::Pencil | Tool::Eraser | Tool::Draw).then(|| {
                    let settings = self.projects.current().draw_settings(tool);
                    BrushSpec::sanitize(settings.size, settings.shape)
                }),
                draw_mode: match tool {
                    Tool::Eraser => DrawMode::Eraser,
                    _ => DrawMode::Pen,
                },
                brush_color: self.projects.current().color,
                line: match (self.line_mode, self.line_anchor, self.line_end) {
                    (true, Some(anchor), Some(end)) => Some(LinePreview { anchor, end }),
                    _ => None,
                },
                selection_clip,
                selection_drag_active,
                transform_drag_active,
                marquee_probe,
                canvas_color_at: marquee_probe,
            };
            self.curve_hovered = self.curve_hover_at(ui.ctx(), camera, canvas_size);
            self.canvas_widget.set_overlay(self.current_overlay());
            let interactions = self.canvas_widget.ui(
                ui,
                &self.theme.current().colors,
                &self.input_capture,
                texture_id,
                view,
                camera,
            );
            self.gizmo_hovered = self.canvas_widget.gizmo_hovered();
            // Transform cursor: while a Selection session is live the cursor
            // follows the gizmo under the pointer — the grabbed handle while
            // dragging, the hovered handle otherwise — and its orientation
            // follows the object's rotation. The body is Grab while hovered and
            // Grabbing while dragged; the rotate/pivot zones stay Crosshair.
            // `None` (and no session) leaves the default arrow.
            let transform_cursor = self
                .projects
                .current()
                .transform
                .as_ref()
                .and_then(TransformSession::selection)
                .and_then(|t| {
                    let dragging = t.drag != GizmoHit::None;
                    let hit = if dragging { t.drag } else { self.gizmo_hovered };
                    if dragging && hit == GizmoHit::Translate {
                        Some(egui::CursorIcon::Grabbing)
                    } else {
                        cursor_for_hit_at_angle(hit, t.object.angle_deg)
                    }
                });
            if let Some(cursor) = transform_cursor {
                self.ctx.output_mut(|o| o.cursor_icon = cursor);
            }
            self.projects.current_mut().camera = interactions.updated_camera;
            let interactions = self.update_tool_gesture(ui.ctx(), interactions);
            self.handle_interactions(interactions);
        }
        let preview_image = self.canvas_texture.as_ref().map(|texture| {
            (
                texture.id(),
                egui::vec2(canvas_size.0 as f32, canvas_size.1 as f32),
            )
        });
        self.panel_dock.preview_feed().set(preview_image);
        if ui.ctx().input(|input| input.key_pressed(egui::Key::F12)) {
            self.panel_dock_demo = !self.panel_dock_demo;
        }
        // Per-frame snapshots for the dock panels (Toolbox + Layers): the
        // panels are pure views over typed cells the App writes before render.
        self.write_dock_snapshots();
        self.sync_layer_settings_panel();
        if self.panel_dock_demo {
            let chrome = PanelChrome {
                colors: &self.theme.current().colors,
                theme: self.theme.current(),
                atlases: self.skin_atlas_cache.as_ref().map(|(_, _, cache)| cache),
                capture: &self.input_capture,
                surface: SurfaceId::Canvas,
            };
            self.panel_dock.show_inside(ui, &chrome);
            // Drain every frame so dock actions cannot accumulate; Close
            // actions from panel headers are applied here.
            let actions = self.panel_dock.drain_actions();
            self.handle_dock_actions(actions);
            // Drain the dock panel events into the model/undo exactly once
            // per frame (the panels only ever push into the host cells).
            self.drain_dock_events();
        }
    }

    /// (Re)load the skin atlases for the active theme.
    ///
    /// Resolves each distinct skin atlas path against the theme dir, decodes
    /// the PNG, and uploads it as a NEAREST texture. The cache is keyed by
    /// (theme name, theme dir) so it is rebuilt only when the theme changes;
    /// load failures are logged and the chrome falls back to flat colors.
    fn sync_skin_atlas(&mut self) {
        let Some(dir) = self.theme.current_dir().map(Path::to_path_buf) else {
            self.skin_atlas_cache = None;
            return;
        };
        let theme = self.theme.current();
        let mut paths: Vec<PathBuf> = theme
            .skins
            .values()
            .map(|skin| PathBuf::from(&skin.atlas_path))
            .collect();
        paths.sort();
        paths.dedup();
        if paths.is_empty() {
            self.skin_atlas_cache = None;
            return;
        }
        let key = (theme.name.clone(), dir);
        if let Some((cached_name, cached_dir, _)) = &self.skin_atlas_cache {
            if cached_name == &key.0 && cached_dir == &key.1 {
                return;
            }
        }
        let mut cache = AtlasCache::new();
        for path in &paths {
            let resolved = key.1.join(path);
            match load_atlas_with_size(&resolved) {
                Ok((bytes, size)) => {
                    let image = egui::ColorImage::from_rgba_unmultiplied(
                        [size.0 as usize, size.1 as usize],
                        &bytes,
                    );
                    let texture = self.ctx.load_texture(
                        format!("skin-atlas:{}", resolved.display()),
                        image,
                        egui::TextureOptions::NEAREST,
                    );
                    cache.insert(path.clone(), SkinAtlas::new(texture, size));
                }
                Err(error) => {
                    eprintln!(
                        "pyxross: skin atlas load failed for {}: {error}",
                        resolved.display()
                    );
                }
            }
        }
        self.skin_atlas_cache = Some((key.0, key.1, cache));
    }

    fn redraw(&mut self) {
        self.sync_texture();

        let raw_input = {
            let Some(window) = self.window.as_ref() else {
                return;
            };
            let Some(state) = &mut self.egui_state else {
                return;
            };
            state.take_egui_input(window)
        };

        let ctx = self.ctx.clone();
        // Advance playback by the real frame delta (U13). The timeline
        // panel updates controller state only; canvas rendering is a
        // separate unit.
        let dt = (ctx.input(|i| i.unstable_dt) * 1000.0) as u32;
        if self.projects.active_id().is_some() {
            self.tick_animation(dt);
        }
        if self.projects.active_id().is_some()
            && self.autosave_base.is_some()
            && should_autosave(
                self.projects.current().is_dirty(),
                self.projects.current_mut().last_autosave,
                Instant::now(),
                AUTOSAVE_INTERVAL,
            )
        {
            self.perform_autosave();
        }
        let mut full_output = ctx.run_ui(raw_input, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| self.ui_frame(ui));
        });

        {
            let Some(window) = self.window.as_ref() else {
                return;
            };
            let Some(state) = &mut self.egui_state else {
                return;
            };
            state.handle_platform_output(window, full_output.platform_output);
        }

        let paint_jobs = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

        if let Some(renderer) = &mut self.renderer {
            let Some(window) = self.window.as_ref() else {
                return;
            };
            let _ = renderer.render_frame(
                window,
                &paint_jobs,
                &full_output.textures_delta,
                self.theme.current().colors.clear_color32(),
            );
        }
        full_output.textures_delta.clear();

        // Live loop: keep the canvas repainting so strokes and zoom stay smooth.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

/// Axis-aligned rect spanning two canvas points, both inclusive.
fn rect_from_points(a: (i32, i32), b: (i32, i32)) -> Rect2i {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    let w = (a.0 - b.0).abs() + 1;
    let h = (a.1 - b.1).abs() + 1;
    Rect2i::new(x, y, w, h)
}

/// R6 F3: true when a modified document is due for an autosave (no prior
/// autosave, or the interval has elapsed since the last one).
fn should_autosave(
    modified: bool,
    last: Option<Instant>,
    now: Instant,
    interval: Duration,
) -> bool {
    if !modified {
        return false;
    }
    match last {
        None => true,
        Some(t) => now.duration_since(t) >= interval,
    }
}

/// The canvas top-left an object of `size` is pasted at so that it is CENTRED
/// on the cursor pixel (Part B): `cursor − size/2` (integer division).
fn paste_origin_for(cursor: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    (cursor.0 - size.0 / 2, cursor.1 - size.1 / 2)
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window_attrs = winit::window::WindowAttributes::default()
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0))
            .with_title("Pyxross Workshop");
        let window = event_loop
            .create_window(window_attrs)
            .expect("Failed to create window");

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(
            Box::new(event_loop.owned_display_handle()),
        ));

        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window).unwrap())
        }
        .unwrap();

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .expect("Failed to get adapter");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("main"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .expect("Failed to request device");

        let config = surface
            .get_default_config(&adapter, 1280, 720)
            .expect("Surface config unavailable");
        surface.configure(&device, &config);

        let renderer = RendererState::new(device, queue, surface, config);

        let egui_state = egui_winit::State::new(
            self.ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            None,
            None,
            None,
        );

        let composite = self.projects.current_mut().layers.composite_layers();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [composite.width(), composite.height()],
            composite.as_bytes(),
        );
        let texture = self
            .ctx
            .load_texture("canvas", image, egui::TextureOptions::NEAREST);

        self.window = Some(window);
        self.renderer = Some(renderer);
        self.egui_state = Some(egui_state);
        self.canvas_texture = Some(texture);
        self.texture_dirty = false;
        // The egui-winit stack ships the `clipboard` feature (arboard), so
        // `Context::copy_image` reaches the OS image clipboard on this host.
        self.projects.current_mut().os_clipboard_available = true;
        self.canvas_cache_token = self.projects.activation_token();
        if let Some(path) = workspace_layout_path() {
            if let Ok(layout) = WorkspaceLayoutV1::load_or_default(&path) {
                self.workspace_layout = layout.normalized_for_startup();
            }
        }
        self.update_title();
        self.autosave_base = dirs::config_dir().map(|d| d.join("pyxross"));
        let recovery = self
            .autosave_base
            .as_deref()
            .map(|base| {
                let journal_base = recovery_base_for(base, self.projects.current().id);
                read_recovery_journal(&journal_base)
            })
            .flatten();
        self.projects.current_mut().recovery = recovery;

        // Continuous polling keeps input latency minimal for live strokes.
        event_loop.set_control_flow(ControlFlow::Poll);
        self.service_pending_native_hosts(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_some_and(|window| window.id() == window_id)
        {
            let response = match (&self.window, &mut self.egui_state) {
                (Some(window), Some(state)) => Some(state.on_window_event(window, &event)),
                _ => None,
            };
            if response.is_some_and(|response| response.repaint) {
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
            }
            match event {
                WindowEvent::CloseRequested => {
                    if let Some(project) = self.projects.active_id() {
                        if self.projects.current().is_dirty() {
                            self.projects.current_mut().pending_action =
                                Some(PendingAction::Close { project });
                            self.redraw();
                        } else {
                            self.sync_workspace_layout();
                            self.save_workspace_layout();
                            event_loop.exit();
                        }
                    } else {
                        self.sync_workspace_layout();
                        self.save_workspace_layout();
                        event_loop.exit();
                    }
                }
                WindowEvent::Resized(size) => {
                    if let Some(renderer) = &mut self.renderer {
                        renderer.resize(size.width, size.height);
                    }
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    if let Some(window) = self.window.as_ref() {
                        window.request_redraw();
                    }
                }
                WindowEvent::RedrawRequested => self.redraw(),
                _ => {}
            }
            return;
        }

        let Some(binding) = self.host_registry.resolve_window(window_id) else {
            return;
        };
        let host = binding.host();
        if let Some(native) = self.native_hosts.get_mut(&host) {
            if native.handle_window_event(&event) {
                native.request_redraw();
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                let action = self
                    .workspace_runtime
                    .os_close_action_for_window(window_id, &self.host_registry);
                if let Some(action) = action {
                    self.defer_workspace_action(action);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(native) = self.native_hosts.get_mut(&host) {
                    native.resize(size.width, size.height);
                    native.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(native) = self.native_hosts.get(&host) {
                    native.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw_native_host(host),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.native_host_retirement.advance();
        self.apply_deferred_workspace_actions();
        self.service_pending_native_hosts(event_loop);
    }
}
impl App {
    // ------------------------------------------------------------------
    // U13: timeline + playback events (R3 wave, feature F3)
    // ------------------------------------------------------------------

    /// Apply timeline panel gestures to the animation state. The timeline is
    /// a pure view (D59/D62); the App owns the sequence and controller and
    /// applies each event as an action.
    fn apply_timeline_events(&mut self, events: Vec<TimelineEvent>) {
        for event in events {
            match event {
                TimelineEvent::SelectFrame(index) => {
                    self.projects.current_mut().animation.goto(index)
                }
                TimelineEvent::SetDuration { index, ms } => {
                    if let Some(frame) = self.projects.current_mut().sequence.frame_mut(index) {
                        frame.set_delay_ms(ms);
                    }
                    self.sync_anim_delays();
                    self.mark_dirty();
                }
                TimelineEvent::Reorder { from, to } => {
                    if from == to {
                        continue;
                    }
                    let to = clamp_reorder_target(to, self.projects.current_mut().sequence.len());
                    if self.projects.current_mut().sequence.reorder(from, to) {
                        self.sync_anim_delays();
                        self.mark_dirty();
                    }
                }
                TimelineEvent::AddFrame => {
                    let region = self
                        .projects
                        .current()
                        .sequence
                        .frame(self.projects.current().animation.current_index())
                        .map(|f| f.region().clone())
                        .unwrap_or_else(|| {
                            Region::new(
                                Rect2i::new(0, 0, CANVAS_WIDTH as i32, CANVAS_HEIGHT as i32),
                                "Frame 1",
                            )
                        });
                    let project = self.projects.current_mut();
                    if project.sequence.push(Frame::new(region, 100)) {
                        self.sync_anim_delays();
                        let project = self.projects.current_mut();
                        project.animation.set_frame_count(project.sequence.len());
                        project.animation.goto(project.sequence.len() - 1);
                        self.mark_dirty();
                    }
                }
                TimelineEvent::RemoveFrame(index) => {
                    if self.projects.current_mut().sequence.len() <= 1 {
                        continue;
                    }
                    if self.projects.current_mut().sequence.remove(index).is_some() {
                        self.sync_anim_delays();
                        self.mark_dirty();
                    }
                }
                TimelineEvent::ToggleLoop => {
                    let on = !self.projects.current_mut().sequence.looping();
                    self.projects.current_mut().sequence.set_looping(on);
                    self.projects.current_mut().animation.set_looping(on);
                    self.mark_dirty();
                }
                TimelineEvent::Play => self.projects.current_mut().animation.play(),
                TimelineEvent::Stop => self.projects.current_mut().animation.stop(),
                TimelineEvent::Restart => self.projects.current_mut().animation.restart(),
            }
        }
    }

    /// Mirror the sequence's per-frame delays into the controller.
    fn sync_anim_delays(&mut self) {
        let project = self.projects.current_mut();
        let delays: Vec<u32> = project.sequence.iter().map(|f| f.delay_ms()).collect();
        project.animation.set_delays(delays);
        project.animation.set_frame_count(project.sequence.len());
    }

    /// Advance playback by one frame delta (called from the redraw path).
    fn tick_animation(&mut self, dt_ms: u32) {
        self.projects.current_mut().animation.advance(dt_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::brush::BrushShape;
    use crate::core::color::Color;
    use crate::core::model::LayerId;
    use crate::core::palette::Palette;
    use crate::core::transform::TransformAlgorithm;
    use crate::input::{FieldierChild, WandSettings};
    use crate::io::decode_png;
    use crate::ui::project::ProjectId;
    use std::cell::Cell;
    use std::rc::Rc;

    const RED: Color = Color::rgb(255, 0, 0);

    #[test]
    fn native_host_retirement_waits_one_event_turn_and_releases_once() {
        struct DropProbe(Rc<Cell<usize>>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }

        // Given: native host ownership is enqueued for deferred retirement.
        let releases = Rc::new(Cell::new(0));
        let owner = Rc::new(DropProbe(Rc::clone(&releases)));
        let mut retirement = NativeHostRetirement::new();
        retirement.enqueue(Rc::clone(&owner));

        // Then: the enqueue turn still owns the host.
        assert_eq!(Rc::strong_count(&owner), 2);
        assert_eq!(releases.get(), 0);

        // When: the event loop advances exactly once.
        retirement.advance();

        // Then: ownership is released exactly once after that turn.
        assert_eq!(Rc::strong_count(&owner), 1);
        assert_eq!(releases.get(), 0);
        drop(owner);
        assert_eq!(releases.get(), 1);
        retirement.advance();
        assert_eq!(releases.get(), 1);
    }

    fn rendered_texts(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        fn visit(shape: &egui::Shape, texts: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(text) => texts.push((text.galley.text().to_string(), text.pos)),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit(shape, texts)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        output
            .shapes
            .iter()
            .for_each(|clipped| visit(&clipped.shape, &mut texts));
        texts
    }

    #[test]
    fn native_workspace_panel_uses_canonical_header_and_close_action() {
        let mut app = App::default();
        let host = app
            .host_registry
            .create(
                PanelId::PanelA,
                app.workspace_runtime
                    .viewport_id(PanelId::PanelA)
                    .expect("Panel A viewport"),
            )
            .expect("native host identity");
        let context = egui::Context::default();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.workspace_panel_host_ui(ui, PanelId::PanelA, host, None);
            });
        });
        output.textures_delta.clear();
        let texts = rendered_texts(&output);
        assert!(texts.iter().any(|(text, _)| text == "Panel A"));
        assert!(texts.iter().any(|(text, _)| text == "Close"));
    }

    #[test]
    fn native_workspace_panel_hides_project_context_text() {
        let mut app = App::default();
        let host = app
            .host_registry
            .create(
                PanelId::PanelA,
                app.workspace_runtime
                    .viewport_id(PanelId::PanelA)
                    .expect("Panel A viewport"),
            )
            .expect("native host identity");
        let context = egui::Context::default();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        };
        let mut output = context.run_ui(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.workspace_panel_host_ui(ui, PanelId::PanelA, host, None);
            });
        });
        output.textures_delta.clear();
        let texts = rendered_texts(&output);
        assert!(
            !texts.iter().any(|(text, _)| text.starts_with("Project:")),
            "{texts:?}"
        );
        assert!(
            !texts.iter().any(|(text, _)| text.starts_with("Canvas:")),
            "{texts:?}"
        );
    }

    #[test]
    fn native_workspace_panel_defers_close_until_after_ui_callback() {
        let mut app = App::default();
        app.apply_workspace_action(WorkspacePanelAction::PopOut(PanelId::PanelA))
            .expect("Panel A should pop out");
        let host = app
            .host_registry
            .resolve_panel(PanelId::PanelA)
            .expect("native host")
            .host();
        let context = egui::Context::default();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        };
        let mut dock_action = None;
        let mut output = context.run_ui(raw_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                dock_action = app.workspace_panel_host_ui(ui, PanelId::PanelA, host, None);
            });
        });
        output.textures_delta.clear();
        let dock_position = rendered_texts(&output)
            .into_iter()
            .find_map(|(text, position)| (text == "Close").then_some(position))
            .expect("Close action should render");

        for event in [
            egui::Event::PointerMoved(dock_position + egui::vec2(4.0, 8.0)),
            egui::Event::PointerButton {
                pos: dock_position + egui::vec2(4.0, 8.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: dock_position + egui::vec2(4.0, 8.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ] {
            let mut action = None;
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 480.0),
                    )),
                    events: vec![event],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        action = app.workspace_panel_host_ui(ui, PanelId::PanelA, host, None);
                    });
                },
            );
            output.textures_delta.clear();
            if action.is_some() {
                dock_action = action;
            }
        }

        assert_eq!(
            app.workspace_runtime
                .state(PanelId::PanelA, &app.panel_layout, &app.host_registry),
            Ok(WorkspacePanelState::Main {
                placement: PanelPlacement::Docked,
                native_host: Some(host)
            })
        );
        app.apply_workspace_action(
            dock_action.expect("Close click should be returned to the caller"),
        )
        .expect("Dock action should apply after the UI callback");
        assert!(matches!(
            app.workspace_runtime
                .state(PanelId::PanelA, &app.panel_layout, &app.host_registry),
            Ok(WorkspacePanelState::Main { .. })
        ));
    }

    #[test]
    fn native_workspace_actions_are_applied_after_event_callback() {
        let mut app = App::default();
        app.apply_workspace_action(WorkspacePanelAction::PopOut(PanelId::PanelA))
            .expect("Panel A should pop out");

        app.defer_workspace_action(WorkspacePanelAction::DockBack(PanelId::PanelA));
        assert!(matches!(
            app.workspace_runtime
                .state(PanelId::PanelA, &app.panel_layout, &app.host_registry),
            Ok(WorkspacePanelState::Main {
                native_host: Some(_),
                ..
            })
        ));

        app.apply_deferred_workspace_actions();
        assert!(matches!(
            app.workspace_runtime
                .state(PanelId::PanelA, &app.panel_layout, &app.host_registry),
            Ok(WorkspacePanelState::Main { .. })
        ));
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("pyx_task2_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(tag: &str, bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!("pyx_task2_{tag}_{}", std::process::id()));
            let _ = std::fs::remove_file(&path);
            std::fs::write(&path, bytes).unwrap();
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Seeds a solid RED rect into the active layer buffer.
    fn seed_red_rect(app: &mut App, rect: Rect2i) {
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                app.projects
                    .current_mut()
                    .layers
                    .active_layer_mut()
                    .buffer
                    .set_pixel(x as usize, y as usize, RED);
            }
        }
    }

    #[derive(Debug, PartialEq)]
    struct ImportState {
        document: String,
        id: ProjectId,
        name: String,
        path: Option<PathBuf>,
        tile_size: u32,
        layer_active: LayerId,
        layer_changed: bool,
        layer_count: usize,
        undo: (usize, usize, Option<String>),
        selection: Option<(Rect2i, Vec<u8>)>,
        selection_snapshot: Option<(Rect2i, Vec<u8>)>,
        marquee: (
            Option<(i32, i32)>,
            Option<Rect2i>,
            Option<(i32, i32)>,
            Option<Rect2i>,
        ),
        clipboard: Option<crate::core::clipboard::ClipboardRegion>,
        stroke: Option<(LayerId, Rect2i, Rect2i)>,
        transform: Option<String>,
        sequence: String,
        animation: String,
        palettes: String,
        active_palette: usize,
        color: Color,
        secondary_color: Color,
        tool: (Tool, Option<Tool>),
        pen_draw_settings: DrawSettings,
        eraser_draw_settings: DrawSettings,
        grid_visible: bool,
        canvas_generation: u64,
        dirty: bool,
        save_generation: u64,
        recovery_key: String,
        recovery_pending: bool,
        autosave_generation: u64,
        new_draft: (u32, u32),
        dialog_open: bool,
        pending_action: String,
        last_autosave: String,
        recovery: Option<RecoveryJournal>,
        camera: crate::core::camera::Camera,
        cache_token: Option<ActivationToken>,
        texture_dirty: bool,
    }

    fn selection_state(selection: Option<&Selection>) -> Option<(Rect2i, Vec<u8>)> {
        selection.map(|value| (value.rect(), value.snapshot().to_vec()))
    }

    fn pending_action_state(action: Option<&PendingAction>) -> String {
        match action {
            None => String::from("none"),
            Some(PendingAction::New {
                project,
                width,
                height,
                tile,
            }) => format!("new:{}:{width}:{height}:{tile}", project.raw()),
            Some(PendingAction::Open { project, path }) => {
                format!("open:{}:{path:?}", project.raw())
            }
            Some(PendingAction::Close { project }) => format!("close:{}", project.raw()),
        }
    }

    fn transform_state(transform: Option<&TransformSession>) -> Option<String> {
        transform.map(|value| match value {
            TransformSession::Selection(t) => format!(
                "selection:{:?}:{:?}:{:?}:{:?}:{:?}",
                t.drag,
                t.last_pt,
                crate::core::transform::LayerCommit {
                    layer_id: t.source.layer_id.as_u64() as usize,
                    dst: (t.source.rect.x as f32, t.source.rect.y as f32),
                    w: t.source.rect.w as usize,
                    h: t.source.rect.h as usize,
                    buf: t.source.snapshot.clone(),
                },
                t.object.commit(),
                t.object.restore(),
            ),
            TransformSession::Curve(c) => format!("curve:{:?}:{:?}", c.drag, c.curve.points()),
        })
    }

    fn import_state(app: &App) -> ImportState {
        let session = app.projects.current();
        ImportState {
            document: format!("{:?}", session.to_document()),
            id: session.id,
            name: session.name.clone(),
            path: session.path.clone(),
            tile_size: session.tile_size,
            layer_active: session.layers.active_layer_id(),
            layer_changed: session.layers.changed(),
            layer_count: session.layers.len(),
            undo: (
                session.undo.undo_len(),
                session.undo.redo_len(),
                session.undo.top_undo_name().map(str::to_string),
            ),
            selection: selection_state(session.selection.as_ref()),
            selection_snapshot: selection_state(session.selection_snapshot.as_ref()),
            marquee: (
                session.gesture.marquee().map(|(origin, _)| origin),
                session.gesture.marquee().map(|(_, rect)| rect),
                match &session.gesture {
                    SelectGesture::AreaMove { origin, .. } => Some(*origin),
                    _ => None,
                },
                session.gesture.move_dest(),
            ),
            clipboard: session.clipboard.clone(),
            stroke: session.stroke.as_ref().map(|value| {
                (
                    value.layer_id(),
                    value.capture_region(),
                    value.bounding_box(),
                )
            }),
            transform: transform_state(session.transform.as_ref()),
            sequence: format!("{:?}", &session.sequence),
            animation: format!("{:?}", &session.animation),
            palettes: format!("{:?}", &session.palettes),
            active_palette: session.active_palette,
            color: session.color,
            secondary_color: session.secondary_color,
            tool: (session.tool_state.tool(), session.tool_state.previous()),
            pen_draw_settings: session.pen_draw_settings,
            eraser_draw_settings: session.eraser_draw_settings,
            grid_visible: session.grid_visible,
            canvas_generation: session.canvas_generation,
            dirty: session.is_dirty(),
            save_generation: session.save_generation,
            recovery_key: session.recovery_key.clone(),
            recovery_pending: session.recovery_pending,
            autosave_generation: session.autosave_generation,
            new_draft: (session.new_draft_w, session.new_draft_h),
            dialog_open: session.dialog_open,
            pending_action: pending_action_state(session.pending_action.as_ref()),
            last_autosave: format!("{:?}", session.last_autosave),
            recovery: session.recovery.clone(),
            camera: session.camera,
            cache_token: app.canvas_cache_token,
            texture_dirty: app.texture_dirty,
        }
    }

    fn assert_import_state_unchanged(app: &App, before: &ImportState) {
        assert_eq!(import_state(app), *before);
    }

    fn png_with_dimensions(width: u32, height: u32) -> Vec<u8> {
        let mut png = encode_png(1, 1, &[255, 255, 255, 255]).unwrap();
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        let mut crc = 0xffff_ffffu32;
        for byte in &png[12..29] {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        png[29..33].copy_from_slice(&(crc ^ 0xffff_ffff).to_be_bytes());
        png
    }

    /// Runs a Select-tool marquee drag from `a` to `b` (inclusive corners).
    fn marquee(app: &mut App, a: (i32, i32), b: (i32, i32)) {
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some(a),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some(b),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some(b),
            ..Default::default()
        });
    }

    /// Sets the held modifiers for the next `handle_interactions` call.
    fn set_modifiers(app: &App, modifiers: egui::Modifiers) {
        app.ctx.input_mut(|i| i.modifiers = modifiers);
    }

    /// Runs a Select-tool marquee drag with held modifiers. The tool is assumed
    /// already active (a deliberate switch would deselect first).
    fn marquee_with(app: &mut App, a: (i32, i32), b: (i32, i32), modifiers: egui::Modifiers) {
        set_modifiers(app, modifiers);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some(a),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some(b),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some(b),
            ..Default::default()
        });
        set_modifiers(app, egui::Modifiers::NONE);
    }

    /// Runs a Select-tool right-button drag from `a` to `b` with `modifiers`
    /// held for the whole gesture. The tool is assumed already active.
    fn right_drag_with(app: &mut App, a: (i32, i32), b: (i32, i32), modifiers: egui::Modifiers) {
        set_modifiers(app, modifiers);
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some(a),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_point: Some(b),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });
        set_modifiers(app, egui::Modifiers::NONE);
    }

    /// A plain right-button drag (no modifiers).
    fn right_drag(app: &mut App, a: (i32, i32), b: (i32, i32)) {
        right_drag_with(app, a, b, egui::Modifiers::NONE);
    }

    #[test]
    fn pencil_stroke_uses_active_color() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((5, 4)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((5, 4)),
            ..Default::default()
        });
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(RED));
        assert_eq!(buf.get_pixel(5, 4), Some(RED));
    }

    #[test]
    fn app_routes_live_editing_to_the_active_project_session() {
        let mut app = App::default();
        let alpha = app.projects.active_id().expect("default project");
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, RED);
        app.projects.current_mut().mark_dirty();
        let _beta = app.new_project("beta", CANVAS_WIDTH, CANVAS_HEIGHT);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(2, 2),
            Some(Color::TRANSPARENT)
        );
        assert!(!app.projects.current().is_dirty());
        app.activate_project(alpha).expect("alpha is open");
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(2, 2),
            Some(RED)
        );
        assert!(app.projects.current().is_dirty());
    }

    #[test]
    fn activating_same_size_clean_project_invalidates_and_rebuilds_canvas_cache() {
        let mut app = App::default();
        let beta = app.new_project("beta", CANVAS_WIDTH, CANVAS_HEIGHT);
        let gamma = app.new_project("gamma", CANVAS_WIDTH, CANVAS_HEIGHT);
        let context = egui::Context::default();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [CANVAS_WIDTH, CANVAS_HEIGHT],
            &vec![0; CANVAS_WIDTH * CANVAS_HEIGHT * 4],
        );
        app.canvas_texture = Some(context.load_texture(
            "activation-cache-test",
            image,
            egui::TextureOptions::NEAREST,
        ));
        app.texture_dirty = false;
        let active_token = app.projects.activation_token().expect("active token");
        app.canvas_cache_token = Some(active_token);
        app.projects.current_mut().layers.clear_changed();
        app.projects
            .session_mut(beta)
            .expect("beta is open")
            .layers
            .clear_changed();
        app.projects
            .session_mut(gamma)
            .expect("gamma is open")
            .layers
            .clear_changed();

        let activation = app.activate_project(beta).expect("beta is open");

        assert!(app.texture_dirty);
        assert_eq!(app.canvas_cache_token, None);
        assert!(activation.changed());
        assert_eq!(activation.token().expect("active token").project(), beta);
        app.sync_texture();
        assert!(!app.texture_dirty);
        assert_eq!(app.canvas_cache_token, app.projects.activation_token());
    }

    #[test]
    fn closing_active_project_consumes_replacement_activation_and_invalidates_cache() {
        let mut app = App::default();
        let alpha = app.projects.active_id().expect("default project");
        let beta = app.new_project("beta", CANVAS_WIDTH, CANVAS_HEIGHT);
        app.sync_texture();
        assert_eq!(app.projects.active_id(), Some(beta));
        app.canvas_cache_token = app.projects.activation_token();
        app.texture_dirty = false;

        let outcome = app
            .close_project(beta, project::CloseProject::Discard)
            .expect("clean project closes");

        assert_eq!(
            outcome
                .activation()
                .and_then(|value| value.token())
                .map(|token| token.project()),
            Some(alpha)
        );
        assert_eq!(app.projects.active_id(), Some(alpha));
        assert_eq!(app.canvas_cache_token, None);
        assert!(app.texture_dirty);
    }

    #[test]
    fn tab_close_request_preserves_dirty_project_and_opens_guard() {
        let mut app = App::default();
        let project = app.projects.active_id().expect("default project");
        app.projects.current_mut().mark_dirty();

        app.request_project_close(project);

        assert!(app.projects.contains(project));
        assert_eq!(app.projects.active_id(), Some(project));
        assert!(matches!(
            app.projects.current().pending_action,
            Some(PendingAction::Close { project: pending }) if pending == project
        ));
    }

    #[test]
    fn eraser_clears_to_transparent() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 1));
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Eraser)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((5, 4)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((5, 4)),
            ..Default::default()
        });
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(5, 4), Some(Color::TRANSPARENT));
    }

    #[test]
    fn live_transform_refresh_uses_bounded_preview_texture() {
        let mut app = App::default();
        app.lift_transform(Rect2i::new(0, 0, 65, 65));
        app.refresh_transform_preview();
        let size = app
            .transform_preview
            .as_ref()
            .map(egui::TextureHandle::size);
        assert_eq!(
            size,
            Some([TRANSFORM_PREVIEW_MAX_DIM, TRANSFORM_PREVIEW_MAX_DIM])
        );
    }

    #[test]
    fn sync_texture_drains_dirty_region_from_non_active_layer() {
        let mut app = App::default();
        let context = egui::Context::default();
        let image = egui::ColorImage::from_rgba_unmultiplied([128, 128], &vec![0; 128 * 128 * 4]);
        app.canvas_texture =
            Some(context.load_texture("sync-test", image, egui::TextureOptions::NEAREST));
        app.texture_dirty = false;
        let non_active = app.projects.current_mut().layers.add_layer("non-active");
        app.projects.current_mut().layers.clear_changed();
        app.projects
            .current_mut()
            .layers
            .layer_mut(non_active)
            .unwrap()
            .buffer
            .set_pixel(9, 11, Color::WHITE);

        app.sync_texture();

        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .layer_mut(non_active)
                .unwrap()
                .buffer
                .take_pixels_changed(),
            None
        );
    }

    #[test]
    fn full_sync_drains_layer_dirty_state_before_returning() {
        let mut app = App::default();
        let context = egui::Context::default();
        let image = egui::ColorImage::from_rgba_unmultiplied([128, 128], &vec![0; 128 * 128 * 4]);
        app.canvas_texture =
            Some(context.load_texture("full-sync-test", image, egui::TextureOptions::NEAREST));
        let non_active = app.projects.current_mut().layers.add_layer("non-active");
        app.projects
            .current_mut()
            .layers
            .layer_mut(non_active)
            .unwrap()
            .buffer
            .set_pixel(9, 11, Color::WHITE);

        app.sync_texture();

        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .layer_mut(non_active)
                .unwrap()
                .buffer
                .take_pixels_changed(),
            None
        );
    }

    #[test]
    fn fill_uses_active_color() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fill)]);
        app.handle_interactions(CanvasInteractions {
            clicked: Some((8, 8)),
            ..Default::default()
        });
        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .active_layer()
                .buffer
                .get_pixel(8, 8),
            Some(RED)
        );
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
    }

    #[test]
    fn click_is_one_pixel_fill_or_noop() {
        let mut app = App::default();
        let before = app
            .projects
            .current_mut()
            .layers
            .active_layer()
            .buffer
            .as_bytes()
            .to_vec();
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });
        assert_eq!(app.projects.current_mut().undo.undo_len(), 0);
        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .active_layer()
                .buffer
                .as_bytes(),
            &before[..]
        );
    }

    #[test]
    fn eyedropper_picks_pixel() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(3, 3, RED);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Eyedropper)]);
        app.handle_interactions(CanvasInteractions {
            clicked: Some((3, 3)),
            ..Default::default()
        });
        assert_eq!(app.projects.current_mut().color, RED);
    }

    #[test]
    fn temporary_eyedropper_restores_tool() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(3, 3, RED);
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((3, 3)),
            ..Default::default()
        });
        assert_eq!(
            app.projects.current_mut().tool_state.tool(),
            Tool::Eyedropper
        );
        assert_eq!(app.projects.current_mut().color, RED);
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            ..Default::default()
        });
        assert_eq!(app.projects.current_mut().tool_state.tool(), Tool::Pencil);
    }

    #[test]
    fn marquee_selects_region() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (5, 6));
        assert_eq!(
            app.projects
                .current_mut()
                .selection
                .as_ref()
                .map(|s| s.rect()),
            Some(Rect2i::new(2, 2, 4, 5))
        );
    }

    #[test]
    fn marquee_reversed_corners_normalize() {
        let mut app = App::default();
        marquee(&mut app, (5, 6), (2, 2));
        assert_eq!(
            app.projects
                .current_mut()
                .selection
                .as_ref()
                .map(|s| s.rect()),
            Some(Rect2i::new(2, 2, 4, 5))
        );
    }

    #[test]
    fn rectangle_marquee_uses_the_mask_capable_selection() {
        let mut app = App::default();
        let undo_before = app.projects.current().undo.undo_len();
        marquee(&mut app, (2, 2), (5, 5));
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a marquee must install the new Selection model");
        assert_eq!(sel.rect(), Rect2i::new(2, 2, 4, 4));
        assert!(
            sel.is_rectangular(),
            "a rectangle uses the mask-capable model's fast path"
        );
        assert_eq!(sel.pixel_count(), 16);
        assert_eq!(
            sel.snapshot().len(),
            16 * 4,
            "the snapshot is captured from the live layer by the new model"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            undo_before,
            "the marquee must not push a legacy cut+paste move command"
        );
    }

    #[test]
    fn click_empty_clears_selection() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (5, 5));
        assert!(app.projects.current_mut().selection.is_some());
        app.handle_interactions(CanvasInteractions {
            clicked: Some((20, 20)),
            ..Default::default()
        });
        assert!(app.projects.current_mut().selection.is_none());
    }

    #[test]
    fn selection_survives_tool_switches_and_painting() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (5, 5));
        let before = app
            .projects
            .current()
            .selection
            .as_ref()
            .map(|sel| (sel.rect(), sel.snapshot().to_vec()));
        assert!(before.is_some());

        // A deliberate switch to the Pen keeps the selection.
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Pencil)]);
        assert!(
            app.projects.current().selection.is_some(),
            "a tool switch must not drop the selection"
        );

        // Painting with the Pen keeps it too, byte for byte. The stamp lands
        // INSIDE the selection: a stroke is clipped to the selection, so
        // painting at (8,8) would write nothing and prove nothing about the
        // selection surviving.
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((3, 3)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((3, 3)),
            ..Default::default()
        });
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(3, 3),
            Some(RED)
        );

        let after = app
            .projects
            .current()
            .selection
            .as_ref()
            .map(|sel| (sel.rect(), sel.snapshot().to_vec()));
        assert_eq!(
            before, after,
            "painting must not change or drop the selection"
        );
    }

    #[test]
    fn escape_cancels_the_selection() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (5, 5));
        assert!(app.projects.current_mut().selection.is_some());
        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(
            app.projects.current_mut().selection.is_none(),
            "Esc must cancel the selection"
        );
    }

    #[test]
    fn area_move_moves_the_selection_not_the_pixels() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        marquee(&mut app, (2, 2), (5, 5));
        let undo_before = app.projects.current().undo.undo_len();

        // Given: a press inside the selection. When: it drags to (11,11).
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((3, 3)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((11, 11)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((11, 11)),
            ..Default::default()
        });

        // Then: only the selection moved — the document is byte-identical. A
        // selection move never cuts or pastes pixels (D77 re-affirms D50/D70
        // and supersedes D76 item 2).
        assert_eq!(
            app.projects
                .current_mut()
                .selection
                .as_ref()
                .map(|s| s.rect()),
            Some(Rect2i::new(10, 10, 4, 4))
        );
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(RED), "the source pixels stay put");
        assert_eq!(
            buf.get_pixel(10, 10),
            Some(Color::TRANSPARENT),
            "nothing is pasted at the destination"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            undo_before,
            "the move must not push an undo step"
        );
    }

    #[test]
    fn dragging_a_mask_selection_shifts_its_mask() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        // A 2×2 diagonal mask at (2,2): only (2,2) and (3,3) are selected.
        let sel = Selection::capture_mask(
            &app.projects.current_mut().layers.active_layer().buffer,
            Rect2i::new(2, 2, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        app.projects.current_mut().selection = Some(sel);
        let undo_before = app.projects.current().undo.undo_len();

        // Press a selected pixel and drag it one pixel down-right.
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((2, 2)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((3, 3)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((3, 3)),
            ..Default::default()
        });

        // The mask translates as a shape while the pixels stay put, unselected
        // pixels inside the source bbox untouched (D77).
        assert_eq!(app.projects.current().undo.undo_len(), undo_before);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(RED), "the source stays");
        assert_eq!(buf.get_pixel(3, 2), Some(RED));
        assert_eq!(buf.get_pixel(2, 3), Some(RED));
        assert_eq!(buf.get_pixel(3, 3), Some(RED));
        assert_eq!(buf.get_pixel(4, 4), Some(RED));
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert_eq!(sel.rect(), Rect2i::new(3, 3, 2, 2));
        assert!(
            !sel.is_rectangular(),
            "the mask shape must survive the move"
        );
        assert!(sel.contains(3, 3));
        assert!(!sel.contains(4, 3), "the diagonal hole stays a hole");
    }

    #[test]
    fn shift_and_right_click_add_and_subtract_from_the_selection() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (3, 3));
        assert_eq!(
            app.projects
                .current_mut()
                .selection
                .as_ref()
                .map(|s| s.rect()),
            Some(Rect2i::new(0, 0, 4, 4))
        );

        // Shift adds a disjoint rect → the union bbox spans both.
        marquee_with(
            &mut app,
            (6, 0),
            (9, 3),
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        let sel = app.projects.current_mut().selection.take().unwrap();
        assert!(sel.contains(0, 0));
        assert!(sel.contains(6, 0));
        assert!(
            !sel.contains(4, 0),
            "the gap between the rects is not selected"
        );
        app.projects.current_mut().selection = Some(sel);

        // Alt+left sweeps grid cells and ADDS them (D77 item 4): it is a
        // region gesture, not a Euclidean marquee and never a subtract.
        marquee_with(
            &mut app,
            (4, 0),
            (7, 3),
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        let sel = app.projects.current_mut().selection.take().unwrap();
        assert!(
            sel.contains(6, 0),
            "the prior selection survives the Alt add"
        );
        assert!(
            sel.contains(10, 10),
            "the swept grid cell joins the selection"
        );
        app.projects.current_mut().selection = Some(sel);

        // Right-button drag subtracts (starting outside the selection).
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((0, 0)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_point: Some((1, 1)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });
        let sel = app.projects.current_mut().selection.take().unwrap();
        assert!(!sel.contains(0, 0), "right-button drag must subtract");
        assert!(sel.contains(3, 3));
    }

    #[test]
    fn right_button_still_subtracts() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (3, 3));

        // Given: a committed selection. When: the right button drags across it.
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((0, 0)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_point: Some((1, 1)),
            ..Default::default()
        });
        // The canvas reports the secondary RELEASE frame with no point (see
        // `secondary_release_reports_eyedropper_ended`), so the real shape has
        // `eyedropper_point == None` here.
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });

        // Then: the dragged cells are removed from the selection.
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(!sel.contains(0, 0), "the right button must subtract");
        assert!(sel.contains(3, 3), "the rest of the selection survives");
    }

    #[test]
    fn add_without_a_base_is_a_noop() {
        let mut app = App::default();
        let new = Selection::capture(
            &app.projects.current().layers.active_layer().buffer,
            Rect2i::new(2, 2, 2, 2),
        )
        .unwrap();

        app.combine_selection(Some(new), SelectMode::Add, None);

        assert!(
            app.projects.current().selection.is_none(),
            "an Add with nothing to add to must not create a selection (D78)"
        );
    }

    #[test]
    fn subtract_without_a_base_is_a_noop() {
        let mut app = App::default();
        let new = Selection::capture(
            &app.projects.current().layers.active_layer().buffer,
            Rect2i::new(2, 2, 2, 2),
        )
        .unwrap();

        app.combine_selection(Some(new), SelectMode::Subtract, None);

        assert!(
            app.projects.current().selection.is_none(),
            "a base-less Subtract must not fabricate a selection (D78)"
        );
    }

    #[test]
    fn intersect_without_a_base_is_a_noop() {
        let mut app = App::default();
        let new = Selection::capture(
            &app.projects.current().layers.active_layer().buffer,
            Rect2i::new(2, 2, 2, 2),
        )
        .unwrap();

        app.combine_selection(Some(new), SelectMode::Intersect, None);

        assert!(
            app.projects.current().selection.is_none(),
            "an Intersect with nothing to intersect must not create a selection (D78)"
        );
    }

    #[test]
    fn replace_without_a_base_installs_the_new_selection() {
        let mut app = App::default();
        let new = Selection::capture(
            &app.projects.current().layers.active_layer().buffer,
            Rect2i::new(2, 2, 2, 2),
        )
        .unwrap();

        app.combine_selection(Some(new), SelectMode::Replace, None);

        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a plain Replace must install the new selection");
        assert_eq!(sel.rect(), Rect2i::new(2, 2, 2, 2));
    }

    #[test]
    fn right_drag_release_with_no_eyedropper_point_still_subtracts() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (3, 3));

        // Given: a committed selection. When: a real right-button drag runs —
        // the press frame carries the point, the drag frame carries a point,
        // and the release frame is exactly what the canvas emits:
        // `{eyedropper_ended: true, eyedropper_point: None}`.
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((0, 0)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_point: Some((1, 1)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });

        // Then: the release still subtracts the swept area. The old code
        // re-derived `right_button` from the missing point, fell back to
        // `Replace`, and reinstalled the swept box instead of removing it.
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(
            !sel.contains(0, 0),
            "the swept area must be subtracted even with no release point"
        );
        assert!(sel.contains(3, 3), "the rest of the selection survives");
    }

    #[test]
    fn right_drag_mode_is_locked_at_press() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (3, 3));

        // Given: a right-button press (Subtract), then Shift held only on the
        // release frame — the mode must come from the press, not the release.
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((0, 0)),
            ..Default::default()
        });
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Then: the press-locked Subtract wins; a Shift that appears only at
        // release must not turn the gesture into an Add.
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(
            !sel.contains(0, 0),
            "the mode locked at press must still subtract"
        );
        assert!(sel.contains(3, 3), "the rest of the selection survives");
    }

    #[test]
    fn ctrl_drag_transforms_the_selected_pixels() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));

        // Given: Ctrl held. When: a press inside the selection lifts it.
        set_modifiers(
            &app,
            egui::Modifiers {
                command: true,
                ctrl: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);
        assert!(app.projects.current_mut().transform.is_some());
        assert!(app.projects.current_mut().selection.is_none());

        // Drag the floating object and release: one undo step, pixels moved.
        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        // Q1 problem 2: no dead zone — the first moved sample applies, and the
        // move stays absolute from the press, landing on Δ (+2, +1).
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((7, 6)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((7, 6)),
            ..Default::default()
        });
        // A release only ends the drag; the commit is the explicit step under
        // test (its own gesture is covered by the double-click / Esc tests).
        app.transform_session_commit();
        assert!(app.projects.current_mut().transform.is_none());
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(6, 5), Some(RED));
    }

    #[test]
    fn alt_does_not_lift_a_transform() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));

        // Given: Alt held. When: a press lands inside the selection.
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Then: no transform session is created (D76 removed the Alt copy lift).
        assert!(
            app.projects.current_mut().transform.is_none(),
            "Alt must not lift a transform"
        );
        assert!(
            app.projects.current().selection.is_some(),
            "the selection stays committed"
        );
    }

    #[test]
    fn ctrl_click_inside_selection_still_lifts() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));

        // Given: Ctrl held. When: a press lands inside the selection.
        set_modifiers(
            &app,
            egui::Modifiers {
                command: true,
                ctrl: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Then: the selection is lifted as a cut (D32 intact).
        assert!(
            app.projects.current_mut().transform.is_some(),
            "Ctrl+click inside the selection must still lift it"
        );
        assert!(app.projects.current().selection.is_none());
    }

    #[test]
    fn deselect_commits_the_floating_selection_as_one_undo_step() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        // Q1 problem 2: no dead zone — the first moved sample applies, and the
        // move stays absolute from the press, landing on Δ (+2, +1).
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((7, 6)),
            ..Default::default()
        });
        // The drag is still live (no release): the floating object is uncommitted.
        assert!(app.projects.current_mut().transform.is_some());
        assert_eq!(app.projects.current_mut().undo.undo_len(), 0);

        // When: the session is deselected, which commits it as one undo step.
        app.deselect();

        // Then: exactly one undo step, source cut, copy pasted.
        assert!(app.projects.current_mut().transform.is_none());
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(6, 5), Some(RED));
    }

    #[test]
    fn non_rectangular_selection_renders_marching_ants() {
        let mut app = App::default();
        // A diagonal mask over a 2×2 box: not a rectangle.
        let sel = Selection::capture_mask(
            &app.projects.current_mut().layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        app.projects.current_mut().selection = Some(sel);

        // Then: the overlay is the mask marching-ants variant, not a rect.
        match app.current_overlay() {
            CanvasOverlay::AntsMask(segments) => {
                assert!(!segments.is_empty(), "the mask boundary must have segments");
            }
            other => panic!("expected AntsMask, got {other:?}"),
        }
    }

    #[test]
    fn cut_copy_paste_respect_the_selection_shape() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        // A diagonal mask over the 4×4 box: only (2,2) and (5,5) are selected.
        let sel = Selection::capture_mask(
            &app.projects.current_mut().layers.active_layer().buffer,
            Rect2i::new(2, 2, 4, 4),
            {
                let mut m = vec![false; 16];
                m[0] = true;
                m[15] = true;
                m
            },
        )
        .unwrap();
        app.projects.current_mut().selection = Some(sel);

        app.copy_selection();
        let clip = app.projects.current_mut().clipboard.clone().unwrap();
        assert_eq!(clip.get_pixel(0, 0), Some(RED));
        assert_eq!(
            clip.get_pixel(1, 0),
            Some(Color::TRANSPARENT),
            "holes stay transparent"
        );
        assert_eq!(clip.get_pixel(3, 3), Some(RED));

        // Cut clears only the selected pixels.
        app.cut_selection();
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(3, 2), Some(RED), "unselected pixels stay");
        assert_eq!(buf.get_pixel(5, 5), Some(Color::TRANSPARENT));
    }

    /// Part B: a paste places a transform object CENTRED on the cursor's canvas
    /// pixel. `paste_origin_for` is the pure centring helper; the object's pos
    /// and bbox follow it, and an off-canvas cursor resolves to the nearest
    /// edge first.
    #[test]
    fn paste_places_transform_object_at_cursor() {
        // Pure helper: centred on the cursor pixel (integer division).
        assert_eq!(paste_origin_for((10, 10), (4, 4)), (8, 8));
        assert_eq!(paste_origin_for((0, 0), (3, 3)), (-1, -1));

        let mut pixels = vec![0u8; 4 * 4 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        let clip = ClipboardRegion::new(4, 4, pixels);

        // Inside the canvas: centred on the cursor pixel, as a paste session
        // (no source cut, empty snapshot).
        let mut app = App::default();
        app.start_transform_from_region(&clip, (10, 10));
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .expect("a paste session")
            .expect_selection();
        assert_eq!(t.object.pos, (8.0, 8.0));
        assert_eq!(t.object.canvas_bbox(), (8.0, 8.0, 12.0, 12.0));
        assert!(!t.cut_source, "a paste does not cut a source");
        assert!(t.source.snapshot.is_empty(), "a paste has no snapshot");
        assert!(app.projects.current().selection.is_none());

        // Outside the canvas: the cursor clamps to the nearest edge first.
        // Use the App's own context so `self.ctx.input` sees the pointer event
        // that the frame delivers.
        let mut app = App::default();
        app.panel_dock_demo = false;
        let ctx = app.ctx.clone();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [CANVAS_WIDTH, CANVAS_HEIGHT],
            &vec![0; CANVAS_WIDTH * CANVAS_HEIGHT * 4],
        );
        app.canvas_texture =
            Some(ctx.load_texture("paste-cursor-test", image, egui::TextureOptions::NEAREST));
        app.texture_dirty = false;
        let mut edge = None;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events: vec![move_to(egui::pos2(300.0, 200.0))],
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| app.ui_frame(ui));
            edge = app.paste_cursor_canvas();
        });
        output.textures_delta.clear();
        let edge = edge.expect("an off-canvas pointer maps to the nearest edge");
        assert_eq!(edge, (CANVAS_WIDTH as i32 - 1, CANVAS_HEIGHT as i32 - 1));
        app.start_transform_from_region(&clip, edge);
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        assert_eq!(
            t.object.pos,
            (
                (CANVAS_WIDTH as i32 - 1 - 2) as f32,
                (CANVAS_HEIGHT as i32 - 1 - 2) as f32
            ),
            "the paste object is centred on the clamped edge pixel"
        );
    }

    /// Part B: the Ctrl+V shortcut routes to `paste_clipboard_as_transform`:
    /// the OS read is asynchronous (G3 finding #6), so the shortcut starts a
    /// background request and a later frame completes it. With no OS image the
    /// internal clipboard fallback opens a paste session at the cursor instead
    /// of blitting directly.
    #[test]
    fn paste_shortcut_opens_a_transform_session_at_the_cursor() {
        let mut app = App::default();
        app.panel_dock_demo = false;
        let ctx = app.ctx.clone();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [CANVAS_WIDTH, CANVAS_HEIGHT],
            &vec![0; CANVAS_WIDTH * CANVAS_HEIGHT * 4],
        );
        app.canvas_texture =
            Some(ctx.load_texture("paste-shortcut-test", image, egui::TextureOptions::NEAREST));
        app.texture_dirty = false;
        let mut pixels = vec![0u8; 2 * 2 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        app.projects.current_mut().clipboard = Some(ClipboardRegion::new(2, 2, pixels));
        // First frame establishes the canvas draw rect.
        run_app_frame(&mut app, &ctx, vec![]);
        run_app_frame(
            &mut app,
            &ctx,
            vec![
                modifiers_changed(egui::Modifiers {
                    command: true,
                    ..egui::Modifiers::NONE
                }),
                move_to(egui::pos2(20.0, 20.0)),
                egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        command: true,
                        ..egui::Modifiers::NONE
                    },
                },
            ],
        );

        // Ctrl+V must not block on the OS clipboard: it starts a background
        // read (receiver stored) instead of opening a session inline.
        assert!(
            app.os_paste_rx.is_some(),
            "Ctrl+V starts an asynchronous OS clipboard read"
        );
        assert!(
            app.projects.current().transform.is_none(),
            "the paste session waits for the async read to land"
        );
        // Drive a deterministic completion that yields no OS image so the
        // internal clipboard fallback opens the session on the next frame.
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(None).expect("send deterministic paste result");
        app.os_paste_rx = Some(crate::ui::clipboard::PendingPasteRead::start(rx));
        run_app_frame(&mut app, &ctx, vec![]);

        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .expect("Ctrl+V must open a paste session")
            .expect_selection();
        assert!(!t.cut_source, "a paste session does not cut a source");
        assert_eq!(
            t.object.pos,
            (19.0, 19.0),
            "the pasted object is centred on the cursor pixel (20,20)"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "opening a paste session pushes no undo entry"
        );
    }

    #[test]
    fn os_clipboard_status_is_reported() {
        // The egui/winit stack exposes `Context::copy_image`; the App reports
        // availability per project and only mirrors when available.
        let mut app = App::default();
        assert!(
            !app.projects.current().os_clipboard_available,
            "the default project reports the OS clipboard as unavailable until probed"
        );
        app.projects.current_mut().os_clipboard_available = true;
        seed_red_rect(&mut app, Rect2i::new(2, 2, 2, 2));
        marquee(&mut app, (2, 2), (3, 3));
        // Copying with the flag set must not panic and must fill the internal
        // clipboard (the OS mirror is a best-effort side effect).
        app.copy_selection();
        assert!(app.projects.current_mut().clipboard.is_some());
    }

    #[test]
    fn area_move_pushes_no_undo_step() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        marquee(&mut app, (2, 2), (5, 5));
        let undo_before = app.projects.current().undo.undo_len();
        let redo_before = app.projects.current().undo.redo_len();

        // Press inside the selection, drag to (11,11), release.
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((3, 3)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((11, 11)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((11, 11)),
            ..Default::default()
        });

        // D77: an area move leaves the undo stack untouched.
        assert_eq!(
            app.projects.current_mut().undo.undo_len(),
            undo_before,
            "an area move must not push an undo step"
        );
        assert_eq!(app.projects.current_mut().undo.redo_len(), redo_before);
        assert_eq!(
            app.projects
                .current_mut()
                .selection
                .as_ref()
                .map(|s| s.rect()),
            Some(Rect2i::new(10, 10, 4, 4)),
            "the selection still translates"
        );
    }

    #[test]
    fn copy_paste_duplicates() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        marquee(&mut app, (2, 2), (5, 5));
        app.copy_selection();
        let clip = app
            .projects
            .current_mut()
            .clipboard
            .clone()
            .expect("copy fills the internal clipboard");
        // Dismiss the selection; paste creates a floating transform object (it
        // is NOT committed until the session ends).
        app.projects.current_mut().selection = None;
        app.start_transform_from_region(&clip, (10, 10)); // pos (8,8)
        assert!(app.projects.current().transform.is_some());
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "the paste session is not committed yet"
        );
        app.transform_session_commit();
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(RED),
            "copy must not cut the source"
        );
        assert_eq!(
            buf.get_pixel(8, 8),
            Some(RED),
            "the copy lands at the cursor"
        );
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        app.undo_document();
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(RED));
        assert_eq!(
            buf.get_pixel(8, 8),
            Some(Color::TRANSPARENT),
            "undo removes the pasted copy in one step"
        );
    }

    /// Part B: a paste-as-transform session has an EMPTY source. Esc must drop
    /// it without panicking (the empty-rect recapture is a no-op) and a commit
    /// must paste it as one undo step (the identity guard is skipped for a
    /// non-cutting source).
    #[test]
    fn paste_session_escape_and_commit_handle_the_empty_source() {
        let mut app = App::default();
        let mut pixels = vec![0u8; 2 * 2 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        let clip = ClipboardRegion::new(2, 2, pixels);

        // Esc drops the paste session and leaves the document untouched.
        app.start_transform_from_region(&clip, (10, 10));
        assert!(app.projects.current().transform.is_some());
        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(app.projects.current().transform.is_none());
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        assert!(app.projects.current().selection.is_none());
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(10, 10),
            Some(Color::TRANSPARENT),
            "Esc must not paste anything"
        );

        // Commit pastes the object at the cursor as one undo step.
        app.start_transform_from_region(&clip, (10, 10)); // pos (9,9)
        app.transform_session_commit();
        assert_eq!(app.projects.current().undo.undo_len(), 1);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(9, 9),
            Some(RED)
        );
    }

    #[test]
    fn cut_selection_clears_and_undo() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        marquee(&mut app, (2, 2), (5, 5));
        app.cut_selection();
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(Color::TRANSPARENT));
        assert!(app.projects.current_mut().selection.is_none());
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        app.undo_document();
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(RED));
    }

    #[test]
    fn delete_selection_clears_pixels_and_keeps_the_selection() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        marquee(&mut app, (2, 2), (4, 4));
        app.delete_selection();

        for y in 2..5 {
            for x in 2..5 {
                assert_eq!(
                    app.projects
                        .current()
                        .layers
                        .active_layer()
                        .buffer
                        .get_pixel(x as usize, y as usize),
                    Some(Color::TRANSPARENT),
                    "({x},{y}) is inside the selection and must be cleared"
                );
            }
        }
        assert!(
            app.projects.current().selection.is_some(),
            "delete must leave the selection in place so it can be reused"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            1,
            "one gesture must land as exactly one undo step"
        );
    }

    #[test]
    fn delete_selection_only_clears_the_masked_pixels() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        let mut mask = vec![true, false, false, true];
        mask[1] = true;
        install_masked_selection(&mut app, Rect2i::new(4, 4, 2, 2), mask);
        app.delete_selection();

        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(5, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(4, 5), Some(RED), "unselected cell survives");
        assert_eq!(buf.get_pixel(5, 5), Some(Color::TRANSPARENT));
    }

    #[test]
    fn delete_selection_undo_restores_every_pixel() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        marquee(&mut app, (2, 2), (4, 4));
        app.delete_selection();
        app.undo_document();

        let buf = &app.projects.current().layers.active_layer().buffer;
        for y in 2..5 {
            for x in 2..5 {
                assert_eq!(
                    buf.get_pixel(x as usize, y as usize),
                    Some(RED),
                    "one undo must bring back every deleted pixel at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn delete_selection_redo_clears_them_again() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        marquee(&mut app, (2, 2), (4, 4));
        app.delete_selection();
        app.undo_document();
        app.redo_document();

        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
    }

    #[test]
    fn delete_selection_without_a_selection_is_a_noop() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 2, 2));
        app.delete_selection();

        assert_eq!(app.projects.current().undo.undo_len(), 0);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(RED));
    }

    #[test]
    fn deleting_an_already_empty_selection_pushes_no_undo_step() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (4, 4));
        app.delete_selection();
        let after_first = app.projects.current().undo.undo_len();

        app.delete_selection();
        assert_eq!(
            app.projects.current().undo.undo_len(),
            after_first,
            "clearing transparent pixels again must not grow the undo stack"
        );
    }

    #[test]
    fn invert_selection_selects_the_canvas_complement() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (4, 4));
        let (w, h, before) = {
            let buf = &app.projects.current().layers.active_layer().buffer;
            (buf.width(), buf.height(), 9)
        };
        app.invert_selection();

        let inverted = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("invert must keep a selection");
        assert_eq!(inverted.rect(), Rect2i::new(0, 0, w as i32, h as i32));
        assert!(!inverted.is_rectangular(), "the source stays excluded");
        assert!(!inverted.contains(2, 2), "a selected cell is now rejected");
        assert!(!inverted.contains(4, 4));
        assert!(inverted.contains(0, 0), "an unselected cell is now taken");
        assert_eq!(inverted.pixel_count(), w * h - before);
    }

    #[test]
    fn invert_selection_without_a_selection_is_a_noop() {
        let mut app = App::default();
        app.invert_selection();
        assert!(app.projects.current().selection.is_none());
    }

    #[test]
    fn inverted_selection_drives_the_write_clip() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 2, 2));
        marquee(&mut app, (2, 2), (3, 3));
        app.invert_selection();

        let clip = App::selection_pixel_clip(app.projects.current().selection.as_ref())
            .expect("an inverted selection still clips writes");
        assert!(
            !clip.contains(2, 2),
            "the original selection no longer clips"
        );
        assert!(clip.contains(20, 20), "everything else does");
    }

    #[test]
    fn delete_key_clears_the_selected_pixels_end_to_end() {
        let mut app = app_with_own_canvas();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        marquee(&mut app, (2, 2), (4, 4));
        plain_key_frame(&mut app, egui::Key::Delete);

        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::TRANSPARENT),
            "the Delete key must reach delete_selection through the keymap"
        );
        assert!(app.projects.current().selection.is_some());
    }

    #[test]
    fn backspace_also_clears_the_selected_pixels() {
        let mut app = app_with_own_canvas();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        marquee(&mut app, (2, 2), (4, 4));
        plain_key_frame(&mut app, egui::Key::Backspace);

        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(Color::TRANSPARENT));
    }

    #[test]
    fn delete_key_without_a_selection_pushes_no_undo_step() {
        let mut app = app_with_own_canvas();
        plain_key_frame(&mut app, egui::Key::Delete);
        assert_eq!(app.projects.current().undo.undo_len(), 0);
    }

    // -----------------------------------------------------------------------
    // Fieldier backend: S / W / L pick the child
    // -----------------------------------------------------------------------

    #[test]
    fn s_key_selects_rectangle_child() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        send_key(&mut app, egui::Key::S, egui::Modifiers::NONE);
        assert_eq!(
            app.projects.current().tool_state.child(),
            FieldierChild::Rectangle
        );
        assert_eq!(app.projects.current().tool_state.tool(), Tool::Fieldier);
    }

    #[test]
    fn w_key_selects_wand_child() {
        let mut app = App::default();
        send_key(&mut app, egui::Key::W, egui::Modifiers::NONE);
        assert_eq!(
            app.projects.current().tool_state.child(),
            FieldierChild::Wand
        );
        assert_eq!(app.projects.current().tool_state.tool(), Tool::Fieldier);
    }

    #[test]
    fn l_key_selects_lasso_child() {
        let mut app = App::default();
        send_key(&mut app, egui::Key::L, egui::Modifiers::NONE);
        assert_eq!(
            app.projects.current().tool_state.child(),
            FieldierChild::Lasso
        );
        assert_eq!(app.projects.current().tool_state.tool(), Tool::Fieldier);
    }

    // -----------------------------------------------------------------------
    // Alt+drag takes grid-cell borders instead of a marquee
    // -----------------------------------------------------------------------

    #[test]
    fn a_plain_drag_still_marquees() {
        let mut app = App::default();
        set_modifiers(&app, egui::Modifiers::NONE);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((1, 1)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((3, 2)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((3, 2)),
            ..Default::default()
        });
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert_eq!(sel.rect(), Rect2i::new(1, 1, 3, 2));
        assert!(sel.is_rectangular());
    }

    #[test]
    fn alt_drag_starts_the_region_selection() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((25, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((25, 5)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("an Alt+drag must select whole grid cells");
        assert_eq!(
            sel.rect(),
            Rect2i::new(0, 0, 32, 16),
            "the drag crosses the two 16px cells in the first row"
        );
        assert_eq!(sel.pixel_count(), 32 * 16);
    }

    #[test]
    fn ctrl_drag_no_longer_starts_a_region() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        set_modifiers(
            &app,
            egui::Modifiers {
                command: true,
                ctrl: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((25, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((25, 5)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a Ctrl+drag still marquees");
        assert_eq!(
            sel.rect(),
            Rect2i::new(5, 5, 21, 1),
            "Ctrl+drag must marquee, not take whole grid cells"
        );
    }

    // -----------------------------------------------------------------------
    // D77: combining keeps the base visible; right button never deselects;
    // Alt region sweep accumulates cells with a button-chosen add/subtract
    // -----------------------------------------------------------------------

    #[test]
    fn base_selection_stays_visible_during_an_add_gesture() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (3, 3));

        // Given: a committed base selection. When: a Shift marquee drag begins.
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((8, 8)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((11, 11)),
            ..Default::default()
        });

        // Then: the live overlay paints the swept box AND the base's ants, and
        // the committed base selection is still installed (D77 item 2).
        match app.current_overlay() {
            CanvasOverlay::RegionCells { cells, base } => {
                assert_eq!(cells, vec![Rect2i::new(8, 8, 4, 4)]);
                assert!(!base.is_empty(), "the base selection's ants must survive");
            }
            other => panic!("expected RegionCells with the base ants, got {other:?}"),
        }
        assert!(
            app.projects.current().selection.is_some(),
            "the base selection stays installed during the gesture"
        );

        // Release keeps the union of both boxes.
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((11, 11)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(sel.contains(0, 0) && sel.contains(11, 11));
    }

    #[test]
    fn base_selection_stays_visible_during_a_subtract_gesture() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (7, 7));

        // Given: a committed base selection. When: a right-button drag begins.
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((4, 4)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_point: Some((11, 11)),
            ..Default::default()
        });

        // Then: the swept box and the base's ants are both live, and the base
        // is still installed (D77 item 2).
        match app.current_overlay() {
            CanvasOverlay::RegionCells { cells, base } => {
                assert_eq!(cells, vec![Rect2i::new(4, 4, 8, 8)]);
                assert!(!base.is_empty(), "the base selection's ants must survive");
            }
            other => panic!("expected RegionCells with the base ants, got {other:?}"),
        }
        assert!(app.projects.current().selection.is_some());

        // Release subtracts the swept box, leaving the rest of the base.
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(!sel.contains(4, 4), "the swept box is removed");
        assert!(
            sel.contains(0, 0) && sel.contains(0, 7),
            "the remainder stays"
        );
    }

    #[test]
    fn right_drag_without_a_selection_is_a_noop() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        seed_red_rect(&mut app, Rect2i::new(2, 2, 2, 2));
        let undo_before = app.projects.current().undo.undo_len();

        // Given: no selection. When: the right button drags.
        right_drag(&mut app, (2, 2), (5, 5));

        // Then: nothing is created, cleared, or moved (D77 item 3).
        assert!(
            app.projects.current().selection.is_none(),
            "a right drag with no selection must not create one"
        );
        assert!(
            app.projects.current().gesture.is_idle(),
            "no gesture must start"
        );
        assert_eq!(app.projects.current().undo.undo_len(), undo_before);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(RED), "no pixels are touched");
    }

    #[test]
    fn right_drag_subtracts_from_the_selection() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (3, 3));

        // Given: a committed selection. When: the right button drags across it.
        right_drag(&mut app, (0, 0), (1, 1));

        // Then: the swept area is removed and the remainder kept (D77 item 3).
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(!sel.contains(0, 0), "the swept area is subtracted");
        assert!(sel.contains(3, 3), "the rest of the selection survives");
    }

    #[test]
    fn right_press_inside_subtracts_not_moves() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        marquee(&mut app, (0, 0), (3, 3));
        let undo_before = app.projects.current().undo.undo_len();

        // Given: a selection. When: the right button presses and releases inside.
        set_modifiers(&app, egui::Modifiers::NONE);
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((3, 3)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });

        // Then: the press subtracts its swept pixel — it does not translate the
        // selection into an area move (D77 item 3).
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(!sel.contains(3, 3), "the pressed pixel is subtracted");
        assert!(sel.contains(0, 0), "the rest of the selection stays put");
        assert_eq!(sel.rect(), Rect2i::new(0, 0, 4, 4), "it did not move");
        assert_eq!(app.projects.current().undo.undo_len(), undo_before);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(
            buf.get_pixel(3, 3),
            Some(RED),
            "no pixels are cut or pasted"
        );
    }

    #[test]
    fn alt_left_sweeps_cells_to_add() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (15, 15));

        // Given: a one-cell base selection. When: Alt+left sweeps two more
        // cells in the same row.
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((20, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((40, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((40, 5)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Then: the swept cells are UNIONED with the base (D77 item 4).
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert_eq!(sel.rect(), Rect2i::new(0, 0, 48, 16));
        assert!(sel.contains(5, 5), "the base cell stays");
        assert!(
            sel.contains(20, 5) && sel.contains(40, 5),
            "the sweep is added"
        );
    }

    #[test]
    fn alt_right_sweeps_cells_to_remove() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        marquee(&mut app, (0, 0), (31, 31));

        // Given: a two-by-two-cell selection. When: Alt+right sweeps the first
        // grid row.
        right_drag_with(
            &mut app,
            (5, 5),
            (25, 5),
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );

        // Then: the swept cells are SUBTRACTED, leaving the second row
        // (D77 item 4).
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(!sel.contains(5, 5), "the swept row is removed");
        assert!(!sel.contains(25, 5), "the swept row is removed");
        assert!(sel.contains(5, 20), "the untouched row survives");
    }

    #[test]
    fn alt_sweep_collects_every_cell_the_pointer_passes_over() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);

        // Given: no selection. When: Alt+left traces an L — down first, then
        // across — so the path visits cells a straight press→release line would
        // skip.
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((5, 40)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((40, 40)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((40, 40)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Then: every crossed cell is selected — the vertical column AND the
        // horizontal run — but the diagonal cell a straight line would have
        // taken is not (D77 item 4).
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(sel.contains(5, 20), "the vertical sweep is collected");
        assert!(sel.contains(20, 40), "the horizontal sweep is collected");
        assert!(
            !sel.contains(20, 20),
            "a cell the pointer never crossed stays unselected"
        );
    }

    // -----------------------------------------------------------------------
    // Lasso child
    // -----------------------------------------------------------------------

    #[test]
    fn lasso_drag_selects_polygon_interior() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Lasso);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((1, 1)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((8, 1)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((1, 8)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((1, 8)),
            ..Default::default()
        });
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a lasso must select its polygon interior");
        assert!(sel.contains(2, 2), "inside the triangle");
        assert!(!sel.contains(7, 7), "outside the triangle's hypotenuse");
    }

    #[test]
    fn lasso_self_overlap_does_not_deselect() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Lasso);
        let points = [(1, 1), (8, 1), (1, 8), (1, 1), (8, 1), (1, 8)];
        for (index, pt) in points.into_iter().enumerate() {
            let first = index == 0;
            let last = index + 1 == points.len();
            app.handle_interactions(CanvasInteractions {
                stroke_started: first,
                stroke_ended: last,
                stroke_point: Some(pt),
                ..Default::default()
            });
        }
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a self-overlapping lasso must not cancel itself via even-odd fill");
        assert!(sel.contains(2, 2));
        assert!(!sel.contains(7, 7));
    }

    // -----------------------------------------------------------------------
    // Wand child: restrict to region, Alt override, double-click coverage
    // -----------------------------------------------------------------------

    #[test]
    fn wand_click_selects_contiguous_region() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 2, 2));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(10, 10, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a wand click must select the seed's matching region");
        assert_eq!(sel.rect(), Rect2i::new(2, 2, 2, 2));
        assert_eq!(sel.pixel_count(), 4);
        assert!(!sel.contains(10, 10), "the disconnected pixel stays out");
    }

    #[test]
    fn wand_alt_temporarily_drops_contiguity() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 2, 2));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(10, 10, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("an Alt+wand click must still select");
        assert_eq!(
            sel.pixel_count(),
            5,
            "Alt drops contiguity to match globally"
        );
        assert!(sel.contains(10, 10));
    }

    #[test]
    fn wand_double_click_selects_alpha_component() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 1, 2));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(3, 2, RED);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(10, 10, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((2, 2)),
            ..Default::default()
        });
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a double-click must select the opaque alpha component");
        assert_eq!(sel.rect(), Rect2i::new(2, 2, 2, 2));
        assert_eq!(sel.pixel_count(), 3);
        assert!(!sel.contains(10, 10), "the disconnected pixel stays out");
    }

    #[test]
    fn wand_restrict_to_region_clamps() {
        let mut app = App::default();
        for x in 2..26 {
            app.projects
                .current_mut()
                .layers
                .active_layer_mut()
                .buffer
                .set_pixel(x, 2, RED);
        }
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.projects
            .current_mut()
            .tool_state
            .wand_mut()
            .restrict_to_region = true;
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a restricted wand click must select the clipped run");
        assert_eq!(sel.rect(), Rect2i::new(2, 2, 14, 1));
        assert!(!sel.contains(20, 2), "the next tile cell is excluded");
    }

    #[test]
    fn wand_ctrl_scroll_adjusts_tolerance() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.projects.current_mut().tool_state.wand_mut().tolerance = 10;
        app.handle_interactions(CanvasInteractions {
            shape_cycle: 5,
            ..Default::default()
        });
        assert_eq!(app.projects.current().tool_state.wand().tolerance, 15);
        app.handle_interactions(CanvasInteractions {
            shape_cycle: -20,
            ..Default::default()
        });
        assert_eq!(app.projects.current().tool_state.wand().tolerance, 0);
        app.handle_interactions(CanvasInteractions {
            shape_cycle: 300,
            ..Default::default()
        });
        assert_eq!(app.projects.current().tool_state.wand().tolerance, 255);
    }

    #[test]
    fn wand_ctrl_scroll_does_not_change_brush_shape() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .shape = BrushShape::Round;
        app.handle_interactions(CanvasInteractions {
            shape_cycle: 1,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Round,
            "Ctrl+scroll on the wand must not touch the pen's brush shape"
        );
        assert_eq!(app.projects.current().tool_state.wand().tolerance, 1);
    }

    #[test]
    fn wand_shift_adds_and_right_subtracts() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, RED);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(8, 8, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            clicked: Some((8, 8)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);
        let added = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("Shift+wand must add");
        assert!(added.contains(2, 2) && added.contains(8, 8));
        assert_eq!(added.pixel_count(), 2);

        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((2, 2)),
            ..Default::default()
        });
        let subtracted = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("right+wand must subtract, not dismiss");
        assert!(!subtracted.contains(2, 2));
        assert!(subtracted.contains(8, 8));
        assert_eq!(subtracted.pixel_count(), 1);
    }

    #[test]
    fn wand_right_click_subtracts_once_not_per_frame() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, RED);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(8, 8, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        // Build a two-component selection: click (2,2), Shift+click (8,8).
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            clicked: Some((8, 8)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Given: the two-component selection. When: a right press (Subtract)
        // is followed by a drag frame still holding the button.
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((2, 2)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_point: Some((8, 8)),
            ..Default::default()
        });

        // Then: the press subtracted (2,2) exactly once, and the drag frame did
        // not re-fire on (8,8) — the first component is gone, the second stays.
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(
            !sel.contains(2, 2),
            "the right press must subtract its seed"
        );
        assert!(
            sel.contains(8, 8),
            "a drag frame must not apply the wand again"
        );
    }

    #[test]
    fn wand_right_drag_does_not_re_add() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, RED);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(8, 8, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            clicked: Some((8, 8)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Given: the two-component selection. When: a full right drag runs from
        // (2,2) across (8,8) to release.
        app.handle_interactions(CanvasInteractions {
            eyedropper_started: true,
            eyedropper_point: Some((2, 2)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_point: Some((8, 8)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            eyedropper_ended: true,
            eyedropper_point: None,
            ..Default::default()
        });

        // Then: only the seeded component was removed; the drag never re-ran
        // the wand over (8,8) and the release never re-derived a Replace/Add.
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(!sel.contains(2, 2), "the seeded component is subtracted");
        assert!(
            sel.contains(8, 8),
            "the drag must not re-apply the wand to a later component"
        );
    }

    #[test]
    fn wand_shift_double_click_adds_to_the_selection() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, RED);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(8, 8, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        // Given: (2,2) is already selected.
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });

        // When: Shift is held and (8,8) is double-clicked.
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((8, 8)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        // Then: the alpha component of (8,8) is UNIONED with the existing
        // selection rather than replacing it.
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(
            sel.contains(2, 2),
            "the Shift+double-click must not wipe the existing selection"
        );
        assert!(sel.contains(8, 8), "the new component is added");
    }

    #[test]
    fn wand_double_click_without_shift_replaces() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, RED);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(8, 8, RED);
        app.projects
            .current_mut()
            .tool_state
            .select_child(FieldierChild::Wand);
        app.handle_interactions(CanvasInteractions {
            clicked: Some((2, 2)),
            ..Default::default()
        });

        // Given: (2,2) selected. When: (8,8) is double-clicked with no modifier.
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((8, 8)),
            ..Default::default()
        });

        // Then: the component replaces the selection, as a plain left
        // double-click did before D78.
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert!(
            sel.contains(8, 8),
            "the double-clicked component is selected"
        );
        assert!(
            !sel.contains(2, 2),
            "a plain double-click replaces the previous selection"
        );
    }

    #[test]
    fn esc_clears_selection_unchanged() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (5, 5));
        assert!(app.projects.current().selection.is_some());
        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(
            app.projects.current().selection.is_none(),
            "Esc must still cancel the selection"
        );
    }

    // -----------------------------------------------------------------------
    // Flip the selection
    // -----------------------------------------------------------------------

    #[test]
    fn flip_key_mirrors_the_selected_pixels_and_keeps_them_selected() {
        let mut app = app_with_own_canvas();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 2, 1));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, Color::rgb(1, 0, 0));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(3, 2, Color::rgb(2, 0, 0));
        set_modifiers(&app, egui::Modifiers::NONE);
        marquee(&mut app, (2, 2), (3, 2));
        app.flip_selection(true);

        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(
            buf.get_pixel(3, 2),
            Some(Color::rgb(1, 0, 0)),
            "left moved right"
        );
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::rgb(2, 0, 0)),
            "right moved left"
        );
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert_eq!(sel.pixel_count(), 2, "the same two pixels stay selected");
        assert_eq!(
            app.projects.current().undo.undo_len(),
            1,
            "one gesture, one undo step"
        );
    }

    #[test]
    fn flip_selection_undo_restores_the_pixels() {
        let mut app = app_with_own_canvas();
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, Color::rgb(1, 0, 0));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(3, 2, Color::rgb(2, 0, 0));
        set_modifiers(&app, egui::Modifiers::NONE);
        marquee(&mut app, (2, 2), (3, 2));
        app.flip_selection(false);
        app.undo_document();
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(2, 2), Some(Color::rgb(1, 0, 0)));
        assert_eq!(buf.get_pixel(3, 2), Some(Color::rgb(2, 0, 0)));
    }

    #[test]
    fn flip_selection_without_a_selection_is_a_noop() {
        let mut app = app_with_own_canvas();
        app.flip_selection(true);
        assert_eq!(app.projects.current().undo.undo_len(), 0);
    }

    // -----------------------------------------------------------------------
    // Rotate / flip the selection via their key bindings (D79)
    // -----------------------------------------------------------------------

    /// Send a modified key as winit reports it: `ModifiersChanged` sets the
    /// held modifiers, then the key press carries them. `Event::Key` alone
    /// does not update `input.modifiers`, so both events are required.
    fn modified_key_frame(app: &mut App, key: egui::Key, modifiers: egui::Modifiers) {
        run_own_ctx_frame(
            app,
            vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
        );
    }

    #[test]
    fn rotate_key_rotates_selection_90_cw() {
        let mut app = app_with_own_canvas();
        // A 4×2 box of distinct pixels: top row 1-4, bottom row 5-8.
        let cells = [
            (2, 2, 1),
            (3, 2, 2),
            (4, 2, 3),
            (5, 2, 4),
            (2, 3, 5),
            (3, 3, 6),
            (4, 3, 7),
            (5, 3, 8),
        ];
        for (x, y, v) in cells {
            app.projects
                .current_mut()
                .layers
                .active_layer_mut()
                .buffer
                .set_pixel(x, y, Color::rgb(v, 0, 0));
        }
        marquee(&mut app, (2, 2), (5, 3));
        modified_key_frame(
            &mut app,
            egui::Key::F,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );

        let buf = &app.projects.current().layers.active_layer().buffer;
        // 90° CW around the box center: the top row becomes the right column,
        // the bottom row becomes the left column.
        assert_eq!(buf.get_pixel(4, 1), Some(Color::rgb(1, 0, 0)));
        assert_eq!(buf.get_pixel(4, 2), Some(Color::rgb(2, 0, 0)));
        assert_eq!(buf.get_pixel(4, 3), Some(Color::rgb(3, 0, 0)));
        assert_eq!(buf.get_pixel(4, 4), Some(Color::rgb(4, 0, 0)));
        assert_eq!(buf.get_pixel(3, 1), Some(Color::rgb(5, 0, 0)));
        assert_eq!(buf.get_pixel(3, 2), Some(Color::rgb(6, 0, 0)));
        assert_eq!(buf.get_pixel(3, 3), Some(Color::rgb(7, 0, 0)));
        assert_eq!(buf.get_pixel(3, 4), Some(Color::rgb(8, 0, 0)));
        // Source cells the rotated box no longer covers are cleared.
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::TRANSPARENT),
            "a source cell outside the dest box is cleared"
        );
        assert_eq!(
            buf.get_pixel(5, 3),
            Some(Color::TRANSPARENT),
            "a source cell outside the dest box is cleared"
        );
        let sel = app.projects.current().selection.as_ref().unwrap();
        assert_eq!(sel.pixel_count(), 8, "the same eight pixels stay selected");
        assert_eq!(
            app.projects.current().undo.undo_len(),
            1,
            "one gesture, one undo step"
        );
        assert_eq!(
            app.projects.current().undo.top_undo_name(),
            Some("Rotate"),
            "the undo step is named Rotate"
        );
    }

    #[test]
    fn rotate_selection_undo_restores() {
        let mut app = app_with_own_canvas();
        let cells = [
            (2, 2, 1),
            (3, 2, 2),
            (4, 2, 3),
            (5, 2, 4),
            (2, 3, 5),
            (3, 3, 6),
            (4, 3, 7),
            (5, 3, 8),
        ];
        for (x, y, v) in cells {
            app.projects
                .current_mut()
                .layers
                .active_layer_mut()
                .buffer
                .set_pixel(x, y, Color::rgb(v, 0, 0));
        }
        marquee(&mut app, (2, 2), (5, 3));
        // The union of the source box and the rotated destination box.
        let union = Rect2i::new(2, 1, 4, 4);
        let before = app
            .projects
            .current()
            .layers
            .active_layer()
            .buffer
            .export_region(union, None)
            .expect("the union region is inside the canvas");
        modified_key_frame(
            &mut app,
            egui::Key::F,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        app.undo_document();
        let after = app
            .projects
            .current()
            .layers
            .active_layer()
            .buffer
            .export_region(union, None)
            .expect("the union region is inside the canvas");
        assert_eq!(
            before, after,
            "undo restores the union region byte-for-byte"
        );
    }

    #[test]
    fn rotate_selection_without_selection_is_a_noop() {
        let mut app = app_with_own_canvas();
        modified_key_frame(
            &mut app,
            egui::Key::F,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        assert_eq!(app.projects.current().undo.undo_len(), 0);
    }

    #[test]
    fn shift_f_binds_vertical_flip() {
        let mut app = app_with_own_canvas();
        // A 2×2 box: top row 1-2, bottom row 3-4.
        let cells = [(2, 2, 1), (3, 2, 2), (2, 3, 3), (3, 3, 4)];
        for (x, y, v) in cells {
            app.projects
                .current_mut()
                .layers
                .active_layer_mut()
                .buffer
                .set_pixel(x, y, Color::rgb(v, 0, 0));
        }
        marquee(&mut app, (2, 2), (3, 3));
        modified_key_frame(
            &mut app,
            egui::Key::F,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );

        let buf = &app.projects.current().layers.active_layer().buffer;
        // Vertical flip mirrors top↔bottom.
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::rgb(3, 0, 0)),
            "bottom-left moved up"
        );
        assert_eq!(
            buf.get_pixel(3, 2),
            Some(Color::rgb(4, 0, 0)),
            "bottom-right moved up"
        );
        assert_eq!(
            buf.get_pixel(2, 3),
            Some(Color::rgb(1, 0, 0)),
            "top-left moved down"
        );
        assert_eq!(
            buf.get_pixel(3, 3),
            Some(Color::rgb(2, 0, 0)),
            "top-right moved down"
        );
        assert_eq!(
            app.projects.current().undo.top_undo_name(),
            Some("Flip"),
            "the undo step is named Flip"
        );
    }

    #[test]
    fn ctrl_f_binds_horizontal_flip() {
        let mut app = app_with_own_canvas();
        // A 2×2 box: top row 1-2, bottom row 3-4.
        let cells = [(2, 2, 1), (3, 2, 2), (2, 3, 3), (3, 3, 4)];
        for (x, y, v) in cells {
            app.projects
                .current_mut()
                .layers
                .active_layer_mut()
                .buffer
                .set_pixel(x, y, Color::rgb(v, 0, 0));
        }
        marquee(&mut app, (2, 2), (3, 3));
        modified_key_frame(
            &mut app,
            egui::Key::F,
            egui::Modifiers {
                command: true,
                ..egui::Modifiers::NONE
            },
        );

        let buf = &app.projects.current().layers.active_layer().buffer;
        // Horizontal flip mirrors left↔right.
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::rgb(2, 0, 0)),
            "top-right moved left"
        );
        assert_eq!(
            buf.get_pixel(3, 2),
            Some(Color::rgb(1, 0, 0)),
            "top-left moved right"
        );
        assert_eq!(
            buf.get_pixel(2, 3),
            Some(Color::rgb(4, 0, 0)),
            "bottom-right moved left"
        );
        assert_eq!(
            buf.get_pixel(3, 3),
            Some(Color::rgb(3, 0, 0)),
            "bottom-left moved right"
        );
        assert_eq!(
            app.projects.current().undo.top_undo_name(),
            Some("Flip"),
            "the undo step is named Flip"
        );
    }

    #[test]
    /// Given the committed selection, when a live drag starts, then the overlay is the plain marquee outline rather than the tinted ants border.
    fn a_live_selection_drag_uses_the_plain_marquee_overlay() {
        let mut app = App::default();
        set_modifiers(&app, egui::Modifiers::NONE);
        marquee(&mut app, (2, 2), (6, 6));
        assert!(matches!(app.current_overlay(), CanvasOverlay::AntsMask(_)));

        // Press inside the selection starts an area move: the preview must be
        // the plain outline, not the tinted ants border.
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((3, 3)),
            ..Default::default()
        });
        assert!(
            matches!(app.current_overlay(), CanvasOverlay::Marquee(_)),
            "an in-flight selection drag must not paint the tinted ants overlay"
        );
    }

    #[test]
    fn mask_selection_move_previews_its_boundary() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        // A 2×2 diagonal mask at (2,2): only (2,2) and (3,3) are selected.
        install_masked_selection(
            &mut app,
            Rect2i::new(2, 2, 2, 2),
            vec![true, false, false, true],
        );

        // Press a selected pixel: the live preview must be the translated mask
        // boundary, not a filled destination box (D76).
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((2, 2)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        match app.current_overlay() {
            CanvasOverlay::AntsMask(segments) => {
                assert!(!segments.is_empty(), "the mask boundary must have segments");
                assert!(
                    segments.contains(&((4, 4), (5, 4))),
                    "the preview must trace the translated mask, not a filled box"
                );
            }
            other => panic!("expected an AntsMask preview, got {other:?}"),
        }
    }

    #[test]
    fn marquee_probe_is_sampled_under_the_pointer() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(10, 10, Color::rgba(10, 20, 30, 255));

        // On-canvas: the probe is the pixel under the pointer.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(10.0, 10.0))]);
        assert_eq!(
            app.marquee_probe_at(
                &ctx,
                app.projects.current().camera,
                (CANVAS_WIDTH as u32, CANVAS_HEIGHT as u32)
            ),
            Some(Color::rgba(10, 20, 30, 255)),
            "an on-canvas pointer must sample the pixel under it"
        );

        // Off-canvas: the probe falls back to None (the theme stroke).
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(300.0, 200.0))]);
        assert_eq!(
            app.marquee_probe_at(
                &ctx,
                app.projects.current().camera,
                (CANVAS_WIDTH as u32, CANVAS_HEIGHT as u32)
            ),
            None,
            "an off-canvas pointer must fall back to None"
        );
    }

    // -----------------------------------------------------------------------
    // Double-click outside dismisses
    // -----------------------------------------------------------------------

    #[test]
    /// Given a press that starts outside the canvas, when the drag continues inside, then the selection anchors at the clamped edge and covers only existing pixels.
    fn a_selection_drag_may_start_outside_the_canvas() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        let outside = egui::pos2(300.0, 200.0);
        let far = CANVAS_WIDTH as i32 - 1;
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![press(outside)]);

        assert_eq!(
            app.projects
                .current()
                .gesture
                .marquee()
                .map(|(_, rect)| rect),
            Some(Rect2i::new(far, far, 1, 1)),
            "an outside press must anchor the marquee at the clamped edge"
        );

        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(60.0, 60.0))]);
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("the drag must produce a selection");
        assert_eq!(sel.rect(), Rect2i::new(60, 60, far - 59, far - 59));
        assert!(
            sel.rect().right() <= CANVAS_WIDTH as i32,
            "the selection must stay inside the existing pixels"
        );
    }

    #[test]
    fn double_click_outside_the_selection_dismisses_it() {
        let mut app = App::default();
        set_modifiers(&app, egui::Modifiers::NONE);
        marquee(&mut app, (2, 2), (4, 4));
        assert!(app.projects.current().selection.is_some());
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((20, 20)),
            ..Default::default()
        });
        assert!(
            app.projects.current().selection.is_none(),
            "a double-click on empty canvas must deselect"
        );
    }

    #[test]
    fn double_click_inside_the_selection_keeps_it() {
        let mut app = App::default();
        set_modifiers(&app, egui::Modifiers::NONE);
        marquee(&mut app, (2, 2), (4, 4));
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((3, 3)),
            ..Default::default()
        });
        assert!(
            app.projects.current().selection.is_some(),
            "a double-click inside must not dismiss"
        );
    }

    /// Part B: a paste object placed past the bottom-right edge commits without
    /// panicking and is cropped to the canvas (the off-canvas part is dropped).
    #[test]
    fn paste_clipped_at_bottom_right() {
        let mut app = App::default();
        let mut pixels = vec![0u8; 4 * 4 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        let clip = ClipboardRegion::new(4, 4, pixels);
        // Cursor on the last canvas pixel: the 4×4 object spans (125,125)-(129,129)
        // and must clip to the 3×3 in-canvas corner.
        app.start_transform_from_region(&clip, (127, 127));
        assert_eq!(
            app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .object
                .canvas_bbox(),
            (125.0, 125.0, 129.0, 129.0)
        );
        app.transform_session_commit();
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(125, 125), Some(RED));
        assert_eq!(buf.get_pixel(127, 127), Some(RED));
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        app.undo_document();
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(125, 125), Some(Color::TRANSPARENT));
    }

    #[test]
    fn two_consecutive_opacity_changes_coalesce() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        app.apply_layer_events(vec![
            LayerPanelEvent::SetOpacity(lid, 0.5),
            LayerPanelEvent::SetOpacity(lid, 0.3),
        ]);
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .layer(lid)
                .unwrap()
                .opacity,
            0.3
        );
    }

    #[test]
    fn app_dock_contains_toolbox_on_the_left_and_layers_on_the_right() {
        let app = App::default();
        let dock = &app.panel_dock;
        assert_eq!(
            dock.placement(panel_dock::PanelId::new(100)),
            Some(panel_dock::PanelPlacement::DockedLeft),
            "Toolbox docks left"
        );
        assert_eq!(
            dock.placement(panel_dock::PanelId::new(101)),
            Some(panel_dock::PanelPlacement::DockedRight),
            "Layers docks right"
        );
        assert_eq!(
            dock.dock_order(panel_dock::DockSide::Left),
            &[panel_dock::PanelId::new(100), panel_dock::PanelId::new(103)]
        );
        assert!(dock
            .dock_order(panel_dock::DockSide::Right)
            .contains(&panel_dock::PanelId::new(101)));
        assert_eq!(
            dock.placement(panel_dock::PanelId::new(
                dock_color_palette_panel::PALETTE_PANEL_ID
            )),
            Some(panel_dock::PanelPlacement::DockedRight),
            "Palette docks right"
        );
        // The demo panels (A/B/C/D) are still present alongside the new ones.
        assert_eq!(dock.panel_count(), 8);
        assert_eq!(
            dock.placement(panel_dock::PanelId::new(1)),
            Some(panel_dock::PanelPlacement::DockedRight)
        );
        assert_eq!(
            dock.placement(panel_dock::PanelId::new(3)),
            Some(panel_dock::PanelPlacement::DockedBottom)
        );
    }

    #[test]
    fn tool_property_panel_is_registered_in_the_app_dock() {
        let app = App::default();
        let dock = &app.panel_dock;
        let id = panel_dock::PanelId::new(103);
        assert_eq!(
            dock.placement(id),
            Some(panel_dock::PanelPlacement::DockedLeft),
            "Tool Property docks left"
        );
        assert_eq!(dock.metadata(id).unwrap().title, "Tool Property");
        assert_eq!(
            dock.dock_order(panel_dock::DockSide::Left),
            &[panel_dock::PanelId::new(100), id],
            "Tool Property sits below the Toolbox in the left dock"
        );
    }

    #[test]
    fn app_layer_rows_are_top_first_and_skip_collapsed_groups() {
        let mut app = App::default();
        let session = app.projects.current_mut();
        let root = session.layers.active_layer_id();
        let a = session.layers.add_layer("A");
        let b = session.layers.add_layer("B");
        let c = session.layers.add_layer("C");
        let g = session.layers.create_group_around(&[a, b], "G").unwrap();
        // Model (bottom-to-top): [root, G, A, B, C].

        // Expanded: display order is top-first — C, G, B, A, root.
        app.layers_host.expanded_groups.borrow_mut().insert(g);
        app.write_dock_snapshots();
        let view = app.layers_host.view.borrow();
        let ids: Vec<LayerId> = view.rows.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec![c, g, b, a, root]);
        assert_eq!(view.rows[1].depth, 0, "the group row sits at root depth");
        assert_eq!(view.rows[2].depth, 1, "children are indented one level");
        assert!(view.rows[1].expanded);
        drop(view);

        // Collapsed: the group's descendants vanish but the group row remains.
        app.apply_layer_events(vec![LayerPanelEvent::ToggleExpanded(g)]);
        assert!(!app.layers_host.expanded_groups.borrow().contains(&g));
        app.write_dock_snapshots();
        let view = app.layers_host.view.borrow();
        let ids: Vec<LayerId> = view.rows.iter().map(|row| row.id).collect();
        assert_eq!(ids, vec![c, g, root]);
        assert!(!view.rows[1].expanded);
    }

    #[test]
    fn app_settings_toggle_opens_and_remove_clears_it() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        let settings_id = panel_dock::PanelId::new(dock_layers_panel::LAYER_SETTINGS_PANEL_ID);
        assert_eq!(*app.layers_host.open_settings.borrow(), None);
        assert_eq!(app.panel_dock.placement(settings_id), None);

        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        assert_eq!(*app.layers_host.open_settings.borrow(), Some(lid));
        assert_eq!(
            app.panel_dock.placement(settings_id),
            Some(panel_dock::PanelPlacement::Floating)
        );

        // Toggling the same id again closes the popup.
        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        assert_eq!(*app.layers_host.open_settings.borrow(), None);
        assert_eq!(app.panel_dock.placement(settings_id), None);

        // Reopen, then remove the layer: the popup must not dangle.
        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        assert_eq!(*app.layers_host.open_settings.borrow(), Some(lid));
        app.projects.current_mut().layers.add_layer("Layer 2");
        app.apply_layer_events(vec![LayerPanelEvent::Remove(lid)]);
        assert_eq!(*app.layers_host.open_settings.borrow(), None);
    }

    #[test]
    fn app_settings_toggle_switch_retargets_the_single_panel() {
        let mut app = App::default();
        let session = app.projects.current_mut();
        let a = session.layers.active_layer_id();
        let b = session.layers.add_layer("B");
        let settings_id = panel_dock::PanelId::new(dock_layers_panel::LAYER_SETTINGS_PANEL_ID);

        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(a)]);
        assert_eq!(
            app.panel_dock.placement(settings_id),
            Some(panel_dock::PanelPlacement::Floating)
        );
        assert_eq!(*app.layers_host.open_settings.borrow(), Some(a));

        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(b)]);
        assert_eq!(
            app.panel_dock.panel_count(),
            9,
            "switching must not add a second settings panel"
        );
        assert_eq!(*app.layers_host.open_settings.borrow(), Some(b));
        assert_eq!(*app.layers_host.rename_buffer.borrow(), "B");
        assert_eq!(
            app.panel_dock.metadata(settings_id).unwrap().title,
            "Layer: B"
        );
        assert!(app.panel_dock.floating_order().contains(&settings_id));
    }

    #[test]
    fn app_settings_panel_title_follows_the_layer_name() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        let settings_id = panel_dock::PanelId::new(dock_layers_panel::LAYER_SETTINGS_PANEL_ID);
        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        app.projects
            .current_mut()
            .layers
            .layer_mut(lid)
            .unwrap()
            .name = "Sketch".to_string();

        app.sync_layer_settings_panel();

        assert_eq!(
            app.panel_dock.metadata(settings_id).unwrap().title,
            "Layer: Sketch"
        );
    }

    #[test]
    fn app_settings_panel_is_removed_when_the_open_layer_is_gone() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        let settings_id = panel_dock::PanelId::new(dock_layers_panel::LAYER_SETTINGS_PANEL_ID);
        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        assert_eq!(
            app.panel_dock.placement(settings_id),
            Some(panel_dock::PanelPlacement::Floating)
        );

        // Removing the open layer clears open_settings; the per-frame sync
        // then drops the now-empty panel.
        app.projects.current_mut().layers.add_layer("Layer 2");
        app.apply_layer_events(vec![LayerPanelEvent::Remove(lid)]);
        assert_eq!(*app.layers_host.open_settings.borrow(), None);
        app.sync_layer_settings_panel();
        assert_eq!(app.panel_dock.placement(settings_id), None);
    }

    #[test]
    fn closing_a_floating_only_panel_removes_it() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        let settings_id = panel_dock::PanelId::new(dock_layers_panel::LAYER_SETTINGS_PANEL_ID);
        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        assert_eq!(
            app.panel_dock.header_action(settings_id),
            Some(panel_dock::PanelHeaderAction::Close)
        );
        let count_before = app.panel_dock.panel_count();

        app.handle_dock_actions(vec![panel_dock::DockAction::ClosePanel(settings_id)]);

        assert_eq!(app.panel_dock.panel_count(), count_before - 1);
        assert_eq!(app.panel_dock.placement(settings_id), None);
        assert_eq!(*app.layers_host.open_settings.borrow(), None);
        assert!(app.layers_host.rename_buffer.borrow().is_empty());
    }

    /// The floating Color Picker's dock id.
    fn color_picker_panel_id() -> panel_dock::PanelId {
        panel_dock::PanelId::new(dock_color_picker_panel::COLOR_PICKER_PANEL_ID)
    }

    /// Applies one color-picker color commit and refreshes the snapshot,
    /// mirroring the per-frame drain order.
    fn commit_color_picker_color(app: &mut App, color: Color) {
        app.color_picker_host
            .events
            .borrow_mut()
            .push(ToolbarEvent::ColorChanged(color));
        app.drain_dock_events();
        app.write_dock_snapshots();
    }

    /// Runs one app frame with advancing animation time. A freshly shown
    /// floating window is hidden for its sizing pass and fades in, so the
    /// picker tests must drive several of these before reading widget rects.
    fn run_app_frame_animated(
        app: &mut App,
        ctx: &egui::Context,
        clock: &mut f64,
        events: Vec<egui::Event>,
    ) {
        let _ = run_app_frame_capturing(app, ctx, clock, events);
    }

    /// [`run_app_frame_animated`], but returns the paint output so tests can
    /// read painted shapes (text positions, marker strokes).
    fn run_app_frame_capturing(
        app: &mut App,
        ctx: &egui::Context,
        clock: &mut f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        *clock += 1.0 / 60.0;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            time: Some(*clock),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| app.ui_frame(ui));
        });
        output.textures_delta.clear();
        output
    }

    /// Drives the frames a floating panel needs to fade in and register its
    /// widgets before a test reads or clicks them.
    fn settle_color_picker(app: &mut App, ctx: &egui::Context, clock: &mut f64) {
        for _ in 0..8 {
            run_app_frame_animated(app, ctx, clock, Vec::new());
        }
    }

    #[test]
    fn color_picker_opens_with_c_and_closes_from_the_top_bar() {
        let mut app = App::default();
        let id = color_picker_panel_id();
        assert_eq!(app.panel_dock.placement(id), None);
        assert!(!app.color_picker_host.view.borrow().open);

        // Given: the C binding. When: it is pressed once.
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);

        // Then: the floating-only panel is registered with the Close action.
        assert_eq!(
            app.panel_dock.placement(id),
            Some(panel_dock::PanelPlacement::Floating)
        );
        assert!(!app.panel_dock.metadata(id).unwrap().can_pop_out);
        assert_eq!(
            app.panel_dock.header_action(id),
            Some(panel_dock::PanelHeaderAction::Close)
        );
        let view = *app.color_picker_host.view.borrow();
        assert!(view.open);
        assert_eq!(view.color, app.projects.current().color);
        assert_eq!(view.old_color, app.projects.current().color);

        // When: C is pressed again. Then: the panel closes.
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        assert_eq!(app.panel_dock.placement(id), None);
        assert!(!app.color_picker_host.view.borrow().open);

        // When: the panel is reopened and the top-bar Close action fires.
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        assert_eq!(
            app.panel_dock.placement(id),
            Some(panel_dock::PanelPlacement::Floating)
        );
        app.handle_dock_actions(vec![panel_dock::DockAction::ClosePanel(id)]);

        // Then: the panel and its open flag are cleared.
        assert_eq!(app.panel_dock.placement(id), None);
        assert!(!app.color_picker_host.view.borrow().open);
    }

    #[test]
    fn picking_in_the_square_sets_the_primary_color_live() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(Color::rgb(0, 0, 255))]);
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        settle_color_picker(&mut app, &ctx, &mut clock);

        // Given: the rendered HSV square.
        let square = ctx
            .read_response(egui::Id::new("color-picker-sv-square"))
            .expect("the color picker square renders")
            .rect;

        // When: the pointer presses at the square's center (S=0.5, V=0.5).
        let pick = square.center();
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(pick)]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(pick)]);

        // Then: the primary color updates in the same frame, keeping the hue.
        assert_eq!(app.projects.current().color, Color::rgb(64, 64, 128));
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![release(pick)]);
        assert_eq!(
            app.color_picker_host.view.borrow().color,
            Color::rgb(64, 64, 128),
            "the snapshot must follow the live primary color"
        );
    }

    #[test]
    fn hue_slider_and_square_are_bound() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(Color::rgb(255, 0, 0))]);
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        settle_color_picker(&mut app, &ctx, &mut clock);

        // Given: the rendered hue bar. When: it is pressed at its center.
        let hue_bar = ctx
            .read_response(egui::Id::new("color-picker-hue"))
            .expect("the hue slider renders")
            .rect;
        let hue_pick = hue_bar.center();
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(hue_pick)]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(hue_pick)]);

        // Then: the color rotates to hue 180 (cyan) at the same S/V.
        assert_eq!(app.projects.current().color, Color::rgb(0, 255, 255));

        // When: the square is picked at its center, the hue must be kept.
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![release(hue_pick)]);
        let square = ctx
            .read_response(egui::Id::new("color-picker-sv-square"))
            .expect("the square renders")
            .rect;
        let square_pick = square.center();
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(square_pick)]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(square_pick)]);

        // Then: the square's S/V applies while the slider's hue survives.
        assert_eq!(app.projects.current().color, Color::rgb(64, 128, 128));
        let (hue, saturation, value) =
            dock_color_picker_panel::color_to_hsv(app.projects.current().color);
        assert!(
            (hue - 180.0).abs() < 0.5,
            "the square pick must keep the slider hue: {hue}"
        );
        assert!((saturation - 0.5).abs() < 0.01);
        assert!((value - 0.5).abs() < 0.01);
    }

    #[test]
    fn rgb_hsl_alpha_hex_fields_stay_in_sync() {
        let mut app = App::default();
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        app.write_dock_snapshots();
        let opened_with = app.color_picker_host.view.borrow().old_color;

        // Given: the RGB fields. When: they commit rgb(10, 20, 30).
        let rgb = Color::rgb(10, 20, 30);
        commit_color_picker_color(&mut app, rgb);
        assert_eq!(app.projects.current().color, rgb);
        assert_eq!(dock_color_picker_panel::format_hex(rgb), "#0A141E");

        // The HSL fields reproduce the same color from its own HSL values.
        let (hue, saturation, lightness) = dock_color_picker_panel::color_to_hsl(rgb);
        commit_color_picker_color(
            &mut app,
            dock_color_picker_panel::hsl_to_color(hue, saturation, lightness, 255),
        );
        assert_eq!(
            app.projects.current().color,
            rgb,
            "the HSL fields must round-trip the RGB color"
        );
        let (hue, saturation, lightness) = dock_color_picker_panel::color_to_hsl(rgb);
        assert!((hue - 210.0).abs() < 0.5);
        assert!((saturation - 0.5).abs() < 0.01);
        assert!((lightness - 0.078431).abs() < 0.001);

        // When: the alpha field commits 128, only alpha changes.
        commit_color_picker_color(&mut app, Color::rgba(10, 20, 30, 128));
        assert_eq!(app.projects.current().color, Color::rgba(10, 20, 30, 128));
        assert_eq!(
            dock_color_picker_panel::format_hex(app.projects.current().color),
            "#0A141E",
            "hex ignores alpha"
        );

        // When: the hex field commits #112233, RGB/HSL follow.
        let parsed = dock_color_picker_panel::parse_hex("#112233").expect("valid hex");
        commit_color_picker_color(&mut app, parsed);
        assert_eq!(app.projects.current().color, Color::rgb(17, 34, 51));
        let (hue, saturation, lightness) =
            dock_color_picker_panel::color_to_hsl(Color::rgb(17, 34, 51));
        assert_eq!(
            dock_color_picker_panel::hsl_to_color(hue, saturation, lightness, 255),
            Color::rgb(17, 34, 51)
        );

        // Then: the previews and the snapshot follow the live color.
        let view = *app.color_picker_host.view.borrow();
        assert_eq!(view.color, Color::rgb(17, 34, 51));
        assert_eq!(view.old_color, opened_with);
        assert!(view.open);
    }

    #[test]
    fn color_picker_previews_revert_and_restore_the_primary_color() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        let opened_with = Color::rgb(255, 0, 0);
        let working = Color::rgb(0, 0, 255);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(opened_with)]);
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        settle_color_picker(&mut app, &ctx, &mut clock);

        // Given: a picker edit to `working`, applied live.
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(working)]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, Vec::new());
        assert_eq!(app.projects.current().color, working);
        assert_eq!(app.color_picker_host.view.borrow().new_color, working);

        // When: the Old preview is clicked.
        let old = ctx
            .read_response(egui::Id::new(("color-picker-preview", "Old")))
            .expect("the Old preview renders")
            .rect;
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(old.center())]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(old.center())]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![release(old.center())]);

        // Then: the primary reverts and New keeps the working color as target.
        assert_eq!(
            app.projects.current().color,
            opened_with,
            "clicking Old must revert the primary"
        );
        assert_eq!(app.color_picker_host.view.borrow().new_color, working);

        // When: the New preview is clicked.
        let new = ctx
            .read_response(egui::Id::new(("color-picker-preview", "New")))
            .expect("the New preview renders")
            .rect;
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(new.center())]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(new.center())]);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![release(new.center())]);

        // Then: the working color is restored.
        assert_eq!(
            app.projects.current().color,
            working,
            "clicking New must restore the working color"
        );
    }

    #[test]
    fn x_hotkey_swaps_primary_and_secondary() {
        let mut app = App::default();
        let primary = Color::rgb(10, 20, 30);
        let secondary = Color::rgb(200, 210, 220);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(primary)]);
        app.apply_toolbar_events(vec![ToolbarEvent::SecondaryColorChanged(secondary)]);

        // When: plain X is pressed. Then: the two colors swap.
        send_key(&mut app, egui::Key::X, egui::Modifiers::NONE);
        assert_eq!(app.projects.current().color, secondary);
        assert_eq!(app.projects.current().secondary_color, primary);
    }

    #[test]
    fn color_picker_reopens_at_its_last_position() {
        let mut app = App::default();
        let id = color_picker_panel_id();
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        let moved = egui::Rect::from_min_size(egui::pos2(260.0, 140.0), egui::vec2(540.0, 300.0));
        app.panel_dock.set_floating_rect(id, moved);

        // When: the panel is closed and reopened.
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);
        assert_eq!(app.panel_dock.placement(id), None);
        send_key(&mut app, egui::Key::C, egui::Modifiers::NONE);

        // Then: it reopens at the remembered position with its fixed height
        // (the picker's spec normalizes the height to its content).
        let expected = egui::Rect::from_min_size(
            moved.min,
            egui::vec2(moved.width(), dock_color_picker_panel::PICKER_WINDOW_HEIGHT),
        );
        assert_eq!(app.panel_dock.floating_rect(id), Some(expected));

        // And: the fixed-size window settles at that rect.
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        settle_color_picker(&mut app, &ctx, &mut clock);
        let reopened = app.panel_dock.floating_rect(id).expect("the picker floats");
        assert!(
            (reopened.min - moved.min).length() < 1.0,
            "reopened at {reopened:?}, expected {moved:?}"
        );
    }

    /// The Palette panel's dock id.
    fn palette_panel_id() -> panel_dock::PanelId {
        panel_dock::PanelId::new(dock_color_palette_panel::PALETTE_PANEL_ID)
    }

    /// Replaces the active palette and marks the project saved, so palette
    /// assertions start from a clean baseline.
    fn set_active_palette(app: &mut App, colors: &[Color]) {
        let session = app.projects.current_mut();
        session.palettes = vec![Palette::new(
            "Test",
            colors
                .iter()
                .map(|color| [color.r, color.g, color.b, color.a])
                .collect(),
        )
        .expect("test palette is valid")];
        session.active_palette = 0;
        session.mark_saved();
    }

    /// Position of the first painted text equal to `needle`.
    fn painted_text_pos(output: &egui::FullOutput, needle: &str) -> Option<egui::Pos2> {
        rendered_texts(output)
            .into_iter()
            .find(|(text, _)| text == needle)
            .map(|(_, pos)| pos)
    }

    /// The painted rect of the swatch filled with `color`.
    fn swatch_rect(output: &egui::FullOutput, color: Color) -> Option<egui::Rect> {
        let fill = egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a);
        fn visit(shape: &egui::Shape, fill: egui::Color32, found: &mut Option<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.fill == fill && found.is_none() => {
                    *found = Some(rect.rect);
                }
                egui::Shape::Vec(shapes) => {
                    shapes.iter().for_each(|shape| visit(shape, fill, found));
                }
                _ => {}
            }
        }
        let mut found = None;
        output
            .shapes
            .iter()
            .for_each(|clipped| visit(&clipped.shape, fill, &mut found));
        found
    }

    /// Rects of every painted rect shape whose stroke uses `color`.
    fn stroked_rects(output: &egui::FullOutput, color: egui::Color32) -> Vec<egui::Rect> {
        fn visit(shape: &egui::Shape, color: egui::Color32, out: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.stroke.color == color => out.push(rect.rect),
                egui::Shape::Vec(shapes) => {
                    shapes.iter().for_each(|shape| visit(shape, color, out));
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        output
            .shapes
            .iter()
            .for_each(|clipped| visit(&clipped.shape, color, &mut out));
        out
    }

    fn pointer_button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A full click (hover, press, release) at `pos` with `button`.
    fn click_app_at(
        app: &mut App,
        ctx: &egui::Context,
        clock: &mut f64,
        pos: egui::Pos2,
        button: egui::PointerButton,
    ) {
        run_app_frame_animated(app, ctx, clock, vec![move_to(pos)]);
        run_app_frame_animated(app, ctx, clock, vec![pointer_button(pos, button, true)]);
        run_app_frame_animated(app, ctx, clock, vec![pointer_button(pos, button, false)]);
    }

    /// The screen rect of the Palette panel (docked right).
    fn palette_panel_rect(app: &App, screen: egui::Vec2) -> egui::Rect {
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), screen);
        let right = egui::Rect::from_min_max(
            egui::pos2(
                viewport.right() - app.panel_dock.dock_extent(panel_dock::DockSide::Right),
                viewport.top(),
            ),
            viewport.max,
        );
        app.panel_dock
            .panel_rects(panel_dock::DockSide::Right, right)
            .into_iter()
            .find(|(id, _)| *id == palette_panel_id())
            .expect("the Palette panel is docked right")
            .1
    }

    #[test]
    fn palette_panel_is_docked_right_by_default() {
        let mut app = App::default();
        let id = palette_panel_id();
        let metadata = app
            .panel_dock
            .metadata(id)
            .expect("the Palette panel is registered");
        assert_eq!(metadata.title, "Palette");
        assert!(metadata.can_pop_out, "the Palette panel must be floatable");
        assert_eq!(
            app.panel_dock.placement(id),
            Some(panel_dock::PanelPlacement::DockedRight)
        );
        assert_eq!(
            app.panel_dock.header_action(id),
            Some(panel_dock::PanelHeaderAction::Float)
        );
        assert!(
            app.panel_dock
                .dock_order(panel_dock::DockSide::Right)
                .contains(&id),
            "the Palette panel must sit in the right dock"
        );

        app.panel_dock
            .transition_panel(id, panel_dock::PanelPlacement::Floating)
            .expect("a dockable panel floats");
        assert_eq!(
            app.panel_dock.placement(id),
            Some(panel_dock::PanelPlacement::Floating)
        );
        assert_eq!(
            app.panel_dock.header_action(id),
            Some(panel_dock::PanelHeaderAction::Dock)
        );
        app.panel_dock
            .transition_panel(id, panel_dock::PanelPlacement::DockedRight)
            .expect("a floating dockable panel docks back");
        assert_eq!(
            app.panel_dock.placement(id),
            Some(panel_dock::PanelPlacement::DockedRight)
        );
    }

    #[test]
    fn palette_add_and_remove_buttons_are_pinned_and_edit_the_active_palette() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        let first = Color::rgb(10, 20, 30);
        let second = Color::rgb(40, 50, 60);
        let primary = Color::rgb(200, 0, 0);
        set_active_palette(&mut app, &[first, second]);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(primary)]);

        // Given: the rendered Palette panel. When: "Add color" is clicked.
        // Then: the PRIMARY color is appended to the active palette.
        let output = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let add = painted_text_pos(&output, "Add color").expect("the Add button paints");
        click_app_at(
            &mut app,
            &ctx,
            &mut clock,
            add + egui::vec2(4.0, 8.0),
            egui::PointerButton::Primary,
        );
        run_app_frame_animated(&mut app, &ctx, &mut clock, Vec::new());
        let palette = &app.projects.current().palettes[0];
        assert_eq!(
            palette.colors,
            vec![[10, 20, 30, 255], [40, 50, 60, 255], [200, 0, 0, 255]],
            "Add must append the primary color to the active palette"
        );
        assert_eq!(*app.palette_host.selected.borrow(), Some(2));
        assert!(
            app.projects.current().is_dirty(),
            "a palette edit marks the project dirty"
        );

        // When: "Remove color" is clicked. Then: the selected entry (the
        // appended one) is dropped from the active palette.
        let output = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let remove = painted_text_pos(&output, "Remove color").expect("the Remove button paints");
        click_app_at(
            &mut app,
            &ctx,
            &mut clock,
            remove + egui::vec2(4.0, 8.0),
            egui::PointerButton::Primary,
        );
        run_app_frame_animated(&mut app, &ctx, &mut clock, Vec::new());
        assert_eq!(
            app.projects.current().palettes[0].colors,
            vec![[10, 20, 30, 255], [40, 50, 60, 255]],
            "Remove must drop the selected palette entry"
        );

        // Given: a palette long enough to overflow its docked panel. When: the
        // swatch grid is scrolled. Then: the header buttons stay pinned.
        let screen = egui::vec2(800.0, 600.0);
        let many: Vec<Color> = (0..200u8).map(|index| Color::rgb(index, 0, 255)).collect();
        set_active_palette(&mut app, &many);
        let output = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let add_before = painted_text_pos(&output, "Add color").expect("Add stays visible");
        let swatch_before = swatch_rect(&output, many[0]).expect("swatch 0 renders").min;
        let panel_center = palette_panel_rect(&app, screen).center();

        run_app_frame_animated(
            &mut app,
            &ctx,
            &mut clock,
            vec![
                move_to(panel_center),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -7.0),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
        );
        let output = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let add_after = painted_text_pos(&output, "Add color").expect("Add stays visible");
        let swatch_after = swatch_rect(&output, many[0])
            .expect("swatch 0 still renders")
            .min;
        assert!(
            (add_after - add_before).length() < 0.01,
            "the header buttons must not scroll: {add_before:?} -> {add_after:?}"
        );
        assert!(
            swatch_after.y < swatch_before.y,
            "the swatches must scroll under the pinned header: {swatch_before:?} -> {swatch_after:?}"
        );
    }

    #[test]
    fn left_click_sets_the_primary_color() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        let entries = [
            Color::rgb(10, 20, 30),
            Color::rgb(40, 50, 60),
            Color::rgb(70, 80, 90),
        ];
        set_active_palette(&mut app, &entries);

        let output = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let swatch = swatch_rect(&output, entries[1]).expect("swatch 1 renders");
        click_app_at(
            &mut app,
            &ctx,
            &mut clock,
            swatch.center(),
            egui::PointerButton::Primary,
        );

        assert_eq!(app.projects.current().color, entries[1]);
        assert_eq!(
            *app.palette_host.selected.borrow(),
            Some(1),
            "the clicked swatch becomes the Remove target"
        );
    }

    #[test]
    fn right_click_sets_the_secondary_color() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        let entries = [
            Color::rgb(10, 20, 30),
            Color::rgb(40, 50, 60),
            Color::rgb(70, 80, 90),
        ];
        set_active_palette(&mut app, &entries);
        assert_eq!(app.projects.current().secondary_color, Color::WHITE);

        let output = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let swatch = swatch_rect(&output, entries[2]).expect("swatch 2 renders");
        click_app_at(
            &mut app,
            &ctx,
            &mut clock,
            swatch.center(),
            egui::PointerButton::Secondary,
        );

        assert_eq!(app.projects.current().secondary_color, entries[2]);
        assert_eq!(
            app.projects.current().color,
            Color::BLACK,
            "a right click must not change the primary color"
        );
        assert_eq!(*app.palette_host.selected.borrow(), Some(2));
    }

    #[test]
    fn palette_marks_the_primary_and_secondary_swatches() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        let entries = [
            Color::rgb(10, 20, 30),
            Color::rgb(40, 50, 60),
            Color::rgb(70, 80, 90),
        ];
        set_active_palette(&mut app, &entries);
        app.apply_toolbar_events(vec![
            ToolbarEvent::ColorChanged(entries[1]),
            ToolbarEvent::SecondaryColorChanged(entries[2]),
        ]);

        let output = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let swatches: Vec<egui::Rect> = entries
            .iter()
            .map(|color| swatch_rect(&output, *color).expect("every swatch renders"))
            .collect();
        let marks = stroked_rects(
            &output,
            app.theme.current().colors.selection_stroke_color32(),
        );

        assert!(
            marks.iter().any(|mark| {
                (mark.center() - swatches[1].center()).length() < 0.5
                    && (mark.width() - swatches[1].width()).abs() < 0.5
            }),
            "the primary-matching swatch must carry an outer marker: {marks:?} vs {swatches:?}"
        );
        assert!(
            marks.iter().any(|mark| {
                (mark.center() - swatches[2].center()).length() < 0.5
                    && mark.width() < swatches[2].width() - 1.0
                    && mark.width() > 1.0
            }),
            "the secondary-matching swatch must carry a smaller inner marker: {marks:?} vs {swatches:?}"
        );
        assert!(
            !marks
                .iter()
                .any(|mark| (mark.center() - swatches[0].center()).length() < 0.5),
            "an unmatched swatch must not be marked: {marks:?} vs {swatches:?}"
        );
    }

    #[test]
    fn palette_content_follows_the_panel_width() {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let mut clock = 0.0;
        let entries: Vec<Color> = (0..40).map(|index| Color::rgb(index * 5, 0, 255)).collect();
        set_active_palette(&mut app, &entries);
        let screen = egui::vec2(800.0, 600.0);

        // Wide (default right dock): neighbouring swatches share a row.
        let wide = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let wide0 = swatch_rect(&wide, entries[0]).expect("swatch 0 renders");
        let wide1 = swatch_rect(&wide, entries[1]).expect("swatch 1 renders");
        assert!(
            (wide0.center().y - wide1.center().y).abs() < 1.0
                && wide1.center().x > wide0.center().x,
            "a wide panel lays swatches out in a row: {wide0:?} vs {wide1:?}"
        );

        // Narrow-but-usable dock (40pt panel): the grid wraps to one column and
        // every swatch stays inside the panel.
        let extent_before = app.panel_dock.dock_extent(panel_dock::DockSide::Right);
        app.panel_dock.resize_dock_area(
            panel_dock::DockSide::Right,
            -(extent_before - 40.0),
            screen,
        );
        assert_eq!(
            app.panel_dock.dock_extent(panel_dock::DockSide::Right),
            40.0
        );
        let narrow = run_app_frame_capturing(&mut app, &ctx, &mut clock, Vec::new());
        let narrow0 =
            swatch_rect(&narrow, entries[0]).expect("swatch 0 renders in the narrow dock");
        let narrow1 =
            swatch_rect(&narrow, entries[1]).expect("swatch 1 renders in the narrow dock");
        assert!(
            (narrow0.center().x - narrow1.center().x).abs() < 1.0,
            "a 40pt panel leaves room for one swatch column: {narrow0:?} vs {narrow1:?}"
        );
        assert!(
            narrow1.center().y > narrow0.center().y,
            "the second swatch must wrap below the first: {narrow0:?} vs {narrow1:?}"
        );
        let panel_rect = palette_panel_rect(&app, screen);
        for rect in [narrow0, narrow1] {
            assert!(
                rect.right() <= panel_rect.right() + 0.5,
                "swatches must follow the panel width: {rect:?} vs {panel_rect:?}"
            );
        }
    }

    #[test]
    fn app_settings_toggle_seeds_and_clears_the_rename_buffer() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        app.projects
            .current_mut()
            .layers
            .layer_mut(lid)
            .unwrap()
            .name = "Sketch".to_string();

        // Opening the popup seeds the buffer with the layer's current name.
        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        assert_eq!(*app.layers_host.rename_buffer.borrow(), "Sketch");

        // Closing the popup clears the buffer.
        app.apply_layer_events(vec![LayerPanelEvent::SettingsToggle(lid)]);
        assert!(app.layers_host.rename_buffer.borrow().is_empty());
    }

    #[test]
    fn app_rename_event_updates_the_layer_name() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        app.apply_layer_events(vec![LayerPanelEvent::Rename(lid, "Sketch".to_string())]);
        assert_eq!(
            app.projects.current().layers.layer(lid).unwrap().name,
            "Sketch"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            1,
            "rename is one undo step"
        );
    }

    #[test]
    fn app_drag_drop_reorder_moves_the_dragged_layer_next_to_the_target() {
        // Dragged below the target (in display order) lands just above it.
        let mut app = App::default();
        let session = app.projects.current_mut();
        let root = session.layers.active_layer_id();
        let a = session.layers.add_layer("A");
        let b = session.layers.add_layer("B");
        let c = session.layers.add_layer("C");
        // Model (bottom-to-top): [root, A, B, C]; display (top-first): [C, B, A, root].
        app.apply_layer_events(vec![LayerPanelEvent::ReorderDropped {
            dragged: a,
            target: c,
        }]);
        assert_eq!(
            app.projects.current().layers.structure(),
            vec![(root, None), (b, None), (c, None), (a, None)],
            "A (below C in display) lands just above C"
        );

        // Dragged above the target (in display order) lands just below it.
        let mut app = App::default();
        let session = app.projects.current_mut();
        let root = session.layers.active_layer_id();
        let a = session.layers.add_layer("A");
        let b = session.layers.add_layer("B");
        let c = session.layers.add_layer("C");
        app.apply_layer_events(vec![LayerPanelEvent::ReorderDropped {
            dragged: c,
            target: a,
        }]);
        assert_eq!(
            app.projects.current().layers.structure(),
            vec![(root, None), (c, None), (a, None), (b, None)],
            "C (above A in display) lands just below A"
        );
    }

    #[test]
    fn app_drag_drop_swaps_two_layers_in_both_directions() {
        // Dragging the LOWER of two layers onto the UPPER one swaps the pair:
        // A (below B in display) lands just above B.
        let mut app = App::default();
        let session = app.projects.current_mut();
        let root = session.layers.active_layer_id();
        let a = session.layers.add_layer("A");
        let b = session.layers.add_layer("B");
        // Model (bottom-to-top): [root, A, B]; display (top-first): [B, A, root].
        app.apply_layer_events(vec![LayerPanelEvent::ReorderDropped {
            dragged: a,
            target: b,
        }]);
        assert_eq!(
            app.projects.current().layers.structure(),
            vec![(root, None), (b, None), (a, None)],
            "A (below B in display) lands just above B: the pair swaps"
        );

        // The reverse drag (UPPER onto LOWER) swaps them the same way: B
        // (above A in display) lands just below A.
        let mut app = App::default();
        let session = app.projects.current_mut();
        let root = session.layers.active_layer_id();
        let a = session.layers.add_layer("A");
        let b = session.layers.add_layer("B");
        app.apply_layer_events(vec![LayerPanelEvent::ReorderDropped {
            dragged: b,
            target: a,
        }]);
        assert_eq!(
            app.projects.current().layers.structure(),
            vec![(root, None), (b, None), (a, None)],
            "B (above A in display) lands just below A: the pair swaps"
        );
    }

    #[test]
    fn app_opacity_stream_coalesces_into_one_undo_step() {
        let mut app = App::default();
        let lid = app.projects.current_mut().layers.active_layer_id();
        // Two consecutive opacity gestures in one frame's event stream.
        app.layers_host
            .events
            .borrow_mut()
            .push(LayerPanelEvent::SetOpacity(lid, 0.5));
        app.layers_host
            .events
            .borrow_mut()
            .push(LayerPanelEvent::SetOpacity(lid, 0.3));
        app.drain_dock_events();
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .layer(lid)
                .unwrap()
                .opacity,
            0.3
        );
        assert!(
            app.layers_host.events.borrow().is_empty(),
            "events drain exactly once per frame"
        );
    }

    #[test]
    fn app_merge_down_matches_composite_layers() {
        let mut app = App::default();
        let session = app.projects.current_mut();
        let bottom = session.layers.active_layer_id();
        let top = session.layers.add_layer("Top");
        session.layers.set_active(top);
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.projects.current_mut().layers.set_active(bottom);
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        app.projects.current_mut().layers.set_active(top);

        let before = app
            .projects
            .current()
            .layers
            .composite_layers()
            .as_bytes()
            .to_vec();
        app.apply_layer_events(vec![LayerPanelEvent::MergeDown]);
        let after = app
            .projects
            .current()
            .layers
            .composite_layers()
            .as_bytes()
            .to_vec();
        assert_eq!(after, before, "merging composites the top over the bottom");
        assert_eq!(app.projects.current().layers.len(), 1);
        assert_eq!(app.projects.current().layers.active_layer_id(), bottom);
    }

    #[test]
    fn layer_remove_clears_selection() {
        let mut app = App::default();
        app.projects.current_mut().layers.add_layer("Layer 2");
        marquee(&mut app, (2, 2), (5, 5));
        assert!(app.projects.current_mut().selection.is_some());
        let lid = app.projects.current_mut().layers.active_layer_id();
        app.apply_layer_events(vec![LayerPanelEvent::Remove(lid)]);
        assert!(app.projects.current_mut().selection.is_none());
        assert_eq!(app.projects.current_mut().layers.len(), 1);
    }

    #[test]
    fn shift_scroll_resizes_the_pen_brush_and_other_tools_ignore_it() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Pencil);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .size = 2;

        app.handle_interactions(CanvasInteractions {
            brush_scroll: 40.0,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).size,
            3,
            "scroll up grows the pen"
        );

        app.handle_interactions(CanvasInteractions {
            brush_scroll: -40.0,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).size,
            2,
            "scroll down shrinks the pen"
        );

        for _ in 0..100 {
            app.handle_interactions(CanvasInteractions {
                brush_scroll: 40.0,
                ..Default::default()
            });
        }
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).size,
            BrushSpec::MAX_SIZE
        );

        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Fill);
        app.handle_interactions(CanvasInteractions {
            brush_scroll: -40.0,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).size,
            BrushSpec::MAX_SIZE,
            "only the Draw tools resize on Shift+scroll"
        );
    }

    #[test]
    fn shift_scroll_resizes_the_brush_for_both_draw_tools() {
        for tool in [Tool::Pencil, Tool::Eraser] {
            let mut app = App::default();
            app.projects.current_mut().tool_state.select_tool(tool);
            app.projects.current_mut().draw_settings_mut(tool).size = 2;

            app.handle_interactions(CanvasInteractions {
                brush_scroll: 40.0,
                ..Default::default()
            });
            assert_eq!(
                app.projects.current().draw_settings(tool).size,
                3,
                "{tool:?}: one notch grows the brush by one step"
            );

            app.handle_interactions(CanvasInteractions {
                brush_scroll: -40.0,
                ..Default::default()
            });
            assert_eq!(
                app.projects.current().draw_settings(tool).size,
                2,
                "{tool:?}: one notch shrinks the brush by one step"
            );

            for _ in 0..100 {
                app.handle_interactions(CanvasInteractions {
                    brush_scroll: 40.0,
                    ..Default::default()
                });
            }
            assert_eq!(
                app.projects.current().draw_settings(tool).size,
                BrushSpec::MAX_SIZE,
                "{tool:?}: grows clamp at 64"
            );

            for _ in 0..100 {
                app.handle_interactions(CanvasInteractions {
                    brush_scroll: -40.0,
                    ..Default::default()
                });
            }
            assert_eq!(
                app.projects.current().draw_settings(tool).size,
                BrushSpec::MIN_SIZE,
                "{tool:?}: shrinks clamp at 1"
            );
        }
    }

    #[test]
    fn ctrl_scroll_sets_the_brush_shape_directionally_without_zooming_or_resizing() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Pencil);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .size = 5;
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .shape = BrushShape::Square;

        app.handle_interactions(CanvasInteractions {
            shape_cycle: 1,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Round,
            "Ctrl+scroll up selects the round shape"
        );
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).size,
            5,
            "Ctrl+scroll must not resize the brush"
        );
        assert_eq!(
            app.projects.current().camera.zoom_percent(),
            100,
            "Ctrl+scroll must not zoom"
        );

        app.handle_interactions(CanvasInteractions {
            shape_cycle: -1,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Square,
            "Ctrl+scroll down selects the square shape"
        );

        app.handle_interactions(CanvasInteractions {
            shape_cycle: 0,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Square,
            "a zero net step count changes nothing"
        );

        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Eraser);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Eraser)
            .shape = BrushShape::Square;
        app.handle_interactions(CanvasInteractions {
            shape_cycle: 1,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Eraser).shape,
            BrushShape::Round,
            "the active tool's own bundle is set"
        );
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Square,
            "the inactive tool's shape must be untouched"
        );
    }

    #[test]
    fn ctrl_scroll_up_sets_the_round_shape() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Pencil);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .shape = BrushShape::Square;
        for _ in 0..5 {
            app.handle_interactions(CanvasInteractions {
                shape_cycle: 1,
                ..Default::default()
            });
            assert_eq!(
                app.projects.current().draw_settings(Tool::Pencil).shape,
                BrushShape::Round,
                "scrolling up repeatedly must stay round"
            );
        }
    }

    #[test]
    fn ctrl_scroll_down_sets_the_square_shape() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Pencil);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .shape = BrushShape::Round;
        for _ in 0..5 {
            app.handle_interactions(CanvasInteractions {
                shape_cycle: -1,
                ..Default::default()
            });
            assert_eq!(
                app.projects.current().draw_settings(Tool::Pencil).shape,
                BrushShape::Square,
                "scrolling down repeatedly must stay square"
            );
        }
    }

    #[test]
    fn shape_cycle_is_ignored_by_non_draw_tools() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Fill);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .shape = BrushShape::Round;
        app.handle_interactions(CanvasInteractions {
            shape_cycle: 1,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Round,
            "only the Draw tools cycle their brush shape"
        );
    }

    #[test]
    fn wheel_resize_never_toggles_the_brush_shape() {
        for shape in [BrushShape::Square, BrushShape::Round] {
            let mut app = App::default();
            app.projects
                .current_mut()
                .tool_state
                .select_tool(Tool::Pencil);
            app.projects
                .current_mut()
                .draw_settings_mut(Tool::Pencil)
                .size = 3;
            app.projects
                .current_mut()
                .draw_settings_mut(Tool::Pencil)
                .shape = shape;

            for delta in [40.0, -40.0, 40.0, 40.0, -40.0, -40.0] {
                app.handle_interactions(CanvasInteractions {
                    brush_scroll: delta,
                    ..Default::default()
                });
                assert_eq!(
                    app.projects.current().draw_settings(Tool::Pencil).shape,
                    shape,
                    "the wheel gesture must only change the size, never the shape"
                );
            }
            assert_eq!(app.projects.current().draw_settings(Tool::Pencil).size, 3);
        }
    }

    #[test]
    fn select_other_layer_clears_selection() {
        let mut app = App::default();
        let other = app.projects.current_mut().layers.add_layer("Layer 2");
        marquee(&mut app, (2, 2), (5, 5));
        assert!(app.projects.current_mut().selection.is_some());
        app.apply_layer_events(vec![LayerPanelEvent::Select(other)]);
        assert!(app.projects.current_mut().selection.is_none());
        assert_eq!(app.projects.current_mut().layers.active_layer_id(), other);
    }

    #[test]
    fn undo_clears_stale_selection() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (5, 5));
        assert!(app.projects.current_mut().selection.is_some());
        app.undo_document();
        assert!(app.projects.current_mut().selection.is_none());
    }

    #[test]
    fn toolbar_brush_size_sanitized() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::BrushSizeChanged(100)]);
        // The toolbar stores the raw slider value; sanitization to 1..=64
        // happens when the brush is built for a stroke (BrushSpec::sanitize).
        assert_eq!(app.projects.current().draw_settings(Tool::Pencil).size, 100);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        // A sanitized size-64 square stamp at (4,4) covers x in -28..=35
        // (centered footprint); a raw size-100 stamp would reach x=53.
        assert_eq!(buf.get_pixel(35, 4), Some(RED));
        assert_eq!(buf.get_pixel(36, 4), Some(Color::TRANSPARENT));
    }

    #[test]
    fn app_applies_brush_shape_changed_to_session() {
        let mut app = App::default();
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Square
        );
        app.apply_toolbar_events(vec![ToolbarEvent::BrushShapeChanged(BrushShape::Round)]);
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Round
        );
        app.apply_toolbar_events(vec![ToolbarEvent::BrushShapeChanged(BrushShape::Square)]);
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Square
        );
    }

    #[test]
    fn begin_stroke_uses_session_brush_shape() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .size = 3;
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .shape = BrushShape::Round;
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        // Round size 3 = the plus shape: the center and its four orthogonal
        // neighbors.
        for (x, y) in [(4, 4), (3, 4), (5, 4), (4, 3), (4, 5)] {
            assert_eq!(buf.get_pixel(x, y), Some(RED), "({x},{y}) must be painted");
        }
        // The square corners stay clear (a 3×3 block would paint them).
        for (x, y) in [(3, 3), (5, 3), (3, 5), (5, 5)] {
            assert_eq!(
                buf.get_pixel(x, y),
                Some(Color::TRANSPARENT),
                "({x},{y}) must stay clear"
            );
        }
    }

    #[test]
    fn stroke_uses_the_active_tools_settings() {
        let mut app = App::default();
        {
            let pen = app.projects.current_mut().draw_settings_mut(Tool::Pencil);
            pen.size = 5;
            pen.shape = BrushShape::Round;
        }
        {
            let eraser = app.projects.current_mut().draw_settings_mut(Tool::Eraser);
            eraser.size = 1;
            eraser.shape = BrushShape::Square;
        }
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .fill(RED);

        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Eraser);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((10, 10)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((10, 10)),
            ..Default::default()
        });

        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(10, 10), Some(Color::TRANSPARENT));
        assert_eq!(
            buf.get_pixel(11, 10),
            Some(RED),
            "the eraser's 1 px footprint must not borrow the pen's 5 px footprint"
        );
        assert_eq!(buf.get_pixel(12, 10), Some(RED));
    }

    #[test]
    fn tool_property_panel_edits_the_active_tools_settings() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .size = 3;
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Eraser)
            .size = 9;

        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Eraser)]);
        app.write_dock_snapshots();
        assert_eq!(
            app.toolbox_host.view.borrow().draw.size,
            9,
            "the panel snapshot must show the ACTIVE (eraser) tool's size"
        );

        app.apply_toolbar_events(vec![ToolbarEvent::BrushSizeChanged(12)]);
        assert_eq!(app.projects.current().draw_settings(Tool::Eraser).size, 12);
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).size,
            3,
            "the pen's size must be untouched by the eraser's panel edit"
        );

        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Pencil)]);
        app.write_dock_snapshots();
        assert_eq!(
            app.toolbox_host.view.borrow().draw.size,
            3,
            "switching back shows the pen's own size in the panel snapshot"
        );
    }

    #[test]
    fn fieldier_child_event_updates_tool_state() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::FieldierChildChanged(
            FieldierChild::Wand,
        )]);
        let state = app.projects.current().tool_state;
        assert_eq!(
            state.tool(),
            Tool::Fieldier,
            "picking a Fieldier child selects the Fieldier tool"
        );
        assert_eq!(state.child(), FieldierChild::Wand);
        assert_eq!(
            state.previous(),
            None,
            "a deliberate child pick clears the restore"
        );
    }

    #[test]
    fn wand_tolerance_event_updates_settings() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::WandToleranceChanged(42)]);
        assert_eq!(app.projects.current().tool_state.wand().tolerance, 42);
    }

    #[test]
    fn wand_restrict_event_updates_settings() {
        let mut app = App::default();
        assert!(!app.projects.current().tool_state.wand().restrict_to_region);
        app.apply_toolbar_events(vec![ToolbarEvent::WandRestrictToRegionChanged(true)]);
        assert!(app.projects.current().tool_state.wand().restrict_to_region);
    }

    #[test]
    fn write_dock_snapshots_carries_child_and_wand() {
        let mut app = App::default();
        {
            let state = &mut app.projects.current_mut().tool_state;
            state.select_child(FieldierChild::Lasso);
            state.wand_mut().tolerance = 9;
            state.wand_mut().restrict_to_region = true;
        }

        app.write_dock_snapshots();

        let view = app.toolbox_host.view.borrow();
        assert_eq!(view.child, FieldierChild::Lasso);
        assert_eq!(
            view.wand,
            WandSettings {
                contiguous: true,
                tolerance: 9,
                restrict_to_region: true,
            }
        );
    }

    /// W2: the Tool Property snapshot reports a live transform (and its
    /// algorithm); with no transform the tool path is the only state.
    #[test]
    fn write_dock_snapshots_reports_transform_state() {
        let mut app = App::default();
        app.write_dock_snapshots();
        {
            let view = app.toolbox_host.view.borrow();
            assert!(!view.transform_active, "no session → tool label path");
            assert_eq!(view.transform_algorithm, TransformAlgorithm::RotSprite);
        }

        app.projects.current_mut().transform_algorithm = TransformAlgorithm::Rotxel;
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        app.write_dock_snapshots();
        {
            let view = app.toolbox_host.view.borrow();
            assert!(
                view.transform_active,
                "a live transform must flag the panel"
            );
            assert_eq!(view.transform_algorithm, TransformAlgorithm::Rotxel);
        }

        // Cancelling returns the panel to the tool path while the stored
        // algorithm survives for the next lift.
        app.cancel_transform();
        app.write_dock_snapshots();
        {
            let view = app.toolbox_host.view.borrow();
            assert!(!view.transform_active, "no session → tool label path");
            assert_eq!(view.transform_algorithm, TransformAlgorithm::Rotxel);
        }
    }

    /// W2: a `TransformAlgorithmChanged` event writes the session field and
    /// applies the algorithm to a live selection object immediately.
    #[test]
    fn transform_algorithm_event_updates_session_and_live_object() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        assert_eq!(
            app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .object
                .algorithm(),
            TransformAlgorithm::RotSprite,
            "a lifted transform starts on the default algorithm"
        );

        app.apply_toolbar_events(vec![ToolbarEvent::TransformAlgorithmChanged(
            TransformAlgorithm::CleanEdge,
        )]);
        assert_eq!(
            app.projects.current().transform_algorithm,
            TransformAlgorithm::CleanEdge
        );
        assert_eq!(
            app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .object
                .algorithm(),
            TransformAlgorithm::CleanEdge,
            "the live object must switch immediately"
        );

        // With no transform the event still stores the choice.
        app.cancel_transform();
        app.apply_toolbar_events(vec![ToolbarEvent::TransformAlgorithmChanged(
            TransformAlgorithm::Rotxel,
        )]);
        assert_eq!(
            app.projects.current().transform_algorithm,
            TransformAlgorithm::Rotxel
        );
    }

    /// W2 (c): a freshly lifted selection seeds the object's algorithm from the
    /// session so the selector is honoured on lift → cancel → lift.
    #[test]
    fn lifted_transform_seeds_algorithm_from_session() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.projects.current_mut().transform_algorithm = TransformAlgorithm::Rotxel;
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        assert_eq!(
            app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .object
                .algorithm(),
            TransformAlgorithm::Rotxel,
            "the lift must seed the object from the session algorithm"
        );
    }

    /// Y2-B: the preview algorithm choice is fast while dragging (regardless
    /// of the selected algorithm) and the selected one when idle.
    #[test]
    fn transform_preview_algorithm_is_fast_while_dragging_and_selected_when_idle() {
        assert_eq!(
            TRANSFORM_DRAG_PREVIEW_ALGORITHM,
            TransformAlgorithm::Rotxel,
            "the fast drag path is Rotxel"
        );
        for selected in [
            TransformAlgorithm::RotSprite,
            TransformAlgorithm::CleanEdge,
            TransformAlgorithm::Rotxel,
        ] {
            assert_eq!(
                transform_preview_algorithm(true, selected),
                TransformAlgorithm::Rotxel,
                "dragging must force the fast algorithm over {selected:?}"
            );
            assert_eq!(
                transform_preview_algorithm(false, selected),
                selected,
                "idle must use the selected algorithm {selected:?}"
            );
        }
    }

    /// Y2-B (d): the drag flag is set by a press through the real interaction
    /// path and cleared by the release; it is false with no drag in flight.
    #[test]
    fn transform_preview_drag_flag_follows_press_and_release() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        assert!(
            !app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .preview_dragging,
            "no drag in flight on lift"
        );

        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert!(
                t.preview_dragging,
                "a latched press starts the drag preview"
            );
            assert_eq!(t.drag, GizmoHit::Translate);
        }

        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((6, 6)),
            ..Default::default()
        });
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert!(!t.preview_dragging, "release clears the drag flag");
            assert_eq!(t.drag, GizmoHit::None);
        }
    }

    /// Y2-B: the dirty key tracks the drag state, so a release forces the next
    /// preview frame to re-render with the selected algorithm even when the
    /// transform state is unchanged.
    #[test]
    fn transform_preview_key_changes_with_drag_state() {
        let mut app = App::default();
        app.lift_transform(Rect2i::new(0, 0, 4, 4));
        app.refresh_transform_preview();
        let idle_key = app
            .transform_preview_key
            .expect("the first preview frame uploads a texture");
        assert!(!idle_key.dragging);

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.preview_dragging = true;
        }
        app.refresh_transform_preview();
        let drag_key = app
            .transform_preview_key
            .expect("the drag frame uploads a texture");
        assert!(drag_key.dragging, "the dirty key must track the drag state");
        assert_ne!(
            idle_key, drag_key,
            "the drag state alone must invalidate the preview key"
        );
    }

    /// W2 (d): transform-not-active leaves the draw/fieldier snapshot untouched
    /// — the algorithm selector is hidden and the tool properties are intact.
    #[test]
    fn transform_inactive_snapshot_keeps_draw_and_fieldier() {
        let mut app = App::default();
        {
            let session = app.projects.current_mut();
            session.tool_state.select_child(FieldierChild::Wand);
            session.draw_settings_mut(Tool::Fieldier).size = 7;
            session.transform_algorithm = TransformAlgorithm::CleanEdge;
        }
        app.write_dock_snapshots();
        let view = app.toolbox_host.view.borrow();
        assert!(
            !view.transform_active,
            "the selector is hidden with no transform"
        );
        assert_eq!(view.tool, Tool::Fieldier);
        assert_eq!(view.child, FieldierChild::Wand);
        assert_eq!(view.draw.size, 7, "the draw properties are unchanged");
    }

    #[test]
    fn wand_contiguous_effective_reflects_alt() {
        let mut app = App::default();
        assert!(
            app.projects.current().tool_state.wand().contiguous,
            "the wand starts contiguous"
        );
        app.write_dock_snapshots();
        assert!(
            app.toolbox_host.view.borrow().wand_contiguous_effective,
            "without Alt the effective contiguity follows the stored setting"
        );

        app.ctx.input_mut(|input| {
            input.modifiers = egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            };
        });
        app.write_dock_snapshots();
        assert!(
            !app.toolbox_host.view.borrow().wand_contiguous_effective,
            "holding Alt must momentarily drop contiguity"
        );
        assert!(
            app.projects.current().tool_state.wand().contiguous,
            "the Alt modifier is temporary and must not rewrite the stored setting"
        );
    }

    #[test]
    fn select_tool_change_keeps_selection() {
        let mut app = App::default();
        marquee(&mut app, (2, 2), (5, 5));
        assert!(app.projects.current_mut().selection.is_some());
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fill)]);
        assert!(
            app.projects.current_mut().selection.is_some(),
            "a deliberate tool switch keeps the selection (Esc cancels it)"
        );
    }

    #[test]
    fn app_tail_wiring_tapers_the_stroke() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .size = 16;
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .shape = BrushShape::Square;
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.apply_toolbar_events(vec![ToolbarEvent::BrushTailChanged(-10)]);
        assert_eq!(app.projects.current().draw_settings(Tool::Pencil).tail, -10);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((16, 64)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((116, 64)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((116, 64)),
            ..Default::default()
        });
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        let column_height = |x: usize| {
            (0..buf.height())
                .filter(|&y| buf.get_pixel(x, y) == Some(RED))
                .count()
        };
        let start = column_height(16);
        let end = column_height(116);
        assert_eq!(start, 16, "the stroke must start at the base size");
        // tail -10 shrinks one pixel per 10 px of travel: 100 px of travel
        // drops the 16 px brush to 6, whose visible end band is the union with
        // the size-7 stamp one step back (6 + 1).
        assert_eq!(
            end, 7,
            "tail -10 at base 16 must shrink to 6 px over 100 px of travel"
        );
        assert!(end < start, "the app-wired tail must thin the stroke");
    }

    // ------------------------------------------------------------------
    // U13: timeline + playback (R3 wave, feature F3)
    // ------------------------------------------------------------------

    /// The default App starts with one frame covering the whole canvas.
    #[test]
    fn timeline_ui_defaults() {
        let mut app = App::default();
        assert_eq!(app.projects.current().sequence.len(), 1);
        assert_eq!(app.projects.current().animation.frame_count(), 1);
        assert_eq!(app.projects.current().animation.current_index(), 0);
        assert!(app.projects.current_mut().sequence.looping());
        assert!(app.projects.current().animation.looping());
        let frame = app.projects.current().sequence.frame(0).unwrap();
        assert_eq!(frame.region().rect(), Rect2i::new(0, 0, 128, 128));
        assert_eq!(frame.delay_ms(), 100);
    }

    /// SelectFrame jumps the controller to the requested frame.
    #[test]
    fn select_frame_jumps_controller() {
        let mut app = App::default();
        app.apply_timeline_events(vec![TimelineEvent::AddFrame]);
        app.apply_timeline_events(vec![TimelineEvent::SelectFrame(1)]);
        assert_eq!(app.projects.current().animation.current_index(), 1);
    }

    /// SetDuration edits the frame's delay and mirrors it into the controller.
    #[test]
    fn set_duration_updates_frame_and_controller() {
        let mut app = App::default();
        app.apply_timeline_events(vec![TimelineEvent::SetDuration { index: 0, ms: 250 }]);
        assert_eq!(
            app.projects.current().sequence.frame(0).unwrap().delay_ms(),
            250
        );
        assert_eq!(app.projects.current().animation.delay_ms(0), 250);
    }

    /// Reorder moves the frame and keeps the controller in sync.
    #[test]
    fn reorder_moves_frames() {
        let mut app = App::default();
        app.apply_timeline_events(vec![TimelineEvent::AddFrame]);
        app.apply_timeline_events(vec![TimelineEvent::AddFrame]);
        app.apply_timeline_events(vec![TimelineEvent::SetDuration { index: 2, ms: 300 }]);
        app.apply_timeline_events(vec![TimelineEvent::Reorder { from: 2, to: 0 }]);
        assert_eq!(app.projects.current().sequence.len(), 3);
        assert_eq!(
            app.projects.current().sequence.frame(0).unwrap().delay_ms(),
            300
        );
        assert_eq!(app.projects.current().animation.frame_count(), 3);
    }

    /// AddFrame clones the current frame's region and selects the new frame.
    #[test]
    fn add_frame_clones_current_region() {
        let mut app = App::default();
        app.apply_timeline_events(vec![TimelineEvent::AddFrame]);
        assert_eq!(app.projects.current().sequence.len(), 2);
        assert_eq!(app.projects.current().animation.frame_count(), 2);
        assert_eq!(app.projects.current().animation.current_index(), 1);
        let f0 = app.projects.current().sequence.frame(0).unwrap();
        let f1 = app.projects.current().sequence.frame(1).unwrap();
        assert_eq!(f0.region().rect(), f1.region().rect());
    }

    /// RemoveFrame deletes the frame and keeps at least one frame.
    #[test]
    fn remove_frame_deletes_and_keeps_one() {
        let mut app = App::default();
        app.apply_timeline_events(vec![TimelineEvent::AddFrame]);
        app.apply_timeline_events(vec![TimelineEvent::RemoveFrame(0)]);
        assert_eq!(app.projects.current().sequence.len(), 1);
        assert_eq!(app.projects.current().animation.frame_count(), 1);
        // Removing the last remaining frame is a no-op.
        app.apply_timeline_events(vec![TimelineEvent::RemoveFrame(0)]);
        assert_eq!(app.projects.current().sequence.len(), 1);
    }

    /// ToggleLoop flips the loop flag on both the sequence and the controller.
    #[test]
    fn toggle_loop_flips_both_flags() {
        let mut app = App::default();
        assert!(app.projects.current_mut().sequence.looping());
        app.apply_timeline_events(vec![TimelineEvent::ToggleLoop]);
        assert!(!app.projects.current_mut().sequence.looping());
        assert!(!app.projects.current().animation.looping());
    }

    /// Play/Stop/Restart drive the controller transport.
    #[test]
    fn transport_controls_drive_controller() {
        let mut app = App::default();
        app.apply_timeline_events(vec![TimelineEvent::Play]);
        assert!(app.projects.current().animation.is_playing());
        app.apply_timeline_events(vec![TimelineEvent::Stop]);
        assert!(!app.projects.current().animation.is_playing());
        app.apply_timeline_events(vec![TimelineEvent::Restart]);
        assert_eq!(app.projects.current().animation.current_index(), 0);
    }

    /// tick_animation advances the controller by the frame delta.
    #[test]
    fn tick_advances_playback() {
        let mut app = App::default();
        app.apply_timeline_events(vec![TimelineEvent::AddFrame]);
        app.apply_timeline_events(vec![TimelineEvent::SelectFrame(0)]);
        app.apply_timeline_events(vec![TimelineEvent::Play]);
        app.tick_animation(100); // frame 0 (100 ms) -> frame 1
        assert_eq!(app.projects.current().animation.current_index(), 1);
        app.tick_animation(100); // frame 1 (100 ms) -> wraps to frame 0
        assert_eq!(app.projects.current().animation.current_index(), 0); // loops back
    }

    // -----------------------------------------------------------------------
    // Transform session (D32/D35/D36)
    // -----------------------------------------------------------------------

    /// Ctrl+LMB lift → translate drag → release commits the session as exactly
    /// one undo step, with the source rect cut and the pixels re-pasted at the
    /// drag offset.
    #[test]
    fn transform_session_commit_cuts_source_and_pastes_at_offset() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        assert!(app.projects.current_mut().transform.is_some());
        assert!(app.projects.current_mut().selection.is_none());

        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        let t = app
            .projects
            .current_mut()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        assert_eq!((t.drag, t.last_pt), (GizmoHit::Translate, (5.0, 5.0)));
        // Q1 problem 2: no dead zone — the first moved sample applies, and the
        // move stays absolute from the press, landing on Δ (+2, +1).
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((7, 6)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((7, 6)),
            ..Default::default()
        });
        // A release only ends the drag; the commit is the explicit step under
        // test (its own gesture is covered by the double-click / Esc tests).
        app.transform_session_commit();

        assert!(app.projects.current_mut().transform.is_none());
        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        // Source rect cut away.
        for y in 4..=5 {
            for x in 4..=5 {
                assert_eq!(
                    buf.get_pixel(x, y),
                    Some(Color::TRANSPARENT),
                    "source not cut at ({x},{y})"
                );
            }
        }
        // Pasted RED 2×2 at pos + (2, 1) → (6, 5).
        for y in 5..=6 {
            for x in 6..=7 {
                assert_eq!(
                    buf.get_pixel(x, y),
                    Some(RED),
                    "pixels missing at ({x},{y})"
                );
            }
        }
    }

    /// A mid-gesture cancel (Esc / tool switch / structural change) leaves the
    /// document byte-identical and the undo stack untouched.
    #[test]
    fn transform_session_cancel_leaves_document_untouched() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((7, 6)),
            ..Default::default()
        });

        app.clear_selection(); // tool-switch / new-selection reset path

        assert!(app.projects.current_mut().transform.is_none());
        assert_eq!(app.projects.current_mut().undo.undo_len(), 0);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        for y in 4..=5 {
            for x in 4..=5 {
                assert_eq!(buf.get_pixel(x, y), Some(RED), "source lost at ({x},{y})");
            }
        }
        assert_eq!(
            buf.get_pixel(7, 6),
            Some(Color::TRANSPARENT),
            "nothing pasted after cancel"
        );
    }

    /// Rotation drags spin around the pivot; scale drags resize relative to
    /// the OPPOSITE corner (fixed anchor) and recentre the pivot on the
    /// transformed pixels (issue #2). The scale drag runs on the unrotated
    /// object so its anchor is the unrotated opposite corner (4,4).
    #[test]
    fn transform_session_rotate_and_scale_around_pivot() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let pivot = app
            .projects
            .current_mut()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pivot();
        assert_eq!(pivot, (4.5, 4.5)); // source centre

        // Scale: ScaleSE drag from (5.5, 4.5) to (6, 5) about the OPPOSITE
        // corner (4,4) — free per-axis resize: ky = (5−4)/(4.5−4) = 2.0 and the
        // width ratio 4/3 snaps to the exact integer target 3 → sx = 1.5 (J3).
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((6, 5));
        let (sx, sy) = app
            .projects
            .current_mut()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .scale_xy();
        assert!(
            (sx - 1.5).abs() < 1e-3 && (sy - 2.0).abs() < 1e-3,
            "expected scale_xy ≈ (1.5, 2.0), got ({sx}, {sy})"
        );
        // Issue #2: after the opposite-corner scale the pivot is recentred to
        // the transformed bbox centre, (4,4)-(7,8) → (5.5, 6.0).
        assert_eq!(
            app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .object
                .pivot(),
            (5.5, 6.0),
            "the pivot is recentred to the bbox centre during an opposite-corner scale"
        );

        // Rotate around the RECENTRED pivot (5.5, 6.0): drag from (5.5, 7.0)
        // [+90°] to (4, 6) [+180° in y-down screen coords] → +90° delta.
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Rotate;
            t.last_pt = (5.5, 7.0);
        }
        app.transform_session_drag((4, 6));
        let angle = app
            .projects
            .current_mut()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .angle_deg;
        assert!(
            (angle - 90.0).abs() < 1e-3,
            "expected a clean +90° rotation around the recentred pivot, got {angle}"
        );
        // Rotating about the (already recentred) pivot leaves it in place.
        assert_eq!(
            app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .object
                .pivot(),
            (5.5, 6.0),
            "rotation keeps the recentred pivot fixed"
        );
    }

    /// A ScaleSE drag resizes about the OPPOSITE corner (min, min), which stays
    /// fixed in canvas space; the pivot never moves and the dragged corner
    /// grows outward. The resize is per-axis (free corner resize).
    #[test]
    fn transform_session_drag_corner_scale_fixes_opposite_corner() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let pivot_before = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pivot();
        assert_eq!(pivot_before, (4.5, 4.5), "pivot is the selection centre");

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((6, 5));

        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        let (min_x, min_y, max_x, max_y) = t.object.canvas_bbox();
        // The opposite corner (4,4) is fixed by the pos compensation.
        assert!(
            (min_x - 4.0).abs() < 1e-3,
            "opposite-corner min_x must stay fixed, got {min_x}"
        );
        assert!(
            (min_y - 4.0).abs() < 1e-3,
            "opposite-corner min_y must stay fixed, got {min_y}"
        );
        // Free per-axis resize: the width ratio 4/3 snaps to the exact integer
        // target 3 (2×4/3 = 2.67 → 3), so sx = 3/2 = 1.5; ky = 2.0 keeps
        // sy = 2.0. (J3 exact-target resize.)
        let (sx, sy) = t.object.scale_xy();
        assert!(
            (sx - 1.5).abs() < 1e-3 && (sy - 2.0).abs() < 1e-3,
            "expected per-axis scale_xy ≈ (1.5, 2.0), got ({sx}, {sy})"
        );
        // The dragged corner grew outward.
        assert!(max_x > 6.0 && max_y > 6.0, "the dragged corner must grow");
        // Issue #2: the pivot is recentred to the transformed bbox centre.
        assert_eq!(
            t.object.pivot(),
            ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5),
            "the pivot must be the transformed bbox centre after a corner scale"
        );
    }

    /// A ScaleSE drag is a PER-AXIS free resize: the width factor (4/3)
    /// differs from the height factor (2.0), the opposite corner stays fixed,
    /// and the pivot is recentred to the bbox centre.
    #[test]
    fn transform_session_drag_corner_scale_is_per_axis() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((6, 5));

        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        // The width ratio 4/3 snaps to the exact integer target 3 (sx = 1.5)
        // while ky = 2.0 keeps sy = 2.0 — the width and height resize
        // INDEPENDENTLY (the whole point of free corner resize). (J3.)
        let (sx, sy) = t.object.scale_xy();
        assert!(
            (sx - 1.5).abs() < 1e-3 && (sy - 2.0).abs() < 1e-3,
            "expected scale_xy ≈ (1.5, 2.0), got ({sx}, {sy})"
        );
        // The opposite corner (4,4) stays fixed.
        let (min_x, min_y, max_x, max_y) = t.object.canvas_bbox();
        assert!(
            (min_x - 4.0).abs() < 1e-3 && (min_y - 4.0).abs() < 1e-3,
            "the opposite corner must stay fixed, got ({min_x}, {min_y})"
        );
        assert_eq!(
            t.object.pivot(),
            ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5),
            "the pivot is recentred to the bbox centre"
        );
    }

    /// A ScaleTop drag resizes the Y axis only (real single-axis non-uniform
    /// resize): the bottom edge (opposite anchor) stays fixed, the top edge
    /// moves, and the width is untouched.
    #[test]
    fn transform_session_drag_edge_scale_top() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleTop;
            t.last_pt = (5.0, 4.0); // top edge midpoint
        }
        app.transform_session_drag((5, 3)); // pull the top edge up 1 px

        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        // Y scaled only (ky = (3−6)/(4−6) = 1.5); X untouched.
        assert_eq!(t.object.scale_xy(), (1.0, 1.5), "ky = (3−6)/(4−6) = 1.5");
        let (min_x, min_y, max_x, max_y) = t.object.canvas_bbox();
        assert!(
            (max_y - 6.0).abs() < 1e-3,
            "the bottom edge (opposite anchor) stays fixed, got max_y={max_y}"
        );
        assert!(min_y < 4.0, "the top edge moves up, got min_y={min_y}");
        // Issue #2: the pivot is recentred to the transformed bbox centre.
        assert_eq!(
            t.object.pivot(),
            ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5),
            "the pivot is the transformed bbox centre after an edge scale"
        );
    }

    /// A ScaleRight drag resizes the X axis only: the left edge (opposite
    /// anchor) stays fixed, the right edge moves right, and the HEIGHT is
    /// untouched — the whole point of non-uniform edge resize.
    #[test]
    fn transform_session_drag_edge_scale_right_only_width_changes() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleRight;
            t.last_pt = (6.0, 5.0); // right edge midpoint
        }
        app.transform_session_drag((7, 5)); // pull the right edge right 1 px

        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        // X scaled only (kx = (7−4)/(6−4) = 1.5); Y untouched.
        assert_eq!(t.object.scale_xy(), (1.5, 1.0), "kx = (7−4)/(6−4) = 1.5");
        let (min_x, min_y, max_x, max_y) = t.object.canvas_bbox();
        assert!(
            (min_x - 4.0).abs() < 1e-3,
            "the left edge (opposite anchor) stays fixed, got min_x={min_x}"
        );
        assert!(max_x > 6.0, "the right edge moves right, got max_x={max_x}");
        // The y-extent is completely unchanged by a horizontal edge resize.
        assert!(
            (min_y - 4.0).abs() < 1e-3 && (max_y - 6.0).abs() < 1e-3,
            "the height must be untouched, got y ∈ [{min_y}, {max_y}]"
        );
        assert_eq!(
            t.object.pivot(),
            ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5),
            "the pivot is the transformed bbox centre after an edge scale"
        );
    }

    /// N1 regression: an EDGE resize must leave the UNTRACKED axis completely
    /// alone even when the press was NOT exactly on the edge midpoint. Before
    /// the fix the untracked axis's min was recomputed from `start_pointer`
    /// vs the anchor and the object teleported along it toward the centre.
    #[test]
    fn edge_scale_with_off_midpoint_press_pins_the_untracked_axis() {
        // Vertical edges: only X may change; the Y bbox must stay EXACTLY put.
        for (hit, press, drag) in [
            (GizmoHit::ScaleRight, (6.0f32, 4.3f32), (7.0f32, 4.3f32)),
            (GizmoHit::ScaleLeft, (4.0, 4.3), (3.0, 4.3)),
        ] {
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
            app.lift_transform(Rect2i::new(4, 4, 2, 2));
            let (start_min_x, start_min_y, start_max_x, start_max_y) = selection_bbox(&app);
            {
                let t = app
                    .projects
                    .current_mut()
                    .transform
                    .as_mut()
                    .unwrap()
                    .expect_selection_mut();
                t.drag = hit;
                t.last_pt = press;
            }
            app.transform_session_drag((drag.0.round() as i32, drag.1.round() as i32));

            let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
            assert!(
                (min_y - start_min_y).abs() < 1e-3 && (max_y - start_max_y).abs() < 1e-3,
                "{hit:?}: the vertical (untracked) axis must stay fixed, got y ∈ [{min_y}, {max_y}] \
                 (start [{start_min_y}, {start_max_y}])"
            );
            assert!(
                (min_x - start_min_x).abs() > 1e-3 || (max_x - start_max_x).abs() > 1e-3,
                "{hit:?}: the horizontal (tracked) axis must resize"
            );
        }

        // Horizontal edges: only Y may change; the X bbox must stay EXACTLY put.
        for (hit, press, drag) in [
            (GizmoHit::ScaleTop, (4.3f32, 4.0f32), (4.3f32, 3.0f32)),
            (GizmoHit::ScaleBottom, (4.3, 6.0), (4.3, 7.0)),
        ] {
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
            app.lift_transform(Rect2i::new(4, 4, 2, 2));
            let (start_min_x, start_min_y, start_max_x, start_max_y) = selection_bbox(&app);
            {
                let t = app
                    .projects
                    .current_mut()
                    .transform
                    .as_mut()
                    .unwrap()
                    .expect_selection_mut();
                t.drag = hit;
                t.last_pt = press;
            }
            app.transform_session_drag((drag.0.round() as i32, drag.1.round() as i32));

            let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
            assert!(
                (min_x - start_min_x).abs() < 1e-3 && (max_x - start_max_x).abs() < 1e-3,
                "{hit:?}: the horizontal (untracked) axis must stay fixed, got x ∈ [{min_x}, {max_x}] \
                 (start [{start_min_x}, {start_max_x}])"
            );
            assert!(
                (min_y - start_min_y).abs() > 1e-3 || (max_y - start_max_y).abs() > 1e-3,
                "{hit:?}: the vertical (tracked) axis must resize"
            );
        }
    }

    /// N1: the exact old-failure shape — a `ScaleRight` press 1 px ABOVE the Y
    /// midpoint used to place the untracked Y min at the anchor (centre) line,
    /// teleporting the box up. The Y extent must stay at the start.
    #[test]
    fn scale_right_off_midpoint_does_not_teleport_y() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
        app.lift_transform(Rect2i::new(4, 4, 4, 4)); // bbox (4,4)-(8,8): y centre = 6
        let (_, start_min_y, _, start_max_y) = selection_bbox(&app);
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleRight;
            t.last_pt = (8.0, 5.0); // NOT the y midpoint (6.0)
        }
        app.transform_session_drag((10, 5));

        let (_, min_y, _, max_y) = selection_bbox(&app);
        assert!(
            (min_y - start_min_y).abs() < 1e-3 && (max_y - start_max_y).abs() < 1e-3,
            "an off-midpoint ScaleRight must not change the Y extent, got [{min_y}, {max_y}] \
             (start [{start_min_y}, {start_max_y}])"
        );
    }

    /// N1: corner resizes still track BOTH axes (the tracked-axes parameter is
    /// `(true, true)`), so the dragged corner follows the cursor on both and
    /// the opposite corner stays fixed.
    #[test]
    fn corner_resize_still_tracks_both_axes() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
        app.lift_transform(Rect2i::new(4, 4, 4, 4)); // bbox (4,4)-(8,8)
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (8.0, 8.0);
        }
        app.transform_session_drag((10, 12));

        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - 4.0).abs() < 1e-3 && (min_y - 4.0).abs() < 1e-3,
            "the opposite corner stays fixed, got ({min_x},{min_y})"
        );
        assert!(
            (max_x - 10.0).abs() < 1e-3 && (max_y - 12.0).abs() < 1e-3,
            "both axes must track the cursor, got ({max_x},{max_y})"
        );
    }

    // -- T2-A: edge flip only on the tracked axis ----------------------------

    /// Drive an EDGE drag whose pointer vectors are expressed in the object's
    /// LOCAL frame (`v0` = press, `v` = current) at an arbitrary object angle.
    /// The canvas anchor is derived exactly as the session captures it, so the
    /// test exercises the real start-state capture and the local ratio
    /// projection (the source of the T2-A wrong-axis flip bug).
    fn edge_drag_local(
        app: &mut App,
        hit: GizmoHit,
        angle_deg: f32,
        v0_local: (f32, f32),
        v_local: (f32, f32),
    ) {
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_angle(angle_deg);
        }
        let anchor = {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            let (w, h) = t.object.source_dims();
            let (w, h) = (w as f32, h as f32);
            let (a_src, _, _) = anchor_src_for(hit, w, h, false);
            canvas_anchor(t.object.canvas_corners(), w, h, a_src)
        };
        // R(angle)·v = rotate_inv(v, −angle) (rotate_inv applies Rᵀ).
        let sp = rotate_inv(v0_local, -angle_deg);
        let cv = rotate_inv(v_local, -angle_deg);
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = hit;
            t.last_pt = (anchor.0 + sp.0, anchor.1 + sp.1);
        }
        app.transform_session_drag((
            (anchor.0 + cv.0).round() as i32,
            (anchor.1 + cv.1).round() as i32,
        ));
    }

    /// T2-A regression: an edge drag PERPENDICULAR to its tracked axis must
    /// NOT toggle the mirror on the axis it does not resize — at angle 0 AND
    /// rotated (θ ≠ 0). Before the fix the untracked axis's local ratio was
    /// fed to the flip XOR unconditionally, so crossing the opposite-edge
    /// midpoint anchor (a perpendicular drag) flipped the WRONG axis.
    #[test]
    fn edge_resize_flip_only_toggles_the_tracked_axis() {
        for angle in [0.0f32, 90.0] {
            // ScaleRight (tracked X): press off the Y midpoint and drag
            // perpendicular past it. The untracked Y ratio goes negative, but
            // flip_v must stay false.
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
            app.lift_transform(Rect2i::new(4, 4, 4, 4));
            edge_drag_local(
                &mut app,
                GizmoHit::ScaleRight,
                angle,
                (4.0, -0.7),
                (4.0, 2.0),
            );
            assert_eq!(
                selection_flips(&app),
                (false, false),
                "angle {angle}: perpendicular ScaleRight must not flip either axis"
            );

            // ScaleTop (tracked Y): press off the X midpoint and drag
            // perpendicular past it. The untracked X ratio goes negative, but
            // flip_h must stay false.
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
            app.lift_transform(Rect2i::new(4, 4, 4, 4));
            edge_drag_local(
                &mut app,
                GizmoHit::ScaleTop,
                angle,
                (0.7, -4.0),
                (-2.0, -4.0),
            );
            assert_eq!(
                selection_flips(&app),
                (false, false),
                "angle {angle}: perpendicular ScaleTop must not flip either axis"
            );

            // ScaleRight PAST the anchor along the TRACKED axis DOES flip H.
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
            app.lift_transform(Rect2i::new(4, 4, 4, 4));
            edge_drag_local(
                &mut app,
                GizmoHit::ScaleRight,
                angle,
                (4.0, -0.7),
                (-1.0, -0.7),
            );
            assert_eq!(
                selection_flips(&app),
                (true, false),
                "angle {angle}: crossing the anchor on the tracked X axis must flip H only"
            );

            // ScaleTop PAST the anchor along the TRACKED axis DOES flip V.
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
            app.lift_transform(Rect2i::new(4, 4, 4, 4));
            edge_drag_local(&mut app, GizmoHit::ScaleTop, angle, (0.7, -4.0), (0.7, 1.0));
            assert_eq!(
                selection_flips(&app),
                (false, true),
                "angle {angle}: crossing the anchor on the tracked Y axis must flip V only"
            );
        }
    }

    // -- T2-B: keyboard movement of the transform ----------------------------

    /// T2-B: arrow keys and WASD translate the floating transform by exactly
    /// 1 px per press, integer-snapped, moving `pos` AND the pivot together.
    #[test]
    fn transform_arrow_and_wasd_keys_move_one_pixel() {
        for (key, dx, dy) in [
            (egui::Key::ArrowUp, 0.0, -1.0),
            (egui::Key::ArrowDown, 0.0, 1.0),
            (egui::Key::ArrowLeft, -1.0, 0.0),
            (egui::Key::ArrowRight, 1.0, 0.0),
            (egui::Key::W, 0.0, -1.0),
            (egui::Key::S, 0.0, 1.0),
            (egui::Key::A, -1.0, 0.0),
            (egui::Key::D, 1.0, 0.0),
        ] {
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
            app.lift_transform(Rect2i::new(4, 4, 2, 2));
            let (px, py) = selection_pos(&app);
            let (cx, cy) = selection_pivot(&app);
            send_key(&mut app, key, egui::Modifiers::NONE);
            let (nx, ny) = selection_pos(&app);
            assert_eq!((nx, ny), (px + dx, py + dy), "{key:?}: object pos");
            assert_eq!(
                selection_pivot(&app),
                (cx + dx, cy + dy),
                "{key:?}: pivot must move with pos"
            );
            assert_eq!(
                (nx.fract(), ny.fract()),
                (0.0, 0.0),
                "{key:?}: the move must stay integer-snapped"
            );
        }
    }

    /// T2-B: Shift (or Alt) uses the larger grid step while a transform is
    /// live. The step is the documented fallback of 10 px.
    #[test]
    fn transform_shift_or_alt_arrow_uses_the_larger_step() {
        for (label, mods) in [
            (
                "shift",
                egui::Modifiers {
                    shift: true,
                    ..egui::Modifiers::NONE
                },
            ),
            (
                "alt",
                egui::Modifiers {
                    alt: true,
                    ..egui::Modifiers::NONE
                },
            ),
        ] {
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
            app.lift_transform(Rect2i::new(4, 4, 2, 2));
            let (px, py) = selection_pos(&app);
            let (cx, cy) = selection_pivot(&app);
            run_shortcuts(
                &mut app,
                vec![
                    modifiers_changed(mods),
                    egui::Event::Key {
                        key: egui::Key::ArrowRight,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: mods,
                    },
                ],
            );
            assert_eq!(
                selection_pos(&app),
                (px + 10.0, py),
                "{label}: the larger step is 10 px"
            );
            assert_eq!(
                selection_pivot(&app),
                (cx + 10.0, cy),
                "{label}: pivot must follow the same step"
            );
        }
    }

    /// T2-B: while a transform is live, W/A/S/D move the object and never
    /// reach the tool/child keybindings below (W→wand, S→rectangle, D→pen).
    #[test]
    fn wasd_move_the_transform_without_switching_tools() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let tool0 = app.projects.current().tool_state.tool();
        let child0 = app.projects.current().tool_state.child();
        let (px, py) = selection_pos(&app);

        send_key(&mut app, egui::Key::W, egui::Modifiers::NONE);
        assert_eq!(selection_pos(&app), (px, py - 1.0), "W moves up 1 px");
        assert_eq!(
            app.projects.current().tool_state.child(),
            child0,
            "W must not pick the wand"
        );

        send_key(&mut app, egui::Key::S, egui::Modifiers::NONE);
        assert_eq!(selection_pos(&app), (px, py), "S moves down 1 px");
        assert_eq!(
            app.projects.current().tool_state.child(),
            child0,
            "S must not pick the rectangle"
        );

        send_key(&mut app, egui::Key::D, egui::Modifiers::NONE);
        assert_eq!(selection_pos(&app), (px + 1.0, py), "D moves right 1 px");

        send_key(&mut app, egui::Key::A, egui::Modifiers::NONE);
        assert_eq!(selection_pos(&app), (px, py), "A moves left 1 px");

        assert_eq!(
            app.projects.current().tool_state.tool(),
            tool0,
            "W/A/S/D must not switch the active tool during a transform"
        );
        assert_eq!(
            app.projects.current().tool_state.child(),
            child0,
            "W/S/D must not switch the Fieldier child during a transform"
        );
        assert!(
            app.projects.current().transform.is_some(),
            "the transform session stays live"
        );
    }

    /// T2-B: Enter still commits and Esc still cancels a transform that was
    /// moved by the keyboard (unchanged from the pointer-move contract).
    #[test]
    fn arrow_moved_transform_enter_commits_escape_cancels() {
        let mut app = lifted_red_transform();
        send_key(&mut app, egui::Key::ArrowRight, egui::Modifiers::NONE);
        send_key(&mut app, egui::Key::ArrowRight, egui::Modifiers::NONE);
        assert_eq!(selection_pos(&app), (6.0, 4.0));
        send_key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
        assert!(
            app.projects.current().transform.is_none(),
            "Enter must commit the keyboard-moved transform"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 1);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(6, 4),
            Some(RED),
            "the moved pixels are committed at +2 px"
        );

        let mut app = lifted_red_transform();
        send_key(&mut app, egui::Key::ArrowRight, egui::Modifiers::NONE);
        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(
            app.projects.current().transform.is_none(),
            "Esc must cancel the keyboard-moved transform"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(4, 4),
            Some(RED),
            "Esc restores the source pixels"
        );
    }

    // -- P1: object-local (scale-then-rotate) resize -------------------------

    /// Midpoint of the quad edge from corner `i` to corner `i+1`.
    fn quad_edge_mid(corners: [(f32, f32); 4], i: usize) -> (f32, f32) {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
    }

    fn selection_corners(app: &App) -> [(f32, f32); 4] {
        app.projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .canvas_corners()
    }

    fn selection_local_dims(app: &App) -> (usize, usize) {
        app.projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .local_scaled_dims()
    }

    /// A quad is a rectangle iff consecutive edges are perpendicular and
    /// opposite edges are parallel (no shear).
    fn assert_rectangular(corners: [(f32, f32); 4], ctx: &str) {
        let e0 = (corners[1].0 - corners[0].0, corners[1].1 - corners[0].1);
        let e1 = (corners[2].0 - corners[1].0, corners[2].1 - corners[1].1);
        let e2 = (corners[3].0 - corners[2].0, corners[3].1 - corners[2].1);
        let dot = e0.0 * e1.0 + e0.1 * e1.1;
        let norm = e0.0.hypot(e0.1).max(1e-6) * e1.0.hypot(e1.1).max(1e-6);
        assert!(
            dot.abs() / norm < 1e-4,
            "{ctx}: not rectangular (dot={dot})"
        );
        assert!(
            (e0.0 + e2.0).abs() < 1e-3 && (e0.1 + e2.1).abs() < 1e-3,
            "{ctx}: opposite edges not parallel"
        );
    }

    /// P1: a rotated EDGE resize resizes along the object's LOCAL normal,
    /// keeping the opposite local edge fixed and the quad a rectangle (no
    /// shear). Covers ScaleTop (local y) and ScaleRight (local x).
    #[test]
    fn edge_resize_follows_local_normal_when_rotated() {
        for angle in [30.0f32, 45.0, 90.0] {
            for (hit, dragged_edge, anchor_edge, tracked_x) in [
                (GizmoHit::ScaleTop, 0usize, 2usize, false),
                (GizmoHit::ScaleRight, 1usize, 3usize, true),
            ] {
                let mut app = App::default();
                seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
                app.lift_transform(Rect2i::new(4, 4, 8, 8));
                {
                    let t = app
                        .projects
                        .current_mut()
                        .transform
                        .as_mut()
                        .unwrap()
                        .expect_selection_mut();
                    t.object.set_angle(angle);
                }
                let before = selection_corners(&app);
                let (lw0, lh0) = selection_local_dims(&app);
                let press = quad_edge_mid(before, dragged_edge);
                // Local outward normal: +x for the right edge, -y for the top.
                let n_local = if tracked_x { (1.0, 0.0) } else { (0.0, -1.0) };
                let th = angle.to_radians();
                let (c, s) = (th.cos(), th.sin());
                let n_canvas = (c * n_local.0 - s * n_local.1, s * n_local.0 + c * n_local.1);
                let cur = (press.0 + n_canvas.0 * 5.0, press.1 + n_canvas.1 * 5.0);
                {
                    let t = app
                        .projects
                        .current_mut()
                        .transform
                        .as_mut()
                        .unwrap()
                        .expect_selection_mut();
                    t.drag = hit;
                    t.last_pt = press;
                }
                app.transform_session_drag((cur.0.round() as i32, cur.1.round() as i32));

                let after = selection_corners(&app);
                let (lw1, lh1) = selection_local_dims(&app);

                // The OPPOSITE local edge midpoint is fixed.
                let a0 = quad_edge_mid(before, anchor_edge);
                let a1 = quad_edge_mid(after, anchor_edge);
                assert!(
                    (a0.0 - a1.0).abs() < 1e-2 && (a0.1 - a1.1).abs() < 1e-2,
                    "angle={angle} {hit:?}: opposite edge midpoint drifted {a0:?} -> {a1:?}"
                );
                // Only the tracked LOCAL axis changed.
                if tracked_x {
                    assert_eq!(lh1, lh0, "angle={angle} {hit:?}: local height changed");
                    assert_ne!(
                        lw1, lw0,
                        "angle={angle} {hit:?}: local width did not change"
                    );
                } else {
                    assert_eq!(lw1, lw0, "angle={angle} {hit:?}: local width changed");
                    assert_ne!(
                        lh1, lh0,
                        "angle={angle} {hit:?}: local height did not change"
                    );
                }
                // The quad stays rectangular (Option A: no shear).
                assert_rectangular(after, &format!("angle={angle} {hit:?}"));
                // The dragged edge midpoint moved parallel to R(θ)·n.
                let d0 = quad_edge_mid(before, dragged_edge);
                let d1 = quad_edge_mid(after, dragged_edge);
                let disp = (d1.0 - d0.0, d1.1 - d0.1);
                let cross = disp.0 * n_canvas.1 - disp.1 * n_canvas.0;
                assert!(
                    cross.abs() < 1e-2,
                    "angle={angle} {hit:?}: dragged edge moved off the local normal (cross={cross})"
                );
            }
        }
    }

    /// P1: at angle 0 an edge resize is unchanged — only the tracked bbox axis
    /// changes and the opposite edge stays put.
    #[test]
    fn edge_resize_angle0_unchanged() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleRight;
            t.last_pt = (12.0, 8.0); // right edge midpoint
        }
        app.transform_session_drag((16, 8));
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!((min_x - 4.0).abs() < 1e-3, "left edge fixed, got {min_x}");
        assert!(
            (min_y - 4.0).abs() < 1e-3 && (max_y - 12.0).abs() < 1e-3,
            "height must be unchanged, got y ∈ [{min_y}, {max_y}]"
        );
        assert!(max_x > 12.0, "right edge must move, got {max_x}");
        let (lw, lh) = selection_local_dims(&app);
        assert_eq!(lh, 8, "local height unchanged at angle 0");
        assert!(lw > 8, "local width grew");
    }

    /// P1: an OFF-MIDPOINT rotated edge press must not change the untracked
    /// local axis and must not shear the quad.
    #[test]
    fn edge_untracked_axis_pinned_when_rotated() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        let angle = 30.0f32;
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_angle(angle);
        }
        let before = selection_corners(&app);
        let (_, lh0) = selection_local_dims(&app);
        // Press on the right edge but OFF its midpoint (toward TR).
        let a = before[1];
        let b = before[2];
        let press = (a.0 + (b.0 - a.0) * 0.3, a.1 + (b.1 - a.1) * 0.3);
        let th = angle.to_radians();
        let (c, s) = (th.cos(), th.sin());
        let n_canvas = (c, s); // local +x rotated
        let cur = (press.0 + n_canvas.0 * 5.0, press.1 + n_canvas.1 * 5.0);
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleRight;
            t.last_pt = press;
        }
        app.transform_session_drag((cur.0.round() as i32, cur.1.round() as i32));
        let (_, lh1) = selection_local_dims(&app);
        assert_eq!(
            lh1, lh0,
            "off-midpoint edge press must not change the untracked local axis"
        );
        assert_rectangular(selection_corners(&app), "off-midpoint rotated edge");
    }

    /// P1: a rotated CORNER resize also stays a rectangle (both local axes
    /// tracked, no shear).
    #[test]
    fn corner_resize_rotated_stays_rectangular() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        let angle = 30.0f32;
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_angle(angle);
        }
        let before = selection_corners(&app);
        let (lw0, lh0) = selection_local_dims(&app);
        let press = before[2]; // local BR (SE handle)
        let th = angle.to_radians();
        let (c, s) = (th.cos(), th.sin());
        // Outward local diagonal (1,1) rotated into canvas space.
        let d_canvas = (c * 1.0 - s * 1.0, s * 1.0 + c * 1.0);
        let cur = (press.0 + d_canvas.0 * 5.0, press.1 + d_canvas.1 * 5.0);
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = press;
        }
        app.transform_session_drag((cur.0.round() as i32, cur.1.round() as i32));
        let (lw1, lh1) = selection_local_dims(&app);
        assert!(
            lw1 > lw0 && lh1 > lh0,
            "both local axes must grow, got ({lw0},{lh0}) -> ({lw1},{lh1})"
        );
        assert_rectangular(selection_corners(&app), "rotated corner resize");
    }

    /// Shift during a Rotate drag snaps the resulting absolute angle to the
    /// nearest 45° multiple (the [`ROTATE_SNAP_DEG`] table holds ONLY 45°
    /// multiples — the old atan(0.5) points are gone).
    #[test]
    fn rotate_shift_snaps_to_45_degree_multiples() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Rotate;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((4, 3));
        set_modifiers(&app, egui::Modifiers::NONE);

        let angle = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .angle_deg;
        assert!(
            ROTATE_SNAP_DEG.contains(&angle),
            "snapped angle {angle} must be one of {ROTATE_SNAP_DEG:?}"
        );
        assert_eq!(
            angle.rem_euclid(45.0),
            0.0,
            "the snapped angle must be a 45° multiple, got {angle}"
        );
    }

    /// Shift on a CORNER scale locks the aspect ratio: both axes scale by the
    /// dominant-axis ratio (a free drag here would be 2.0 × 1.0).
    #[test]
    fn shift_corner_scale_locks_aspect() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (6.0, 6.0);
        }
        app.transform_session_drag((8, 6));
        set_modifiers(&app, egui::Modifiers::NONE);

        let (sx, sy) = selection_scale(&app);
        assert!(
            (sx - sy).abs() < 1e-3,
            "Shift must lock the aspect ratio, got ({sx}, {sy})"
        );
        assert!(
            (sx - 2.0).abs() < 1e-3,
            "the dominant-axis ratio is 2.0, got ({sx}, {sy})"
        );
    }

    /// Shift on an EDGE scale is IGNORED: the edge still resizes about the
    /// opposite edge midpoint (single-axis), exactly like a plain drag.
    #[test]
    fn shift_edge_scale_is_ignored() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleRight;
            t.last_pt = (6.0, 5.0);
        }
        app.transform_session_drag((8, 5));
        set_modifiers(&app, egui::Modifiers::NONE);

        assert_eq!(
            selection_scale(&app),
            (2.0, 1.0),
            "a Shift edge drag behaves like a plain single-axis resize"
        );
        let (min_x, _, _, _) = selection_bbox(&app);
        assert!(
            (min_x - 4.0).abs() < 1e-3,
            "the opposite (left) edge stays fixed, got min_x={min_x}"
        );
    }

    /// Alt on a CORNER scale anchors at the bbox CENTRE: both opposite corners
    /// move outward, the centre stays put, and the pivot is recentred there.
    #[test]
    fn alt_corner_scale_anchors_at_the_centre() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (6.0, 6.0);
        }
        app.transform_session_drag((8, 8));
        set_modifiers(&app, egui::Modifiers::NONE);

        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            min_x < 4.0 && max_x > 6.0 && min_y < 4.0 && max_y > 6.0,
            "both opposite corners move outward, got ({min_x},{min_y})-({max_x},{max_y})"
        );
        assert!(
            ((min_x + max_x) * 0.5 - 5.0).abs() < 1e-3
                && ((min_y + max_y) * 0.5 - 5.0).abs() < 1e-3,
            "the bbox centre stays at (5,5), got ({min_x},{min_y})-({max_x},{max_y})"
        );
        assert_eq!(
            selection_pivot(&app),
            (5.0, 5.0),
            "Alt recentres the pivot to the centre anchor"
        );
    }

    /// Alt+Shift on a CORNER scale is a UNIFORM centre scale: both axes get the
    /// same (dominant-axis) factor, unlike the free Alt scale.
    #[test]
    fn alt_shift_corner_scale_is_uniform_about_the_centre() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 10, 10));
        app.lift_transform(Rect2i::new(0, 0, 10, 10));
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (12.0, 8.0);
        }
        app.transform_session_drag((16, 10));
        set_modifiers(&app, egui::Modifiers::NONE);

        let (sx, sy) = selection_scale(&app);
        assert!(
            (sx - sy).abs() < 1e-3,
            "Alt+Shift must lock the aspect ratio (uniform), got ({sx}, {sy})"
        );
        assert!(sx > 1.0, "the scale must grow, got ({sx}, {sy})");
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            ((min_x + max_x) * 0.5 - 5.0).abs() < 1e-3
                && ((min_y + max_y) * 0.5 - 5.0).abs() < 1e-3,
            "the centre anchor stays at (5,5), got ({min_x},{min_y})-({max_x},{max_y})"
        );
    }

    /// Alt on an EDGE scale anchors the relevant axis at the bbox centre: both
    /// opposite edges move outward while the untouched axis stays put.
    #[test]
    fn alt_edge_scale_anchors_at_the_centre() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        set_modifiers(
            &app,
            egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleRight;
            t.last_pt = (6.0, 5.0);
        }
        app.transform_session_drag((8, 5));
        set_modifiers(&app, egui::Modifiers::NONE);

        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            min_x < 4.0 && max_x > 6.0,
            "both horizontal edges move outward, got ({min_x}, {max_x})"
        );
        assert!(
            (min_y - 4.0).abs() < 1e-3 && (max_y - 6.0).abs() < 1e-3,
            "the vertical axis is untouched, got ({min_y}, {max_y})"
        );
        assert_eq!(selection_pivot(&app), (5.0, 5.0), "Alt centres the pivot");
    }

    /// Regression (L1-B): after ANY Alt (centre-anchored) scale the pivot must
    /// sit at the gizmo/bbox centre, and the bbox centre must stay where the
    /// centre anchor was (a centre-anchored resize is centre-invariant).
    ///
    /// Exercised across rotations (including the exact-90° integer-coefficient
    /// placement path and a generic f64-trig angle), for a corner and both edge
    /// handles, with the DEFAULT (off-centre) pivot so the recentre's pos
    /// correction is genuinely stressed.
    #[test]
    fn alt_scale_centres_the_pivot_when_rotated() {
        for angle in [0.0_f32, 30.0, 45.0, 90.0, 135.0] {
            for hit in [GizmoHit::ScaleSE, GizmoHit::ScaleRight, GizmoHit::ScaleTop] {
                let mut app = App::default();
                seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
                app.lift_transform(Rect2i::new(4, 4, 4, 4));
                {
                    let t = app
                        .projects
                        .current_mut()
                        .transform
                        .as_mut()
                        .unwrap()
                        .expect_selection_mut();
                    t.object.set_angle(angle);
                    // Deliberately leave the DEFAULT (off-centre) pivot so the
                    // recentre's pos correction is genuinely exercised.
                    let _ = t.object.canvas_bbox();
                }
                let (mnx, mny, mxx, mxy) = selection_bbox(&app);
                let start_center = ((mnx + mxx) * 0.5, (mny + mxy) * 0.5);
                set_modifiers(
                    &app,
                    egui::Modifiers {
                        alt: true,
                        ..egui::Modifiers::NONE
                    },
                );
                let start_pointer = match hit {
                    GizmoHit::ScaleSE => (mxx, mxy),
                    GizmoHit::ScaleRight => (mxx, (mny + mxy) * 0.5),
                    GizmoHit::ScaleTop => ((mnx + mxx) * 0.5, mny),
                    _ => unreachable!(),
                };
                let cur = match hit {
                    GizmoHit::ScaleSE => (mxx + 4.0, mxy + 4.0),
                    GizmoHit::ScaleRight => (mxx + 4.0, (mny + mxy) * 0.5),
                    GizmoHit::ScaleTop => ((mnx + mxx) * 0.5, mny - 4.0),
                    _ => unreachable!(),
                };
                {
                    let t = app
                        .projects
                        .current_mut()
                        .transform
                        .as_mut()
                        .unwrap()
                        .expect_selection_mut();
                    t.drag = hit;
                    t.last_pt = start_pointer;
                }
                app.transform_session_drag((cur.0.round() as i32, cur.1.round() as i32));
                set_modifiers(&app, egui::Modifiers::NONE);

                let (a, b, c, d) = selection_bbox(&app);
                let pivot = selection_pivot(&app);
                let center = ((a + c) * 0.5, (b + d) * 0.5);
                assert!(
                    (pivot.0 - center.0).abs() < 1e-2 && (pivot.1 - center.1).abs() < 1e-2,
                    "angle={angle} hit={hit:?}: Alt must put the pivot on the bbox centre, \
                     got pivot {pivot:?} vs centre {center:?}"
                );
                assert!(
                    (center.0 - start_center.0).abs() < 1e-2
                        && (center.1 - start_center.1).abs() < 1e-2,
                    "angle={angle} hit={hit:?}: a centre-anchored resize must leave the \
                     centre invariant, got {start_center:?} -> {center:?}"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // K2: absolute start-referenced resize + FLIP on crossing + drag lock
    // -----------------------------------------------------------------------

    /// The dragged corner tracks the cursor via the ABSOLUTE formula
    /// `target = round(start_local_dim * |ratio|)` projected into the local
    /// frame about the fixed anchor: pulling out then back lands on the same
    /// absolute target with no compounded float/`round` drift, and the opposite
    /// corner stays fixed.
    #[test]
    fn corner_resize_tracks_the_cursor_absolutely_without_drift() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 3, 3));
        app.lift_transform(Rect2i::new(0, 0, 3, 3));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (4.0, 4.0); // press just outside the 3x3 SE corner
        }
        // Frame 1: |ratio| = 5/4 = 1.25 → round(3 × 1.25) = 4.
        app.transform_session_drag((5, 5));
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - 0.0).abs() < 1e-3 && (min_y - 0.0).abs() < 1e-3,
            "the opposite anchor (0,0) must stay fixed, got ({min_x},{min_y})"
        );
        assert!(
            (max_x - 4.0).abs() < 1e-3 && (max_y - 4.0).abs() < 1e-3,
            "frame 1 must land on the absolute target 4, got ({max_x},{max_y})"
        );
        // Frame 2: |ratio| = 7/4 = 1.75 → round(3 × 1.75) = 5. The old
        // INCREMENTAL code (current 4 × 7/5) would have produced 6.
        app.transform_session_drag((7, 7));
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - 0.0).abs() < 1e-3 && (min_y - 0.0).abs() < 1e-3,
            "the anchor must stay fixed across frames, got ({min_x},{min_y})"
        );
        assert!(
            (max_x - 5.0).abs() < 1e-3 && (max_y - 5.0).abs() < 1e-3,
            "frame 2 must use the absolute target 5 (no drift), got ({max_x},{max_y})"
        );
        // Frame 3: pull back to (5,5) → back to the absolute target 4.
        app.transform_session_drag((5, 5));
        let (_, _, max_x, max_y) = selection_bbox(&app);
        assert!(
            (max_x - 4.0).abs() < 1e-3 && (max_y - 4.0).abs() < 1e-3,
            "pulling back must return to the absolute target 4, got ({max_x},{max_y})"
        );
    }

    /// Crossing the opposite corner on an axis FLIPS that axis independently:
    /// the size stays positive, the anchor stays pinned, and the object extends
    /// to the other side instead of vanishing or locking at 1 px.
    #[test]
    fn corner_resize_flips_when_crossing_the_opposite_corner() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.lift_transform(Rect2i::new(0, 0, 4, 4));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (4.0, 4.0);
        }
        // Cursor (-3, 6): X crosses the anchor (-0.75 → flip_h, width 3); Y
        // stays positive (1.5 → height 6).
        app.transform_session_drag((-3, 6));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert!(t.object.flip_h && !t.object.flip_v, "only X must flip");
            assert_eq!(t.object.rotated_dims(), (3, 6));
        }
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - (-3.0)).abs() < 1e-3 && (max_x - 0.0).abs() < 1e-3,
            "the box must mirror to the LEFT of the anchor, got x [{min_x},{max_x}]"
        );
        assert!(
            (min_y - 0.0).abs() < 1e-3 && (max_y - 6.0).abs() < 1e-3,
            "Y must simply grow, got y [{min_y},{max_y}]"
        );
        // The anchor (0,0) is now the TOP-RIGHT corner and is exactly fixed.
        assert!(
            (max_x - 0.0).abs() < 1e-3 && (min_y - 0.0).abs() < 1e-3,
            "the anchor (0,0) must stay fixed across the flip"
        );

        // Both axes crossed (-3, -4): flip_h AND flip_v, positive sizes.
        app.transform_session_drag((-3, -4));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert!(t.object.flip_h && t.object.flip_v, "both axes must flip");
            assert_eq!(t.object.rotated_dims(), (3, 4));
        }
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - (-3.0)).abs() < 1e-3
                && (min_y - (-4.0)).abs() < 1e-3
                && (max_x - 0.0).abs() < 1e-3
                && (max_y - 0.0).abs() < 1e-3,
            "both axes must mirror about the fixed anchor, got ({min_x},{min_y})-({max_x},{max_y})"
        );
    }

    /// Pulling a dragged edge past its opposite edge flips that axis only; the
    /// other axis and the anchor midpoint are untouched.
    #[test]
    fn edge_resize_flips_when_pulled_past_the_opposite_edge() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.lift_transform(Rect2i::new(0, 0, 4, 4));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleRight;
            t.last_pt = (4.0, 2.0); // right-edge midpoint
        }
        // Cursor (-3, 2): the dragged right edge crosses the left anchor.
        app.transform_session_drag((-3, 2));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert!(t.object.flip_h && !t.object.flip_v, "only the X axis flips");
        }
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - (-3.0)).abs() < 1e-3 && (max_x - 0.0).abs() < 1e-3,
            "the edge must mirror to the left of the anchor, got x [{min_x},{max_x}]"
        );
        assert!(
            (min_y - 0.0).abs() < 1e-3 && (max_y - 4.0).abs() < 1e-3,
            "the vertical axis must be untouched, got y [{min_y},{max_y}]"
        );
    }

    /// The anchor is captured once at drag start and never moves when a flip
    /// toggles on, then off again.
    #[test]
    fn resize_anchor_stays_fixed_across_a_flip() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.lift_transform(Rect2i::new(0, 0, 4, 4));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (4.0, 4.0);
        }
        // No flip: (6,4) → [0,6]×[0,4].
        app.transform_session_drag((6, 4));
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - 0.0).abs() < 1e-3 && (max_x - 6.0).abs() < 1e-3,
            "pre-flip box must extend right from the anchor, got x [{min_x},{max_x}]"
        );
        assert!((min_y - 0.0).abs() < 1e-3 && (max_y - 4.0).abs() < 1e-3);
        // Flip ON: (-2,4) → [-2,0]×[0,4]; the anchor (0,0) is the right edge.
        app.transform_session_drag((-2, 4));
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - (-2.0)).abs() < 1e-3 && (max_x - 0.0).abs() < 1e-3,
            "flipped box must mirror about the SAME anchor, got x [{min_x},{max_x}]"
        );
        assert!((min_y - 0.0).abs() < 1e-3 && (max_y - 4.0).abs() < 1e-3);
        // Flip OFF again: (5,4) → [0,5]×[0,4] about the same anchor.
        app.transform_session_drag((5, 4));
        let (min_x, _, max_x, _) = selection_bbox(&app);
        assert!(
            (min_x - 0.0).abs() < 1e-3 && (max_x - 5.0).abs() < 1e-3,
            "the anchor must not have drifted after the flip round-trip, got x [{min_x},{max_x}]"
        );
    }

    /// A pre-existing mirror (keyboard H/V flip) must survive a resize that
    /// does NOT cross the anchor, and must not move the start bbox: the pixel
    /// flip and the PLACEMENT flip are separate.
    #[test]
    fn corner_resize_keeps_a_pre_existing_flip_until_crossing() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 4, 4));
        app.lift_transform(Rect2i::new(4, 4, 4, 4));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_flips(true, false); // pre-existing H mirror
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (8.0, 8.0);
        }
        // Grow without crossing: the box grows from the SAME (top-left) anchor.
        app.transform_session_drag((12, 12));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert!(t.object.flip_h, "the pre-existing mirror must persist");
            assert!(!t.object.flip_v);
        }
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        assert!(
            (min_x - 4.0).abs() < 1e-3 && (min_y - 4.0).abs() < 1e-3,
            "the start bbox must not move from a pre-existing mirror, got ({min_x},{min_y})"
        );
        assert!(
            (max_x - 12.0).abs() < 1e-3 && (max_y - 12.0).abs() < 1e-3,
            "the resize must still grow the box, got ({max_x},{max_y})"
        );
    }

    /// While a handle drag is live no OTHER gizmo element may start: a second
    /// press on a different handle continues the latched gesture, and the latch
    /// only clears on release.
    #[test]
    fn drag_lock_keeps_the_grabbed_handle_until_release() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 40, 30));
        app.lift_transform(Rect2i::new(4, 4, 40, 30));

        // Press the SE corner handle (bbox (4,4)-(44,34)).
        app.gizmo_hovered = GizmoHit::ScaleSE;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((44, 34)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::ScaleSE);

        // A second press over a DIFFERENT handle must not take over.
        app.gizmo_hovered = GizmoHit::ScaleNE;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((40, 6)),
            ..Default::default()
        });
        assert_eq!(
            selection_drag(&app),
            GizmoHit::ScaleSE,
            "a second handle must not start mid-drag"
        );
        // The live gesture is still the SE resize: the opposite corner (4,4)
        // remains the anchor.
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            (min_x - 4.0).abs() < 1e-3 && (min_y - 4.0).abs() < 1e-3,
            "the live gesture must still be the SE resize, got ({min_x},{min_y})"
        );

        // Release clears the latch...
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((40, 6)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::None);
        // ...so a fresh press CAN grab another handle.
        app.gizmo_hovered = GizmoHit::ScaleNE;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((40, 6)),
            ..Default::default()
        });
        assert_eq!(
            selection_drag(&app),
            GizmoHit::ScaleNE,
            "after release a new handle must be grabbable"
        );
    }

    /// A Translate drag moves BOTH the object's pos and its pivot by the same
    /// delta (spec C2) so a follow-up rotation/scale stays glued to the moved
    /// selection.
    #[test]
    fn transform_session_drag_translate_moves_pivot_with_object() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let pos_before = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pos;
        let pivot_before = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pivot();

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Translate;
            t.last_pt = (5.0, 5.0);
        }
        // Q1 problem 2: the FIRST pixel of movement moves the object — no dead
        // zone. A +1,+1 sample already shifts both pos and pivot by (1, 1).
        app.transform_session_drag((6, 6));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert_eq!(
                t.object.pos,
                (pos_before.0 + 1.0, pos_before.1 + 1.0),
                "the object must follow the first pixel of movement"
            );
            assert_eq!(
                t.object.pivot(),
                (pivot_before.0 + 1.0, pivot_before.1 + 1.0),
                "the pivot moves WITH the object on the first pixel"
            );
        }
        // The delta is recomputed ABSOLUTE from the original press, so a later
        // Δ of (+2, +1) still lands on the intended offset.
        app.transform_session_drag((7, 6));

        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        assert_eq!(t.object.pos, (pos_before.0 + 2.0, pos_before.1 + 1.0));
        assert_eq!(
            t.object.pivot(),
            (pivot_before.0 + 2.0, pivot_before.1 + 1.0),
            "the pivot moves WITH the object"
        );
    }

    /// M1: rotation always happens around the centre pivot, which cannot be
    /// moved — a rotate drag leaves the pivot exactly where it was.
    #[test]
    fn transform_session_rotate_is_about_the_centre_pivot() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let pivot_before = selection_pivot(&app);

        // Pivot (4.5, 4.5): (5.5, 4.5) is at 0°; (5, 5) is at +45°.
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Rotate;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((5, 5));

        let angle = selection_angle(&app);
        assert!(
            (angle - 45.0).abs() < 1e-3,
            "rotation is measured around the centre pivot, got {angle}"
        );
        assert_eq!(
            selection_pivot(&app),
            pivot_before,
            "a rotation keeps the (unmovable) centre pivot fixed"
        );
    }

    /// Shift during a Rotate drag snaps the RESULTING absolute angle to the
    /// nearest pixel-art angle in [`ROTATE_SNAP_DEG`].
    #[test]
    fn transform_session_drag_rotate_snapped_to_pixel_angles() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Rotate;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((4, 3));
        set_modifiers(&app, egui::Modifiers::NONE);

        let angle = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .angle_deg;
        assert!(
            ROTATE_SNAP_DEG.contains(&angle),
            "snapped angle {angle} must be one of {ROTATE_SNAP_DEG:?}"
        );
    }

    /// Without Shift, a Rotate drag accumulates the pointer angle in DEGREES
    /// (radians in, degrees stored) — a known drag around the pivot must land
    /// on ≈ −108.43° and would be thousands of degrees off if `rotate_by`
    /// were fed degrees.
    #[test]
    fn transform_session_drag_rotate_unsnapped_radians() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Rotate;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((4, 3));

        let angle = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .angle_deg;
        assert!(
            (angle - (-108.43)).abs() < 0.05,
            "expected ≈ −108.43°, got {angle} (degrees/radians regression?)"
        );
    }

    /// Rotation is ABSOLUTE from the drag start (`start_angle + Δθ`), never an
    /// accumulation of the object's current angle: corrupting `angle_deg`
    /// mid-drag (as a stale accumulator would) must not change the result.
    #[test]
    fn rotate_is_absolute_from_the_start_angle() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Rotate;
            t.last_pt = (5.5, 4.5);
        }
        app.transform_session_drag((4, 3));
        assert!(
            (selection_angle(&app) - (-108.43)).abs() < 0.05,
            "first frame rotates about the start, got {}",
            selection_angle(&app)
        );

        // Simulate a stale accumulator writing a bogus angle mid-gesture.
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_angle(500.0);
        }
        app.transform_session_drag((4, 3));
        assert!(
            (selection_angle(&app) - (-108.43)).abs() < 0.05,
            "rotation must recompute from the start angle, got {}",
            selection_angle(&app)
        );
    }

    /// A Shift-held MOVE locks to the axis chosen from the INITIAL movement
    /// direction (horizontal here): the vertical component is discarded.
    #[test]
    fn shift_move_locks_to_the_initial_axis() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let start = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pos;
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Translate;
            t.last_pt = (5.0, 5.0);
        }
        // dx = 6, dy = 2 → the FIRST-frame direction is horizontal.
        app.transform_session_drag((11, 7));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert_eq!(
                t.object.pos,
                (start.0 + 6.0, start.1),
                "a horizontal Shift lock discards the vertical movement"
            );
        }
        // A later jitter favouring the OTHER axis must NOT re-pick the latch:
        // dx = 1, dy = 8 still keeps the horizontal axis (dy dropped).
        app.transform_session_drag((6, 13));
        set_modifiers(&app, egui::Modifiers::NONE);

        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        assert_eq!(
            t.object.pos,
            (start.0 + 1.0, start.1),
            "the axis is latched on the first movement and a later jitter must not re-pick it"
        );

        // Vertical initial direction locks the other way.
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let start = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pos;
        set_modifiers(
            &app,
            egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        );
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Translate;
            t.last_pt = (5.0, 5.0);
        }
        app.transform_session_drag((6, 11));
        set_modifiers(&app, egui::Modifiers::NONE);
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        assert_eq!(
            t.object.pos,
            (start.0, start.1 + 6.0),
            "a vertical Shift lock discards the horizontal movement"
        );
    }

    /// Q1 problem 2: a MOVE has NO dead zone — the object follows the FIRST
    /// pixel of pointer movement, integer-snapped and absolute from the press.
    #[test]
    fn move_starts_on_the_first_pixel() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        let start = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pos;
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::Translate;
            t.last_pt = (5.0, 5.0);
        }
        // A single +1 px sample moves the object immediately.
        app.transform_session_drag((6, 5));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert_eq!(
                t.object.pos,
                (start.0 + 1.0, start.1),
                "the first pixel of movement must move the object (no dead zone)"
            );
        }
        // A second +1 px sample keeps tracking absolute from the press.
        app.transform_session_drag((7, 5));
        {
            let t = app
                .projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection();
            assert_eq!(
                t.object.pos,
                (start.0 + 2.0, start.1),
                "the move tracks the pointer absolute from the press"
            );
        }
        // Absolute from the press: bringing the pointer back to the press
        // point restores the start position exactly.
        app.transform_session_drag((5, 5));
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        assert_eq!(
            t.object.pos, start,
            "returning to the press point restores the start position (absolute)"
        );
    }

    /// The drag HUD is built only while a handle is grabbed and reports the
    /// live size / angle / position / scale.
    #[test]
    fn hud_state_is_built_during_a_drag() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        // No drag → no HUD.
        match app.current_overlay() {
            CanvasOverlay::Gizmo { active, hud, .. } => {
                assert_eq!(active, None);
                assert!(hud.is_none(), "no HUD before a handle is grabbed");
            }
            other => panic!("expected a Gizmo overlay, got {other:?}"),
        }

        // Grabbing a handle → active + HUD with the live transform state.
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.object.set_angle(24.0);
        }
        match app.current_overlay() {
            CanvasOverlay::Gizmo { active, hud, .. } => {
                assert_eq!(active, Some(GizmoHit::ScaleSE));
                let hud = hud.expect("a HUD while dragging");
                // A 2×2 box rotated 24° has a ceil'd rotated bbox of 3×3.
                assert_eq!(hud.size, (3, 3));
                assert!((hud.angle_deg - 24.0).abs() < 1e-3);
                assert_eq!(hud.pos, (4, 4));
                assert_eq!(hud.scale, (1.0, 1.0));
            }
            other => panic!("expected a Gizmo overlay, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // F1 issue #1: the REAL grab path (hit_test → press → drag)
    // -----------------------------------------------------------------------

    /// The gizmo handle the REAL canvas widget would report for a screen
    /// position: the same identity-camera mapping `canvas.rs` feeds to
    /// `hit_test` (origin (0,0), pan (0,0), scale 1). Using this instead of a
    /// hand-set `gizmo_hovered` means the test fails if the hit-test geometry
    /// regresses.
    fn gizmo_hit_at(app: &App, pos: egui::Pos2) -> GizmoHit {
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .expect("a live transform session")
            .expect_selection();
        crate::render::gizmo::hit_test(
            pos,
            t.object.canvas_corners(),
            egui::pos2(0.0, 0.0),
            (0, 0),
            1.0,
        )
    }

    fn selection_drag(app: &App) -> GizmoHit {
        app.projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .drag
    }

    fn selection_scale(app: &App) -> (f32, f32) {
        app.projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .scale_xy()
    }

    fn selection_angle(app: &App) -> f32 {
        app.projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .angle_deg
    }

    /// Unrotated cursor table helper (angle 0) for the base mapping test.
    fn cursor_for_hit(hit: GizmoHit) -> Option<egui::CursorIcon> {
        cursor_for_hit_at_angle(hit, 0.0)
    }

    fn selection_bbox(app: &App) -> (f32, f32, f32, f32) {
        app.projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .canvas_bbox()
    }

    fn selection_pivot(app: &App) -> (f32, f32) {
        app.projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pivot()
    }

    /// The floating object's canvas `pos` (top-left of the identity placement).
    fn selection_pos(app: &App) -> (f32, f32) {
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        t.object.pos
    }

    /// The floating object's mirror flags `(flip_h, flip_v)`.
    fn selection_flips(app: &App) -> (bool, bool) {
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        (t.object.flip_h, t.object.flip_v)
    }

    /// Issue #1 regression: grabbing the SE corner through the real hit-test
    /// path resizes. Crucially, egui re-fires the drag start AFTER the pointer
    /// has moved off the handle; the old code re-latched from that
    /// post-movement hover (`None`) and dropped the grab, so nothing resized.
    #[test]
    fn gizmo_corner_grab_resizes_through_the_real_hit_test_path() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        // bbox (4,4)-(12,12): the SE handle is at screen (12,12).
        assert_eq!(
            gizmo_hit_at(&app, egui::pos2(12.0, 12.0)),
            GizmoHit::ScaleSE,
            "the real hit-test must resolve the SE corner handle"
        );
        app.gizmo_hovered = gizmo_hit_at(&app, egui::pos2(12.0, 12.0));

        // Press on the handle: latch ScaleSE.
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((12, 12)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::ScaleSE);

        // Re-fired drag start at a point that is now empty space. (The handle
        // hit rects are 15 px; (20,20) is just clear of the SE corner square.)
        assert_eq!(
            gizmo_hit_at(&app, egui::pos2(20.0, 20.0)),
            GizmoHit::None,
            "the moved point must be outside the corner square and rotation zone"
        );
        app.gizmo_hovered = gizmo_hit_at(&app, egui::pos2(20.0, 20.0));
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((20, 20)),
            ..Default::default()
        });
        assert_eq!(
            selection_drag(&app),
            GizmoHit::ScaleSE,
            "a re-fired start must not clobber the latched grab"
        );

        let (sx, sy) = selection_scale(&app);
        assert!(
            sx > 1.0 && sy > 1.0,
            "the corner drag must resize, got ({sx}, {sy})"
        );
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            (min_x - 4.0).abs() < 1e-3 && (min_y - 4.0).abs() < 1e-3,
            "the opposite corner (4,4) must stay fixed"
        );

        // Release clears the latch so the next press can grab again.
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((19, 19)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::None);
    }

    /// Issue #1: an edge grab through the real hit-test path resizes along its
    /// axis only.
    #[test]
    fn gizmo_edge_grab_resizes_along_its_axis_through_the_real_path() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 16, 8));
        app.lift_transform(Rect2i::new(4, 4, 16, 8));
        // bbox (4,4)-(20,12): the top-edge midpoint is at screen (12,4).
        assert_eq!(
            gizmo_hit_at(&app, egui::pos2(12.0, 4.0)),
            GizmoHit::ScaleTop,
            "the real hit-test must resolve the top edge handle"
        );
        app.gizmo_hovered = gizmo_hit_at(&app, egui::pos2(12.0, 4.0));
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((12, 4)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::ScaleTop);
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((12, 0)),
            ..Default::default()
        });

        let (sx, sy) = selection_scale(&app);
        assert_eq!(sx, 1.0, "a top-edge drag must not touch the X axis");
        assert!(sy > 1.0, "a top-edge drag must grow the Y axis");
        let (_, min_y, _, max_y) = selection_bbox(&app);
        assert!(
            (max_y - 12.0).abs() < 1e-3,
            "the bottom anchor stays fixed, got max_y={max_y}"
        );
        assert!(min_y < 4.0, "the top edge moves up, got min_y={min_y}");
    }

    /// Issue #1: the diagonal rotation zone (strictly disjoint from the corner
    /// square) rotates through the real hit-test path.
    #[test]
    fn gizmo_rotation_zone_rotates_through_the_real_path() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 16, 8));
        app.lift_transform(Rect2i::new(4, 4, 16, 8));
        let pivot = selection_pivot(&app);
        // SE rotation zone centre: corner (20,12) + (15,15) = (35,27).
        assert_eq!(
            gizmo_hit_at(&app, egui::pos2(35.0, 27.0)),
            GizmoHit::Rotate,
            "the real hit-test must resolve the SE rotation zone"
        );
        app.gizmo_hovered = gizmo_hit_at(&app, egui::pos2(35.0, 27.0));
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((35, 27)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::Rotate);
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((12, 30)),
            ..Default::default()
        });

        let angle = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .angle_deg;
        assert!(
            angle.abs() > 1.0,
            "the rotation zone must rotate, got {angle}"
        );
        assert_eq!(
            selection_pivot(&app),
            pivot,
            "rotation keeps the pivot fixed"
        );
    }

    /// M1: the centre mark is NOT interactive. Through the real hit-test path a
    /// point at the bbox centre resolves to `Translate`, never a pivot handle,
    /// so the pivot cannot be grabbed or moved.
    #[test]
    fn gizmo_centre_is_translate_not_a_pivot_handle_through_the_real_path() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 40, 30));
        app.lift_transform(Rect2i::new(4, 4, 40, 30));
        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        let centre = egui::pos2((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
        assert_eq!(
            gizmo_hit_at(&app, centre),
            GizmoHit::Translate,
            "the bbox centre must hit-test as the translate interior, never a pivot handle"
        );
        // A press there starts a Translate (the centre mark has no grab).
        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((24, 19)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::Translate);
    }

    /// O1: the overlay gizmo carries the object's ROTATED quad, and a rotated
    /// corner / the centre resolve through the real hit-test mapping.
    #[test]
    fn current_overlay_gizmo_corners_rotate_with_the_content() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 40, 30));
        app.lift_transform(Rect2i::new(4, 4, 40, 30));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_angle(30.0);
        }
        let expected = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .canvas_corners();
        let corners = match app.current_overlay() {
            CanvasOverlay::Gizmo { corners, .. } => corners,
            other => panic!("expected a Gizmo overlay, got {other:?}"),
        };
        assert_eq!(
            corners, expected,
            "the overlay must carry the object's rotated quad"
        );
        // The quad really rotated away from the old axis-aligned bbox: its
        // leading edge is neither horizontal nor vertical.
        let edge = (corners[1].0 - corners[0].0, corners[1].1 - corners[0].1);
        assert!(
            edge.0.abs() > 1e-3 && edge.1.abs() > 1e-3,
            "the overlay quad must be rotated, got edge {edge:?}"
        );
        // A rotated corner resolves to its scale handle via the real mapping.
        assert_eq!(
            gizmo_hit_at(&app, egui::pos2(corners[2].0, corners[2].1)),
            GizmoHit::ScaleSE,
            "the rotated BR corner must be the SE handle"
        );
        let centre = (
            (corners[0].0 + corners[1].0 + corners[2].0 + corners[3].0) * 0.25,
            (corners[0].1 + corners[1].1 + corners[2].1 + corners[3].1) * 0.25,
        );
        assert_eq!(
            gizmo_hit_at(&app, egui::pos2(centre.0, centre.1)),
            GizmoHit::Translate,
            "the quad centre is the translate interior"
        );
    }

    /// Issue #1: an interior drag translates the object (through the real
    /// hit-test path, so it is not shadowed by any handle).
    #[test]
    fn gizmo_interior_drag_translates_through_the_real_path() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 40, 30));
        app.lift_transform(Rect2i::new(4, 4, 40, 30));
        let pos_before = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pos;
        assert_eq!(
            gizmo_hit_at(&app, egui::pos2(36.0, 26.0)),
            GizmoHit::Translate,
            "the real hit-test must resolve the interior"
        );
        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((36, 26)),
            ..Default::default()
        });
        assert_eq!(selection_drag(&app), GizmoHit::Translate);
        // Q1 problem 2: no dead zone — the first moved sample applies, and the
        // delta stays ABSOLUTE from the original press (not accumulated).
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((38, 27)),
            ..Default::default()
        });
        let pos = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .pos;
        assert_eq!(pos, (pos_before.0 + 2.0, pos_before.1 + 1.0));
    }

    /// Issue #1 end-to-end: a full App frame sequence (raw pointer input
    /// through the real canvas widget, including egui's drag-threshold
    /// re-fire of `stroke_started`) presses the SE handle and drags it out.
    /// Complements the constructed hit-test test with the real screen mapping.
    #[test]
    fn gizmo_corner_grab_resizes_in_a_full_app_frame_sequence() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        // The demo dock panel floats over the canvas top-left; disable it so
        // the handle is reachable exactly like in the real editor.
        app.panel_dock_demo = false;
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        marquee(&mut app, (4, 4), (11, 11));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        // Establish the canvas origin / camera before mapping the handle.
        run_app_frame(&mut app, &ctx, vec![]);
        let camera = app.projects.current().camera;
        let se = app
            .canvas_widget
            .canvas_rect_to_screen(camera, Rect2i::new(4, 4, 8, 8))
            .max;

        run_app_frame(&mut app, &ctx, vec![move_to(se)]);
        assert_eq!(
            app.gizmo_hovered,
            GizmoHit::ScaleSE,
            "hover at {se:?} must be the SE corner"
        );
        run_app_frame(&mut app, &ctx, vec![press(se)]);
        assert_eq!(selection_drag(&app), GizmoHit::ScaleSE);

        // Cross the egui drag threshold (this re-fires stroke_started at the
        // moved point) and keep dragging outward.
        run_app_frame(&mut app, &ctx, vec![move_to(se + egui::vec2(6.0, 6.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(se + egui::vec2(12.0, 12.0))]);
        let (sx, sy) = selection_scale(&app);
        assert!(
            sx > 1.0 && sy > 1.0,
            "the full-app corner drag must resize, got ({sx}, {sy})"
        );
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            (min_x - 4.0).abs() < 1e-3 && (min_y - 4.0).abs() < 1e-3,
            "the opposite corner (4,4) must stay fixed"
        );
        run_app_frame(&mut app, &ctx, vec![release(se + egui::vec2(12.0, 12.0))]);
    }

    // -----------------------------------------------------------------------
    // F1 issue #2: pivot stays at the centre of the transformed pixels
    // -----------------------------------------------------------------------

    /// Issue #2: after a corner scale the pivot is recentred to the
    /// transformed bbox centre, while the anchored opposite corner and the
    /// rendered output stay exactly where the resize put them.
    #[test]
    fn pivot_stays_centered_after_corner_scale() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        let pivot_before = selection_pivot(&app);
        assert_eq!(pivot_before, (7.5, 7.5));

        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (12.0, 12.0);
        }
        app.transform_session_drag((16, 16));

        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        // The opposite corner (4,4) stays fixed and the output is the resized
        // box (8×8 → 12×12 at 1.5×) — the recentring must not move it.
        assert!(
            (min_x - 4.0).abs() < 1e-3 && (min_y - 4.0).abs() < 1e-3,
            "opposite corner must stay fixed, got ({min_x}, {min_y})"
        );
        assert!(
            (max_x - 16.0).abs() < 1e-3 && (max_y - 16.0).abs() < 1e-3,
            "the resize output must be preserved, got max ({max_x}, {max_y})"
        );
        let pivot = selection_pivot(&app);
        assert!(
            (pivot.0 - (min_x + max_x) * 0.5).abs() < 1e-3
                && (pivot.1 - (min_y + max_y) * 0.5).abs() < 1e-3,
            "the pivot must be the transformed bbox centre, got {pivot:?} vs bbox ({min_x},{min_y})-({max_x},{max_y})"
        );
        assert!(
            (pivot.0 - 10.0).abs() < 1e-3 && (pivot.1 - 10.0).abs() < 1e-3,
            "the recentred pivot must be (10, 10), got {pivot:?}"
        );
    }

    /// Issue #2: after a single-axis edge scale the pivot is recentred to the
    /// transformed bbox centre too.
    #[test]
    fn pivot_stays_centered_after_edge_scale() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.drag = GizmoHit::ScaleTop;
            t.last_pt = (8.0, 4.0);
        }
        app.transform_session_drag((8, 0));

        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        let pivot = selection_pivot(&app);
        assert!(
            (pivot.0 - (min_x + max_x) * 0.5).abs() < 1e-3
                && (pivot.1 - (min_y + max_y) * 0.5).abs() < 1e-3,
            "the pivot must be the transformed bbox centre after an edge scale, got {pivot:?}"
        );
        assert!(
            (pivot.0 - 8.0).abs() < 1e-3 && (pivot.1 - 6.0).abs() < 1e-3,
            "the recentred pivot must be (8, 6), got {pivot:?}"
        );
        // The bottom anchor is untouched by the recentring.
        assert!(
            (max_y - 12.0).abs() < 1e-3,
            "the bottom anchor stays fixed, got max_y={max_y}"
        );
    }

    /// Issue #2: the recentring compensation is rotation-aware — on a rotated
    /// object the pivot still lands exactly on the transformed bbox centre and
    /// the output is preserved (a straight-axis correction would drift).
    #[test]
    fn pivot_stays_centered_after_corner_scale_on_rotated_object() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
        app.lift_transform(Rect2i::new(4, 4, 8, 8));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.rotate_by(30f32.to_radians());
            t.drag = GizmoHit::ScaleSE;
            t.last_pt = (12.0, 12.0);
        }
        app.transform_session_drag((16, 16));

        let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
        let pivot = selection_pivot(&app);
        assert!(
            (pivot.0 - (min_x + max_x) * 0.5).abs() < 1e-3
                && (pivot.1 - (min_y + max_y) * 0.5).abs() < 1e-3,
            "the pivot must be the transformed bbox centre on a rotated object, got {pivot:?} vs bbox ({min_x},{min_y})-({max_x},{max_y})"
        );
    }

    /// M1: the pivot is ALWAYS the transformed bbox centre after a scale — the
    /// user cannot move it, so both a corner and an edge scale recentre it
    /// (there is no manual latch any more).
    #[test]
    fn pivot_is_always_the_bbox_centre_after_scaling() {
        for hit in [GizmoHit::ScaleSE, GizmoHit::ScaleRight] {
            let mut app = App::default();
            seed_red_rect(&mut app, Rect2i::new(4, 4, 8, 8));
            app.lift_transform(Rect2i::new(4, 4, 8, 8));
            let (_min_x, min_y, max_x, max_y) = selection_bbox(&app);
            let sp = match hit {
                GizmoHit::ScaleSE => (max_x, max_y),
                GizmoHit::ScaleRight => (max_x, (min_y + max_y) * 0.5),
                _ => unreachable!(),
            };
            let cur = match hit {
                GizmoHit::ScaleSE => (max_x + 6.0, max_y + 6.0),
                GizmoHit::ScaleRight => (max_x + 6.0, (min_y + max_y) * 0.5),
                _ => unreachable!(),
            };
            {
                let t = app
                    .projects
                    .current_mut()
                    .transform
                    .as_mut()
                    .unwrap()
                    .expect_selection_mut();
                t.drag = hit;
                t.last_pt = sp;
            }
            app.transform_session_drag((cur.0.round() as i32, cur.1.round() as i32));

            let (min_x, min_y, max_x, max_y) = selection_bbox(&app);
            let pivot = selection_pivot(&app);
            assert!(
                (pivot.0 - (min_x + max_x) * 0.5).abs() < 1e-3
                    && (pivot.1 - (min_y + max_y) * 0.5).abs() < 1e-3,
                "{hit:?}: the pivot must be the transformed bbox centre, got {pivot:?} vs \
                 ({min_x},{min_y})-({max_x},{max_y})"
            );
        }
    }

    /// Esc on a MASKED selection restores the selection WITH its mask (the
    /// mask survives the cancel round-trip), not as a widened plain rect.
    #[test]
    fn esc_cancels_and_restores_a_masked_selection() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        let mask = vec![true, true, true, false];
        install_masked_selection(&mut app, Rect2i::new(4, 4, 2, 2), mask.clone());
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        translate_and_release(&mut app);

        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);

        assert!(app.projects.current().transform.is_none());
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        let restored = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("the cancelled transform must restore the masked selection");
        assert!(!restored.is_rectangular(), "the mask must be preserved");
        assert_eq!(restored.rect(), Rect2i::new(4, 4, 2, 2));
        assert_eq!(restored.mask(), Some(mask.as_slice()));
    }

    /// The Rotate-CW shortcut (R) during a live session rotates by exactly
    /// 90° — a `rotate_by(90.0)` degrees/radians regression would blow the
    /// angle up to thousands of degrees.
    #[test]
    fn transform_rotate_shortcut_is_90_degrees() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        send_key(&mut app, egui::Key::R, egui::Modifiers::NONE);

        let angle = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection()
            .object
            .angle_deg;
        assert!(
            (angle - 90.0).abs() < 1e-3,
            "the Rotate-CW shortcut must land on 90°, got {angle}"
        );
    }

    /// Pure table test of the gizmo→cursor mapping (unrotated base).
    #[test]
    fn cursor_for_hit_mapping() {
        assert_eq!(cursor_for_hit(GizmoHit::None), None);
        assert_eq!(
            cursor_for_hit(GizmoHit::Translate),
            Some(egui::CursorIcon::Grab)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleNW),
            Some(egui::CursorIcon::ResizeNwSe)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleSE),
            Some(egui::CursorIcon::ResizeNwSe)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleNE),
            Some(egui::CursorIcon::ResizeNeSw)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleSW),
            Some(egui::CursorIcon::ResizeNeSw)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleTop),
            Some(egui::CursorIcon::ResizeVertical)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleBottom),
            Some(egui::CursorIcon::ResizeVertical)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleLeft),
            Some(egui::CursorIcon::ResizeHorizontal)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::ScaleRight),
            Some(egui::CursorIcon::ResizeHorizontal)
        );
        assert_eq!(
            cursor_for_hit(GizmoHit::Rotate),
            Some(egui::CursorIcon::Crosshair)
        );
    }

    /// The resize cursor follows the SCREEN orientation of a rotated bbox: a
    /// 90° rotation swaps each corner's diagonal and each edge's axis, while the
    /// rotate cursor stays a fixed Crosshair.
    #[test]
    fn cursor_for_hit_follows_rotation() {
        // Corner diagonals flip every 90°.
        assert_eq!(
            cursor_for_hit_at_angle(GizmoHit::ScaleNW, 0.0),
            Some(egui::CursorIcon::ResizeNwSe)
        );
        assert_eq!(
            cursor_for_hit_at_angle(GizmoHit::ScaleNW, 90.0),
            Some(egui::CursorIcon::ResizeNeSw)
        );
        assert_eq!(
            cursor_for_hit_at_angle(GizmoHit::ScaleNW, 180.0),
            Some(egui::CursorIcon::ResizeNwSe)
        );
        assert_eq!(
            cursor_for_hit_at_angle(GizmoHit::ScaleNW, 270.0),
            Some(egui::CursorIcon::ResizeNeSw)
        );
        // Edges flip axis every 90°.
        assert_eq!(
            cursor_for_hit_at_angle(GizmoHit::ScaleTop, 90.0),
            Some(egui::CursorIcon::ResizeHorizontal)
        );
        assert_eq!(
            cursor_for_hit_at_angle(GizmoHit::ScaleLeft, 90.0),
            Some(egui::CursorIcon::ResizeVertical)
        );
        // Rotate stays put regardless of angle.
        assert_eq!(
            cursor_for_hit_at_angle(GizmoHit::Rotate, 137.0),
            Some(egui::CursorIcon::Crosshair)
        );
    }

    /// A drag release on a selection transform only ENDS the drag: the session
    /// stays live, the identity transform is not committed, and the pixels are
    /// untouched. Committing needs a double-click on empty space (or a tool
    /// switch / deselect); Esc cancels and restores.
    #[test]
    fn selection_transform_release_only_ends_the_drag() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 1, 1));
        app.lift_transform(Rect2i::new(4, 4, 1, 1));
        let before = app
            .projects
            .current()
            .layers
            .active_layer()
            .buffer
            .as_bytes()
            .to_vec();

        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });

        assert!(
            app.projects.current().transform.is_some(),
            "a release must not commit the session"
        );
        assert_eq!(
            app.projects
                .current()
                .transform
                .as_ref()
                .expect("the session survives the release")
                .expect_selection()
                .drag,
            GizmoHit::None,
            "a release only drops the grabbed gizmo"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "an uncommitted session pushes nothing"
        );
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .as_bytes()
                .to_vec(),
            before,
            "the document stays byte-identical until the commit gesture"
        );
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(4, 4),
            Some(Color::TRANSPARENT),
            "Part C: the lift already cut the source pixel from the layer"
        );
    }

    const BLUE: Color = Color::rgb(0, 0, 255);

    #[test]
    fn transform_session_flip_h_commit_mirrors_horizontally() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(5, 4, BLUE);
        marquee(&mut app, (4, 4), (5, 5));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_flips(true, false);
        }
        app.transform_session_commit();

        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(BLUE));
        assert_eq!(buf.get_pixel(5, 4), Some(RED));
    }

    #[test]
    fn transform_session_flip_v_commit_mirrors_vertically() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(4, 5, BLUE);
        marquee(&mut app, (4, 4), (5, 5));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_flips(false, true);
        }
        app.transform_session_commit();

        assert_eq!(app.projects.current_mut().undo.undo_len(), 1);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(BLUE));
        assert_eq!(buf.get_pixel(4, 5), Some(RED));
    }

    /// Double flip (H then H again) cancels itself: the committed pixels are
    /// byte-identical to the source, so no undo entry is pushed.
    #[test]
    fn transform_session_double_flip_h_is_identity() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(5, 4, BLUE);
        marquee(&mut app, (4, 4), (5, 5));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        {
            let t = app
                .projects
                .current_mut()
                .transform
                .as_mut()
                .unwrap()
                .expect_selection_mut();
            t.object.set_flips(true, false);
            t.object.set_flips(false, false);
        }
        app.transform_session_commit();

        // Identity re-paste: no undo entry pushed.
        assert_eq!(app.projects.current_mut().undo.undo_len(), 0);
        let buf = &app.projects.current_mut().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(RED));
        assert_eq!(buf.get_pixel(5, 4), Some(BLUE));
    }

    #[test]
    fn document_roundtrip_preserves_layers_and_frames() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        app.projects.current_mut().sequence.push(Frame::new(
            Region::new(Rect2i::new(0, 0, 8, 8), "Frame 2"),
            250,
        ));

        let doc = app.app_to_document();
        let mut restored = App::default();
        restored.document_to_app(&doc);
        let doc2 = restored.app_to_document();

        assert_eq!(doc, doc2);
        assert_eq!(
            restored.projects.current().layers.width(),
            app.projects.current_mut().layers.width()
        );
        assert_eq!(
            restored.projects.current().layers.height(),
            app.projects.current_mut().layers.height()
        );
        assert_eq!(restored.projects.current().sequence.len(), 2);
        assert_eq!(
            restored
                .projects
                .current()
                .sequence
                .iter()
                .nth(1)
                .unwrap()
                .delay_ms(),
            250
        );
        assert_eq!(
            restored
                .projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(4, 4),
            Some(RED)
        );
    }

    #[test]
    fn stroke_sets_modified_until_saved() {
        let mut app = App::default();
        assert!(!app.projects.current().is_dirty());
        // Painting red on transparent pixels changes the document (an undo
        // command is pushed), so the document becomes modified.
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((4, 4)),
            ..Default::default()
        });
        assert!(app.projects.current().is_dirty());
        // A same-color fill (seed color already matches) pushes no command
        // and must not dirty an already-clean document.
        app.projects.current_mut().mark_saved();
        app.apply_fill((2, 2));
        assert!(app.projects.current().is_dirty());
        let cmd_pushed = app.projects.current_mut().undo.undo_len();
        app.apply_fill((2, 2));
        assert_eq!(app.projects.current_mut().undo.undo_len(), cmd_pushed);
    }

    #[test]
    fn selecting_palette_marks_project_modified() {
        let mut app = App::default();
        app.projects.current_mut().palettes = vec![
            Palette::new("One", vec![[1, 2, 3, 255]]).unwrap(),
            Palette::new("Two", vec![[4, 5, 6, 255]]).unwrap(),
        ];
        app.projects.current_mut().mark_saved();
        app.apply_toolbar_events(vec![ToolbarEvent::PaletteSelected(1)]);
        assert_eq!(app.projects.current_mut().active_palette, 1);
        assert_eq!(app.projects.current_mut().color, Color::rgba(4, 5, 6, 255));
        assert!(app.projects.current().is_dirty());
    }

    fn send_key(app: &mut App, key: egui::Key, modifiers: egui::Modifiers) {
        let ctx = app.ctx.clone();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw_input, |_ui| app.handle_shortcuts());
        output.textures_delta.clear();
    }

    /// Run `handle_shortcuts` with an arbitrary event list (so tests can feed
    /// the `Event::Copy`/`Event::Cut` / key-release events that `egui-winit`
    /// delivers in place of the raw clipboard key presses).
    fn run_shortcuts(app: &mut App, events: Vec<egui::Event>) {
        let ctx = app.ctx.clone();
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw_input, |_ui| app.handle_shortcuts());
        output.textures_delta.clear();
    }

    /// Commit a square selection over `rect` (assuming the pixels are already
    /// painted) and return it.
    fn commit_selection(app: &mut App, rect: Rect2i) {
        let sel = Selection::capture(&app.projects.current().layers.active_layer().buffer, rect)
            .expect("the selection captures the seeded pixels");
        app.projects.current_mut().selection = Some(sel);
    }

    #[test]
    fn ctrl_c_event_copies_the_selection() {
        // Given a committed selection over painted pixels.
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        commit_selection(&mut app, Rect2i::new(2, 2, 3, 3));
        assert!(app.projects.current().clipboard.is_none());

        // When: egui-winit delivers Ctrl+C as `Event::Copy` (no Key press).
        run_shortcuts(&mut app, vec![egui::Event::Copy]);

        // Then: the internal clipboard holds the selection's pixels.
        let clip = app
            .projects
            .current_mut()
            .clipboard
            .clone()
            .expect("Event::Copy populates the internal clipboard");
        assert_eq!((clip.width(), clip.height()), (3, 3));
        assert_eq!(clip.get_pixel(0, 0), Some(RED));
        assert_eq!(clip.get_pixel(2, 2), Some(RED));
    }

    #[test]
    fn ctrl_x_event_cuts_the_selection() {
        // Given a committed selection over painted pixels.
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        commit_selection(&mut app, Rect2i::new(2, 2, 3, 3));
        assert_eq!(app.projects.current().undo.undo_len(), 0);

        // When: egui-winit delivers Ctrl+X as `Event::Cut` (no Key press).
        run_shortcuts(&mut app, vec![egui::Event::Cut]);

        // Then: the selected pixels are cleared, the clipboard is set, and the
        // cut is exactly one undo step.
        let clip = app
            .projects
            .current_mut()
            .clipboard
            .clone()
            .expect("Event::Cut populates the internal clipboard");
        assert_eq!(clip.get_pixel(0, 0), Some(RED));
        {
            let buf = &app.projects.current_mut().layers.active_layer().buffer;
            assert_eq!(buf.get_pixel(2, 2), Some(Color::TRANSPARENT));
            assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
        }
        assert_eq!(app.projects.current().undo.undo_len(), 1, "one undo step");
        assert!(
            app.projects.current().selection.is_none(),
            "cut drops the selection"
        );
    }

    #[test]
    fn ctrl_v_release_event_requests_a_paste() {
        // Given: egui-winit swallows the Ctrl+V press, but the RELEASE falls
        // through as a `Key{pressed:false}` event with the command modifier.
        let mut app = App::default();
        let command = egui::Modifiers {
            command: true,
            ctrl: true,
            ..egui::Modifiers::NONE
        };
        assert!(app.os_paste_rx.is_none());

        // When: the Ctrl+V release is delivered (with the pointer on-canvas).
        run_shortcuts(
            &mut app,
            vec![
                modifiers_changed(command),
                move_to(egui::pos2(20.0, 20.0)),
                egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: command,
                },
            ],
        );

        // Then: a (background) paste read was requested.
        assert!(
            app.os_paste_rx.is_some(),
            "the Ctrl+V release must request a paste"
        );

        // A plain V press with no command must NOT request a paste.
        let mut plain = App::default();
        run_shortcuts(
            &mut plain,
            vec![
                move_to(egui::pos2(20.0, 20.0)),
                egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(
            plain.os_paste_rx.is_none(),
            "a plain V press is not a paste"
        );
    }

    #[test]
    fn custom_paste_binding_still_uses_the_keymap() {
        // Given: the Paste action rebound to a non-Command key.
        let mut app = App::default();
        app.keymap.bindings.insert(
            crate::input::Action::Paste,
            crate::input::KeyBinding {
                key: crate::input::LogicalKey::L,
                modifiers: crate::input::Modifiers::default(),
            },
        );

        // When: a plain L press is delivered.
        run_shortcuts(
            &mut app,
            vec![
                move_to(egui::pos2(20.0, 20.0)),
                egui::Event::Key {
                    key: egui::Key::L,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );

        // Then: the retained keymap path still requests the paste.
        assert!(
            app.os_paste_rx.is_some(),
            "a custom paste binding must go through the keymap"
        );
    }

    /// H1 second round: the Ctrl+V release must be detected even when the
    /// backend only sets `ctrl` (not `command`). egui-winit maps
    /// `command = ctrl` on Linux, but `command || ctrl` keeps the release path
    /// working if a backend/build ever differs.
    #[test]
    fn paste_release_detects_with_ctrl_only_modifiers() {
        let mut app = App::default();
        assert!(app.os_paste_rx.is_none());

        let ctrl_only = egui::Modifiers {
            ctrl: true,
            command: false,
            ..egui::Modifiers::NONE
        };
        run_shortcuts(
            &mut app,
            vec![
                modifiers_changed(ctrl_only),
                move_to(egui::pos2(20.0, 20.0)),
                egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: ctrl_only,
                },
            ],
        );

        assert!(
            app.os_paste_rx.is_some(),
            "a ctrl-only Ctrl+V release must still request a paste"
        );
    }

    /// H1 second round: a pending OS read that never completes must NOT wedge
    /// paste. After the bounded wait the receiver is dropped, the internal
    /// project clipboard opens the paste session at the captured cursor, and
    /// the in-flight slot is cleared.
    #[test]
    fn stuck_os_paste_falls_back_to_internal_clipboard() {
        let mut app = App::default();
        let mut pixels = vec![0u8; 2 * 2 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        app.projects.current_mut().clipboard = Some(ClipboardRegion::new(2, 2, pixels));

        // A pending read whose sender never sends. Keep `_tx` alive so the
        // channel stays connected (a dropped sender would instead resolve as a
        // completed no-image read).
        app.begin_os_paste((10, 10));
        let (_tx, rx) = std::sync::mpsc::channel();
        app.os_paste_rx = Some(crate::ui::clipboard::PendingPasteRead::start(rx));
        assert!(app.os_paste_cursor.is_some());

        // Drive the frame counter past the deadline.
        for _ in 0..crate::ui::clipboard::PASTE_WAIT_FRAMES {
            app.poll_os_paste();
        }

        // The timed-out read is fully cleared (unwedged) …
        assert!(
            app.os_paste_rx.is_none(),
            "a timed-out OS read must be dropped, not kept forever"
        );
        // … and the internal clipboard fallback opened the paste session.
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .expect("the internal clipboard fallback opens a paste session")
            .expect_selection();
        assert_eq!(t.object.bounds_size(), (2, 2));
        assert_eq!(
            t.object.pos,
            (9.0, 9.0),
            "the fallback paste is centred on the captured cursor"
        );
        assert!(
            t.source.snapshot.is_empty(),
            "a paste session has no source snapshot"
        );
    }

    /// H1 second round: after a timed-out read the NEXT Ctrl+V must start a
    /// fresh OS read — proving the stacking guard was released and paste is not
    /// permanently wedged.
    #[test]
    fn paste_request_recovers_after_timeout() {
        let mut app = App::default();
        let mut pixels = vec![0u8; 2 * 2 * 4];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[255, 0, 0, 255]);
        }
        app.projects.current_mut().clipboard = Some(ClipboardRegion::new(2, 2, pixels));

        // Force a read that never completes, then drive it past the deadline.
        app.begin_os_paste((10, 10));
        let (_tx, rx) = std::sync::mpsc::channel();
        app.os_paste_rx = Some(crate::ui::clipboard::PendingPasteRead::start(rx));
        for _ in 0..crate::ui::clipboard::PASTE_WAIT_FRAMES {
            app.poll_os_paste();
        }
        assert!(app.os_paste_rx.is_none(), "the timed-out read is cleared");
        assert!(
            app.projects.current().transform.is_some(),
            "the fallback paste session opened"
        );

        // Drop the fallback session: `handle_shortcuts` suspends clipboard keys
        // while a transform is live.
        app.cancel_transform();
        assert!(app.projects.current().transform.is_none());

        // A fresh Ctrl+V release must request a new read (no permanent wedge).
        let command = egui::Modifiers {
            command: true,
            ctrl: true,
            ..egui::Modifiers::NONE
        };
        run_shortcuts(
            &mut app,
            vec![
                modifiers_changed(command),
                move_to(egui::pos2(20.0, 20.0)),
                egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: command,
                },
            ],
        );
        assert!(
            app.os_paste_rx.is_some(),
            "a fresh Ctrl+V after a timeout must request a new paste"
        );
    }

    /// H1 second round: reprise exactly what egui-winit emits for a real Ctrl+C
    /// — a `ModifiersChanged{command,ctrl}` followed by `Event::Copy` — and
    /// confirm it copies the selection. This guards the full raw-input shape,
    /// not just the bare `Event::Copy`.
    #[test]
    fn realistic_copy_sequence_copies() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 3, 3));
        commit_selection(&mut app, Rect2i::new(2, 2, 3, 3));

        let mods = egui::Modifiers {
            command: true,
            ctrl: true,
            ..egui::Modifiers::NONE
        };
        run_shortcuts(&mut app, vec![modifiers_changed(mods), egui::Event::Copy]);

        let clip = app
            .projects
            .current_mut()
            .clipboard
            .clone()
            .expect("the realistic Ctrl+C sequence copies the selection");
        assert_eq!((clip.width(), clip.height()), (3, 3));
        assert_eq!(clip.get_pixel(0, 0), Some(RED));
        assert_eq!(clip.get_pixel(2, 2), Some(RED));
    }

    #[test]
    fn customized_toggle_grid_binding_dispatches_action() {
        let mut app = App::default();
        app.keymap.bindings.insert(
            crate::input::Action::ToggleGrid,
            crate::input::KeyBinding {
                key: crate::input::LogicalKey::H,
                modifiers: crate::input::Modifiers::default(),
            },
        );
        assert!(app.projects.current_mut().grid_visible);
        send_key(&mut app, egui::Key::H, egui::Modifiers::NONE);
        assert!(!app.projects.current_mut().grid_visible);
    }

    #[test]
    fn d_selects_the_pen_and_e_selects_the_eraser() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Fill);

        send_key(&mut app, egui::Key::D, egui::Modifiers::NONE);
        assert_eq!(app.projects.current().tool_state.tool(), Tool::Pencil);
        assert_eq!(app.projects.current().tool_state.previous(), None);

        send_key(&mut app, egui::Key::E, egui::Modifiers::NONE);
        assert_eq!(app.projects.current().tool_state.tool(), Tool::Eraser);
        assert_eq!(app.projects.current().tool_state.previous(), None);

        send_key(&mut app, egui::Key::D, egui::Modifiers::NONE);
        assert_eq!(
            app.projects.current().tool_state.tool(),
            Tool::Pencil,
            "D returns to the pen after an eraser switch"
        );
    }

    #[test]
    fn document_shortcut_is_blocked_during_transform_session() {
        let mut app = App::default();
        app.apply_fill((1, 1));
        let undo_before = app.projects.current_mut().undo.undo_len();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 1, 1));
        app.lift_transform(Rect2i::new(4, 4, 1, 1));
        assert!(app.projects.current_mut().transform.is_some());
        send_key(
            &mut app,
            egui::Key::Z,
            egui::Modifiers {
                command: true,
                ..egui::Modifiers::NONE
            },
        );
        assert_eq!(app.projects.current_mut().undo.undo_len(), undo_before);
        assert!(app.projects.current_mut().transform.is_some());
    }

    #[test]
    fn undo_redo_and_fill_set_modified() {
        let mut app = App::default();
        app.apply_fill((1, 1));
        assert!(app.projects.current().is_dirty());
        // Undo of a real command restores the previous state.
        app.projects.current_mut().mark_saved();
        app.undo_document();
        assert!(app.projects.current().is_dirty());
        app.projects.current_mut().mark_saved();
        app.redo_document();
        assert!(app.projects.current().is_dirty());
    }

    #[test]
    fn create_new_canvas_resets_state() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 5, 5));
        app.projects.current_mut().mark_dirty();
        app.create_new_canvas(24, 32, 8);
        assert_eq!(app.projects.current_mut().layers.width(), 24);
        assert_eq!(app.projects.current_mut().layers.height(), 32);
        assert_eq!(app.projects.current_mut().layers.len(), 1);
        assert_eq!(app.projects.current().sequence.len(), 1);
        assert!(app.projects.current_mut().undo.can_undo() == false);
        assert_eq!(app.projects.current().name, "Untitled");
        assert!(!app.projects.current().is_dirty());
        assert_eq!(app.projects.current().tile_size, 8);
    }

    #[test]
    fn file_new_guards_unsaved_changes() {
        let mut app = App::default();
        app.projects.current_mut().mark_dirty();
        app.file_new();
        assert!(app.projects.current_mut().pending_action.is_some());
        assert!(!app.projects.current_mut().dialog_open);
        // An unmodified document opens the New dialog immediately.
        app.projects.current_mut().pending_action = None;
        app.projects.current_mut().mark_saved();
        app.file_new();
        assert!(app.projects.current_mut().pending_action.is_none());
        assert!(app.projects.current_mut().dialog_open);
    }

    #[test]
    fn save_to_clears_modified_and_sets_name() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.projects.current_mut().mark_dirty();
        let path = TempDir::new("f2_save_to");
        assert!(app.save_to(path.path().to_path_buf()));
        assert!(!app.projects.current().is_dirty());
        assert_eq!(
            app.projects.current().name,
            "pyx_task2_f2_save_to_".to_owned() + &std::process::id().to_string()
        );
    }

    #[test]
    fn failed_save_preserves_dirty_recovery_and_pending_state() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        let base = TempDir::new("failed_save_recovery");
        app.autosave_base = Some(base.path().to_path_buf());
        let project = app.projects.current().id;
        let journal_base = recovery_base_for(base.path(), project);
        write_recovery_journal(
            &journal_base,
            &RecoveryJournal {
                format_version: FORMAT_VERSION,
                project_dir: None,
                autosave_dir: PathBuf::from("failed-save-autosave"),
                project_name: "dirty".to_string(),
                saved_at: 1,
            },
        )
        .unwrap();
        app.projects.current_mut().recovery = read_recovery_journal(&journal_base);
        app.projects.current_mut().recovery_pending = true;
        app.projects.current_mut().pending_action = Some(PendingAction::Close { project });
        app.projects.current_mut().mark_dirty();
        let path = TempFile::new("failed_save_target", b"not a directory");

        assert!(!app.save_to(path.path().to_path_buf()));

        assert!(app.projects.current().is_dirty());
        assert!(app.projects.current().recovery.is_some());
        assert!(app.projects.current().recovery_pending);
        assert!(app.projects.current().pending_action.is_some());
        assert!(journal_base.join("recovery.json").exists());
        assert!(app
            .projects
            .current()
            .last_error
            .as_deref()
            .unwrap()
            .starts_with("Failed to save:"));
    }

    #[test]
    fn save_then_load_restores_modified_flag_and_name() {
        let mut source = App::default();
        seed_red_rect(&mut source, Rect2i::new(0, 0, 4, 4));
        let path = TempDir::new("f2_roundtrip");
        assert!(source.save_to(path.path().to_path_buf()));

        let mut app = App::default();
        app.load_project(path.path().to_path_buf());
        assert!(!app.projects.current().is_dirty());
        assert_eq!(
            app.projects.current().name,
            "pyx_task2_f2_roundtrip_".to_owned() + &std::process::id().to_string()
        );
        // The saved pixels came back.
        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .active_layer()
                .buffer
                .get_pixel(0, 0),
            Some(RED)
        );
    }

    #[test]
    fn execute_pending_close_discards_and_guard_cancel_preserves() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        let base = TempDir::new("close_recovery");
        app.autosave_base = Some(base.path().to_path_buf());
        app.projects.current_mut().mark_dirty();
        let project = app.projects.current().id;
        let journal_base = recovery_base_for(base.path(), project);
        write_recovery_journal(
            &journal_base,
            &RecoveryJournal {
                format_version: FORMAT_VERSION,
                project_dir: None,
                autosave_dir: PathBuf::from("stale-autosave"),
                project_name: "closing".to_string(),
                saved_at: 1,
            },
        )
        .unwrap();
        app.projects.current_mut().recovery = read_recovery_journal(&journal_base);
        app.projects.current_mut().recovery_pending = true;
        app.projects.current_mut().last_autosave = Some(Instant::now());
        app.projects.current_mut().autosave_generation = 3;
        app.projects.current_mut().dialog_open = true;
        app.projects.current_mut().pending_action = Some(PendingAction::Close { project });
        app.execute_pending();
        assert_eq!(app.projects.active_id(), None);
        assert!(!journal_base.join("recovery.json").exists());

        // Cancel keeps the document and drops the deferred action.
        let mut app = App::default();
        app.projects.current_mut().mark_saved();
        app.projects.current_mut().pending_action = Some(PendingAction::New {
            project: app.projects.current().id,
            width: 10,
            height: 10,
            tile: 2,
        });
        app.guard_cancel();
        assert!(app.projects.current_mut().pending_action.is_none());
        assert_eq!(app.projects.current_mut().layers.width(), CANVAS_WIDTH);
    }

    // -----------------------------------------------------------------------
    // R6 F3/F4: autosave + recovery journal
    // -----------------------------------------------------------------------

    #[test]
    fn default_app_has_no_autosave_state() {
        let app = App::default();
        assert!(app.autosave_base.is_none());
        assert!(app.projects.current().last_autosave.is_none());
        assert!(app.projects.current().recovery.is_none());
    }

    #[test]
    fn should_autosave_matrix() {
        let now = Instant::now();
        let interval = Duration::from_secs(300);
        // Not modified: never autosave.
        assert!(!should_autosave(false, None, now, interval));
        assert!(!should_autosave(false, Some(now), now, interval));
        // Elapsed below the interval: wait.
        assert!(!should_autosave(
            true,
            Some(now),
            now + interval - Duration::from_secs(1),
            interval
        ));
        // Elapsed at/above the interval with a modified doc: autosave.
        assert!(should_autosave(true, Some(now), now + interval, interval));
        assert!(should_autosave(
            true,
            Some(now),
            now + interval + Duration::from_secs(10),
            interval
        ));
        // No prior autosave + modified: autosave immediately.
        assert!(should_autosave(true, None, now, interval));
    }

    #[test]
    fn perform_autosave_writes_copy_and_keeps_modified() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.projects.current_mut().mark_dirty();
        app.projects.current_mut().name = "autosave test".to_string();
        let fake_path = std::env::temp_dir().join(format!("pyx_f3_fake_{}", std::process::id()));
        app.projects.current_mut().path = Some(fake_path.clone());
        let base = TempDir::new("f3_base");
        app.autosave_base = Some(base.path().to_path_buf());

        app.perform_autosave();

        let autosave_dir = autosave_dir_for(
            base.path(),
            &format!("{}-autosave test", app.projects.current().id.raw()),
        );
        assert!(autosave_dir.join("manifest.json").exists());
        assert_eq!(load_document(&autosave_dir).unwrap(), app.app_to_document());
        assert!(
            app.projects.current().is_dirty(),
            "autosave must not clear the dirty flag"
        );
        assert_eq!(
            app.projects.current().path,
            Some(fake_path.clone()),
            "autosave must not touch current_path"
        );
        assert!(app.projects.current().last_autosave.is_some());
        assert_eq!(app.projects.current().autosave_generation, 1);
        assert!(app.projects.current().recovery_pending);
        assert!(app.projects.current_mut().last_error.is_none());
        let journal_base = recovery_base_for(base.path(), app.projects.current().id);
        let journal = read_recovery_journal(&journal_base).unwrap();
        assert_eq!(journal.project_name, "autosave test");
        assert_eq!(journal.project_dir, Some(fake_path));
        assert_eq!(app.projects.current().recovery, Some(journal));
    }

    #[test]
    fn failed_autosave_preserves_recovery_state_after_prior_success() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.projects.current_mut().mark_dirty();
        let good_base = TempDir::new("f3_autosave_good");
        app.autosave_base = Some(good_base.path().to_path_buf());
        app.perform_autosave();
        let before = import_state(&app);
        let bad_base = TempFile::new("f3_autosave_bad", b"not a directory");
        app.autosave_base = Some(bad_base.path().to_path_buf());

        app.perform_autosave();

        let after = import_state(&app);
        assert_eq!(after.document, before.document);
        assert_eq!(after.dirty, before.dirty);
        assert_eq!(after.recovery_pending, before.recovery_pending);
        assert_eq!(after.autosave_generation, before.autosave_generation);
        assert_eq!(after.last_autosave, before.last_autosave);
        assert_eq!(after.recovery, before.recovery);
        assert!(app
            .projects
            .current()
            .last_error
            .as_deref()
            .unwrap()
            .starts_with("Autosave failed:"));
    }

    #[test]
    fn autosave_recovery_isolated_between_projects() {
        let mut app = App::default();
        let alpha = app.projects.active_id().expect("default project");
        let beta = app.new_project("beta", CANVAS_WIDTH, CANVAS_HEIGHT);
        let base = TempDir::new("f3_isolated");
        app.autosave_base = Some(base.path().to_path_buf());

        app.activate_project(alpha).expect("alpha is open");
        app.projects.current_mut().name = "alpha".to_string();
        app.projects.current_mut().mark_dirty();
        app.perform_autosave();
        let alpha_base = recovery_base_for(base.path(), alpha);
        assert!(read_recovery_journal(&alpha_base).is_some());

        app.activate_project(beta).expect("beta is open");
        app.projects.current_mut().mark_dirty();
        app.perform_autosave();
        let beta_base = recovery_base_for(base.path(), beta);
        assert!(read_recovery_journal(&beta_base).is_some());
        assert!(read_recovery_journal(&alpha_base).is_some());

        app.activate_project(alpha).expect("alpha is open");
        app.projects.current_mut().recovery = read_recovery_journal(&alpha_base);
        app.recovery_discard();
        assert!(read_recovery_journal(&alpha_base).is_none());
        assert!(read_recovery_journal(&beta_base).is_some());
    }

    #[test]
    fn save_to_clears_recovery_journal() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        let base = TempDir::new("f3_save_clear");
        app.autosave_base = Some(base.path().to_path_buf());
        let journal_base = recovery_base_for(base.path(), app.projects.current().id);
        write_recovery_journal(
            &journal_base,
            &RecoveryJournal {
                format_version: FORMAT_VERSION,
                project_dir: None,
                autosave_dir: PathBuf::from("/tmp/stale"),
                project_name: "stale".to_string(),
                saved_at: 1,
            },
        )
        .unwrap();
        assert!(journal_base.join("recovery.json").exists());
        app.projects.current_mut().recovery = read_recovery_journal(&journal_base);
        app.projects.current_mut().recovery_pending = true;
        app.projects.current_mut().last_autosave = Some(Instant::now());
        app.projects.current_mut().autosave_generation = 5;
        app.projects.current_mut().dialog_open = true;
        app.projects.current_mut().pending_action = Some(PendingAction::New {
            project: app.projects.current().id,
            width: 8,
            height: 8,
            tile: 1,
        });

        let path = TempDir::new("f3_save_to");
        assert!(app.save_to(path.path().to_path_buf()));
        assert!(!journal_base.join("recovery.json").exists());
        assert!(app.projects.current().recovery.is_none());
        assert!(!app.projects.current().recovery_pending);
        assert!(app.projects.current().last_autosave.is_none());
        assert_eq!(app.projects.current().autosave_generation, 0);
        assert!(!app.projects.current().dialog_open);
        assert!(app.projects.current().pending_action.is_none());
    }

    #[test]
    fn recovery_restore_loads_autosave_and_clears_journal() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        app.projects.current_mut().name = "crash project".to_string();
        let base = TempDir::new("f4_restore");
        app.autosave_base = Some(base.path().to_path_buf());
        let autosave_dir = autosave_dir_for(
            base.path(),
            &format!("{}-crash project", app.projects.current().id.raw()),
        );
        save_document(&app.app_to_document(), &autosave_dir).unwrap();
        let project_dir = std::env::temp_dir().join("pyx_f4_real_project");
        let journal_base = recovery_base_for(base.path(), app.projects.current().id);
        write_recovery_journal(
            &journal_base,
            &RecoveryJournal {
                format_version: FORMAT_VERSION,
                project_dir: Some(project_dir.clone()),
                autosave_dir: autosave_dir.clone(),
                project_name: "crash project".to_string(),
                saved_at: 1,
            },
        )
        .unwrap();
        app.projects.current_mut().recovery = read_recovery_journal(&journal_base);
        app.projects.current_mut().recovery_pending = true;
        app.projects.current_mut().last_autosave = Some(Instant::now());
        app.projects.current_mut().autosave_generation = 5;
        app.projects.current_mut().dialog_open = true;
        app.projects.current_mut().recovery_pending = true;
        app.projects.current_mut().last_autosave = Some(Instant::now());
        app.projects.current_mut().autosave_generation = 5;
        app.projects.current_mut().dialog_open = true;

        // Mutate the app so restore visibly replaces it.
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(0, 0, Color::TRANSPARENT);
        app.projects.current_mut().name = "Untitled".to_string();
        app.projects.current_mut().path = None;
        app.projects.current_mut().mark_saved();

        app.recovery_restore();

        assert_eq!(app.projects.current().path, Some(project_dir));
        assert_eq!(app.projects.current().name, "crash project");
        assert!(app.projects.current().is_dirty());
        assert!(app.projects.current_mut().last_error.is_none());
        assert!(app.projects.current().recovery.is_none());
        assert!(!app.projects.current().recovery_pending);
        assert!(app.projects.current().last_autosave.is_none());
        assert_eq!(app.projects.current().autosave_generation, 0);
        assert!(!app.projects.current().dialog_open);
        assert!(!journal_base.join("recovery.json").exists());
        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .active_layer()
                .buffer
                .get_pixel(0, 0),
            Some(RED)
        );
    }

    #[test]
    fn recovery_discard_deletes_journal_and_keeps_document() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        let base = TempDir::new("f4_discard");
        app.autosave_base = Some(base.path().to_path_buf());
        let journal_base = recovery_base_for(base.path(), app.projects.current().id);
        write_recovery_journal(
            &journal_base,
            &RecoveryJournal {
                format_version: FORMAT_VERSION,
                project_dir: None,
                autosave_dir: PathBuf::from("/tmp/stale"),
                project_name: "stale".to_string(),
                saved_at: 1,
            },
        )
        .unwrap();
        app.projects.current_mut().recovery = read_recovery_journal(&journal_base);
        app.projects.current_mut().recovery_pending = true;
        app.projects.current_mut().last_autosave = Some(Instant::now());
        app.projects.current_mut().autosave_generation = 5;
        app.projects.current_mut().dialog_open = true;
        let before = app
            .projects
            .current_mut()
            .layers
            .active_layer()
            .buffer
            .as_bytes()
            .to_vec();

        app.recovery_discard();

        assert!(app.projects.current().recovery.is_none());
        assert!(!app.projects.current().recovery_pending);
        assert!(app.projects.current().last_autosave.is_none());
        assert_eq!(app.projects.current().autosave_generation, 0);
        assert!(!app.projects.current().dialog_open);
        assert!(!journal_base.join("recovery.json").exists());
        assert_eq!(
            app.projects
                .current_mut()
                .layers
                .active_layer()
                .buffer
                .as_bytes(),
            &before[..]
        );
    }

    #[test]
    fn load_project_clears_recovery_journal() {
        let source = App::default();
        let path = TempDir::new("f4_load");
        save_document(&source.app_to_document(), path.path()).unwrap();

        let mut app = App::default();
        let base = TempDir::new("f4_load_clear");
        app.autosave_base = Some(base.path().to_path_buf());
        let next_project = ProjectId::new(app.projects.current().id.raw() + 1);
        let journal_base = recovery_base_for(base.path(), next_project);
        write_recovery_journal(
            &journal_base,
            &RecoveryJournal {
                format_version: FORMAT_VERSION,
                project_dir: None,
                autosave_dir: PathBuf::from("/tmp/stale"),
                project_name: "stale".to_string(),
                saved_at: 1,
            },
        )
        .unwrap();
        app.load_project(path.path().to_path_buf());
        assert!(!journal_base.join("recovery.json").exists());
        assert!(app.projects.current().recovery.is_none());
        assert!(!app.projects.current().recovery_pending);
        assert!(app.projects.current().last_autosave.is_none());
        assert_eq!(app.projects.current().autosave_generation, 0);
        assert!(!app.projects.current().dialog_open);
    }

    #[test]
    fn closing_last_project_leaves_main_frame_safe_to_redraw() {
        let mut app = App::default();
        let project = app.projects.current().id;
        app.close_project(project, project::CloseProject::Discard)
            .unwrap();
        assert_eq!(app.projects.active_id(), None);
        app.sync_texture();
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| app.ui_frame(ui));
        assert!(
            output.shapes.is_empty(),
            "canvas-only frame must not render shell UI without a project"
        );
        output.textures_delta.clear();
    }

    #[test]
    fn native_preview_image_uses_active_session_dimensions() {
        let app = App::default();
        let image = preview_image_for_session(app.projects.current());
        assert_eq!(image.size, [128, 128]);
    }

    #[test]
    fn native_render_errors_do_not_schedule_another_frame() {
        assert!(!native_render_succeeded(Err(())));
        assert!(native_render_succeeded(Ok(())));
    }

    #[test]
    fn opening_project_creates_one_tab_and_reuses_it() {
        let source = App::default();
        let path = TempDir::new("tab_reuse");
        save_document(&source.app_to_document(), path.path()).unwrap();

        let mut app = App::default();
        let initial = app.projects.tabs().len();
        app.load_project(path.path().to_path_buf());
        let project = app.projects.current().id;
        assert_eq!(app.projects.tabs().len(), initial + 1);
        assert_eq!(app.projects.current().path.as_deref(), Some(path.path()));

        app.load_project(path.path().to_path_buf());
        assert_eq!(app.projects.tabs().len(), initial + 1);
        assert_eq!(app.projects.current().id, project);
    }

    #[test]
    fn create_new_canvas_clears_recovery_journal() {
        let mut app = App::default();
        let base = TempDir::new("f4_new_clear");
        app.autosave_base = Some(base.path().to_path_buf());
        let journal_base = recovery_base_for(base.path(), app.projects.current().id);
        write_recovery_journal(
            &journal_base,
            &RecoveryJournal {
                format_version: FORMAT_VERSION,
                project_dir: None,
                autosave_dir: PathBuf::from("/tmp/stale"),
                project_name: "stale".to_string(),
                saved_at: 1,
            },
        )
        .unwrap();
        app.projects.current_mut().recovery = read_recovery_journal(&journal_base);
        app.projects.current_mut().recovery_pending = true;
        app.projects.current_mut().last_autosave = Some(Instant::now());
        app.projects.current_mut().autosave_generation = 5;
        app.projects.current_mut().dialog_open = true;

        app.create_new_canvas(16, 16, 8);
        assert!(!journal_base.join("recovery.json").exists());
        assert!(app.projects.current().recovery.is_none());
        assert!(!app.projects.current().recovery_pending);
        assert!(app.projects.current().last_autosave.is_none());
        assert_eq!(app.projects.current().autosave_generation, 0);
        assert!(!app.projects.current().dialog_open);
    }

    // -----------------------------------------------------------------------
    // R7: export suite + PNG import
    // -----------------------------------------------------------------------

    #[test]
    fn export_selected_composites_visible_layers() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 4, 4));
        marquee(&mut app, (0, 0), (3, 3));
        let (w, h, rgba) = app.export_selected_bytes().unwrap();
        assert_eq!((w, h), (4, 4));
        let img = decode_png(&encode_png(w, h, &rgba).unwrap()).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(img.rgba, rgba);
        for px in img.rgba.chunks_exact(4) {
            assert_eq!(px, &[255, 0, 0, 255]);
        }
        // No selection reports a clear error.
        let app = App::default();
        assert_eq!(
            app.export_selected_bytes().unwrap_err(),
            "No selection to export"
        );
    }

    #[test]
    fn file_import_png_creates_layer_region_frame() {
        let mut app = App::default();
        let layers_before = app.projects.current_mut().layers.len();
        let frames_before = app.projects.current().sequence.len();
        let path = TempFile::new(
            "png_success",
            &encode_png(8, 8, &vec![255u8; 8 * 8 * 4]).unwrap(),
        );
        app.file_import_png_path(path.path());
        assert!(app.projects.current().last_error.is_none());
        assert_eq!(app.projects.current_mut().layers.len(), layers_before + 1);
        assert_eq!(app.projects.current().sequence.len(), frames_before + 1);
        let layer = app.projects.current_mut().layers.active_layer();
        assert_eq!(layer.name, "Imported");
        assert_eq!(
            layer.buffer.get_pixel(0, 0),
            Some(Color::rgb(255, 255, 255))
        );
        assert_eq!(
            layer.buffer.get_pixel(7, 7),
            Some(Color::rgb(255, 255, 255))
        );
        let frame = app
            .projects
            .current()
            .sequence
            .frame(app.projects.current().sequence.len() - 1)
            .unwrap();
        assert_eq!(frame.region().rect(), Rect2i::new(0, 0, 8, 8));
        assert_eq!(frame.delay_ms(), 100);
        assert_eq!(
            app.projects
                .current_mut()
                .selection
                .as_ref()
                .unwrap()
                .rect(),
            Rect2i::new(0, 0, 8, 8)
        );
        assert!(
            app.projects.current().is_dirty(),
            "import must mark the document modified"
        );
        assert!(app.texture_dirty);
        assert_eq!(app.canvas_cache_token, None);
    }

    #[test]
    fn file_import_png_boundary_decode_failures_preserve_session() {
        let valid = encode_png(1, 1, &[255, 255, 255, 255]).unwrap();
        let mut truncated = valid.clone();
        truncated.truncate(truncated.len() / 2);
        let mut corrupt = valid.clone();
        let corrupt_index = corrupt.len() - 1;
        corrupt[corrupt_index] ^= 1;
        let cases = [
            ("malformed", b"not a png".to_vec()),
            ("truncated", truncated),
            ("corrupt", corrupt),
        ];

        for (name, bytes) in cases {
            let mut app = App::default();
            app.texture_dirty = false;
            app.canvas_cache_token = app.projects.activation_token();
            app.projects.current_mut().last_error = Some("previous error".to_string());
            let before = import_state(&app);
            let path = TempFile::new(&format!("png_{name}"), &bytes);

            app.file_import_png_path(path.path());

            assert_import_state_unchanged(&app, &before);
            assert!(app
                .projects
                .current()
                .last_error
                .as_deref()
                .unwrap()
                .starts_with("Failed to import PNG:"));
        }
    }

    #[test]
    fn file_import_png_boundary_empty_dimensions_preserve_session() {
        let mut app = App::default();
        app.new_project("empty canvas", 0, 0);
        app.texture_dirty = false;
        app.canvas_cache_token = app.projects.activation_token();
        app.projects.current_mut().last_error = Some("previous error".to_string());
        let before = import_state(&app);
        let path = TempFile::new(
            "png_empty",
            &encode_png(1, 1, &[255, 255, 255, 255]).unwrap(),
        );

        app.file_import_png_path(path.path());

        assert_import_state_unchanged(&app, &before);
        assert_eq!(
            app.projects.current().last_error.as_deref(),
            Some("Imported image is empty")
        );
    }

    #[test]
    fn file_import_png_boundary_overflow_dimensions_preserve_session() {
        let mut app = App::default();
        app.texture_dirty = false;
        app.canvas_cache_token = app.projects.activation_token();
        app.projects.current_mut().last_error = Some("previous error".to_string());
        let before = import_state(&app);
        let path = TempFile::new("png_overflow", &png_with_dimensions(u32::MAX, 2));

        app.file_import_png_path(path.path());

        assert_import_state_unchanged(&app, &before);
        assert!(app
            .projects
            .current()
            .last_error
            .as_deref()
            .unwrap()
            .starts_with("Failed to import PNG:"));
    }

    #[test]
    fn file_import_png_boundary_frame_limit_preserves_session() {
        let mut app = App::default();
        while app.projects.current().sequence.len() < MAX_FRAMES {
            assert!(app.projects.current_mut().sequence.push(Frame::new(
                Region::new(Rect2i::new(0, 0, 1, 1), "Limit"),
                100,
            )));
        }
        app.texture_dirty = false;
        app.canvas_cache_token = app.projects.activation_token();
        app.projects.current_mut().last_error = Some("previous error".to_string());
        let before = import_state(&app);
        let path = TempFile::new(
            "png_frame_limit",
            &encode_png(1, 1, &[255, 255, 255, 255]).unwrap(),
        );

        app.file_import_png_path(path.path());

        assert_import_state_unchanged(&app, &before);
        assert_eq!(
            app.projects.current().last_error.as_deref(),
            Some("Project frame limit reached")
        );
    }

    // -----------------------------------------------------------------------
    // Global-pointer tool gesture: a press outside the canvas arms the tool
    // -----------------------------------------------------------------------

    /// Build an App with a canvas texture uploaded so `ui_frame` renders the
    /// canvas and drives the global-pointer gesture machine.
    fn app_with_canvas(ctx: &egui::Context) -> App {
        let mut app = App::default();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [CANVAS_WIDTH, CANVAS_HEIGHT],
            &vec![0; CANVAS_WIDTH * CANVAS_HEIGHT * 4],
        );
        app.canvas_texture =
            Some(ctx.load_texture("canvas-gesture-test", image, egui::TextureOptions::NEAREST));
        app.texture_dirty = false;
        app
    }

    /// Run one full-app frame with the given pointer events. The canvas draw
    /// rect is (0,0)-(128,128) at 100% zoom with no pan.
    fn run_app_frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| app.ui_frame(ui));
        });
        output.textures_delta.clear();
    }

    fn press(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn release(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn press_shift(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers {
                shift: true,
                ..egui::Modifiers::NONE
            },
        }
    }

    fn press_alt(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers {
                alt: true,
                ..egui::Modifiers::NONE
            },
        }
    }

    fn modifiers_changed(modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::ModifiersChanged(modifiers)
    }

    fn move_to(pos: egui::Pos2) -> egui::Event {
        egui::Event::PointerMoved(pos)
    }

    #[test]
    /// Given a live Select drag, when the pointer moves outside the canvas, then the marquee keeps following, clamped to the far canvas edge.
    fn select_drag_keeps_extending_while_the_pointer_is_outside_the_canvas() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        let outside = egui::pos2(300.0, 200.0);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(80.0, 80.0))]);

        // The pointer leaves the canvas; the selection must follow to the edge
        // instead of freezing at the last in-canvas point.
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);

        let far = CANVAS_WIDTH as i32 - 1;
        assert_eq!(
            app.projects
                .current()
                .gesture
                .marquee()
                .map(|(_, rect)| rect),
            Some(Rect2i::new(60, 60, far - 59, far - 59)),
            "the marquee must clamp to the far canvas edge instead of freezing"
        );
    }

    #[test]
    /// Given a live draw-tool drag, when the pointer moves outside the canvas, then nothing is painted at the clamped edge.
    fn draw_tool_drag_does_not_paint_at_the_clamped_edge() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Pencil)]);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(20.0, 20.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(20.0, 20.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(40.0, 40.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(300.0, 300.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(300.0, 300.0))]);

        let far = (CANVAS_WIDTH as i32 - 1, CANVAS_HEIGHT as i32 - 1);
        let buffer = app.projects.current().layers.active_layer().buffer.clone();
        assert_eq!(
            buffer.get_pixel(far.0 as usize, far.1 as usize),
            Some(Color::TRANSPARENT),
            "a draw tool must stay paused outside the canvas, not follow the clamp"
        );
    }

    #[test]
    fn press_outside_without_entering_the_canvas_pushes_no_undo_step() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        let outside = egui::pos2(300.0, 200.0);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![press(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(400.0, 300.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(400.0, 300.0))]);
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        assert!(app.projects.current().stroke.is_none());
    }

    #[test]
    fn press_outside_then_enter_canvas_paints_from_the_entry_point() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let outside = egui::pos2(300.0, 200.0);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![press(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(50.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(60.0, 50.0))]);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(50, 50), Some(RED));
        assert_eq!(buf.get_pixel(60, 50), Some(RED));
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    fn leaving_and_re_entering_the_canvas_draws_no_connecting_line() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let outside = egui::pos2(300.0, 200.0);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![press(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(50.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(100.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(110.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(110.0, 50.0))]);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(50, 50), Some(RED));
        assert_eq!(buf.get_pixel(60, 50), Some(RED));
        assert_eq!(buf.get_pixel(100, 50), Some(RED));
        assert_eq!(buf.get_pixel(110, 50), Some(RED));
        for x in 61..100 {
            assert_eq!(
                buf.get_pixel(x, 50),
                Some(Color::TRANSPARENT),
                "pixel ({x},50) must stay untouched between the two segments"
            );
        }
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    /// L1-A: a live selection transform owns the gesture, so the Fieldier's
    /// clamped selection routing must NOT hijack it. The object must follow the
    /// CLAMP-FREE pointer out of the canvas and track it exactly on re-entry —
    /// no phantom start/end, no drift, no gesture drop.
    fn transform_drag_survives_leaving_and_reentering_the_canvas() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        // Fieldier is the natural tool while transforming a selection; its
        // clamped branch is exactly what corrupted the drag before the bypass.
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        app.lift_transform(Rect2i::new(20, 20, 60, 60)); // bbox (20,20)-(80,80)
        assert!(app.projects.current().transform.is_some());

        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(30.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(30.0, 60.0))]);
        // Cross egui's drag threshold inside the bbox to latch Translate.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(30.0, 68.0))]);

        // Leave the canvas: the object must follow the UNCLAMPED pointer.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(220.0, 220.0))]);
        assert!(
            app.projects.current().transform.is_some(),
            "leaving the canvas must not end the transform gesture"
        );
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            min_x > CANVAS_WIDTH as f32 && min_y > CANVAS_HEIGHT as f32,
            "the object must follow the pointer past the canvas edge (clamp-free), \
             got bbox min ({min_x},{min_y})"
        );

        // Re-enter and return to the press anchor: the gesture is still live and
        // tracks absolutely, proving no state corruption from the round trip.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(30.0, 60.0))]);
        assert!(app.projects.current().transform.is_some());
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            (min_x - 20.0).abs() < 1e-3 && (min_y - 20.0).abs() < 1e-3,
            "returning to the anchor must restore the start bbox (20,20), got ({min_x},{min_y})"
        );

        // Release only ends the drag; the session stays live for commit/cancel.
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(30.0, 60.0))]);
        assert!(
            app.projects.current().transform.is_some(),
            "releasing the pointer must only end the drag, not the session"
        );
    }

    // -----------------------------------------------------------------------
    // EK SORUN: transform drag OUTSIDE the canvas widget (global pointer)
    // -----------------------------------------------------------------------

    /// Lift a selection and park its gizmo fully OUTSIDE the 128×128 canvas
    /// draw rect but inside the surrounding letterbox: a 40×40 object at
    /// (150,150) has bbox (150,150)-(190,190) at 100% zoom. Pressing its
    /// interior therefore exercises the GLOBAL transform pointer path — the
    /// canvas widget's draw-rect-scoped `primary_response` never sees the
    /// press.
    fn lift_transform_outside_canvas(app: &mut App) {
        app.lift_transform(Rect2i::new(20, 20, 40, 40)); // bbox (20,20)-(60,60)
        let t = app
            .projects
            .current_mut()
            .transform
            .as_mut()
            .expect("a live transform session")
            .expect_selection_mut();
        t.object.pos = (150.0, 150.0);
    }

    /// (a) A press INSIDE the gizmo body that lies outside the canvas draw
    /// rect (letterbox) latches a Translate drag through the global path.
    #[test]
    fn press_on_gizmo_outside_canvas_latches_translate() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.ctx = ctx.clone();
        app.panel_dock_demo = false;
        lift_transform_outside_canvas(&mut app);
        assert_eq!(
            selection_bbox(&app),
            (150.0, 150.0, 190.0, 190.0),
            "fixture: the object's gizmo must sit fully outside the canvas"
        );

        // (170,170) is the bbox centre: outside the (0,0)-(128,128) draw rect
        // and inside the panel letterbox.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(170.0, 170.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(170.0, 170.0))]);

        assert_eq!(
            selection_drag(&app),
            GizmoHit::Translate,
            "a press on the gizmo body outside the canvas must latch Translate"
        );
        assert!(
            app.projects.current().transform.is_some(),
            "the session must stay live after the press"
        );
    }

    /// (b) With the button held, the drag continues while the pointer is
    /// OUTSIDE the canvas and brings the object back toward the canvas.
    #[test]
    fn gizmo_drag_outside_canvas_keeps_moving_the_object() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.ctx = ctx.clone();
        app.panel_dock_demo = false;
        lift_transform_outside_canvas(&mut app);

        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(170.0, 170.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(170.0, 170.0))]);

        // Drag to another letterbox point (still outside the canvas): the drag
        // must stay live and the object must follow the pointer.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(200.0, 200.0))]);
        assert_eq!(selection_drag(&app), GizmoHit::Translate);
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            min_x > CANVAS_WIDTH as f32 && min_y > CANVAS_HEIGHT as f32,
            "the object must follow the pointer while fully outside the canvas, got ({min_x},{min_y})"
        );

        // Then drag back toward the canvas: it must come in.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 60.0))]);
        assert_eq!(selection_drag(&app), GizmoHit::Translate);
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            min_x < CANVAS_WIDTH as f32 && min_y < CANVAS_HEIGHT as f32,
            "dragging toward the canvas must bring the object in, got ({min_x},{min_y})"
        );
    }

    /// (c) A global mouseup outside the canvas ends the drag; the session stays
    /// live for commit/cancel.
    #[test]
    fn global_mouseup_outside_canvas_ends_the_drag() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.ctx = ctx.clone();
        app.panel_dock_demo = false;
        lift_transform_outside_canvas(&mut app);

        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(170.0, 170.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(170.0, 170.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(200.0, 200.0))]);
        assert_eq!(selection_drag(&app), GizmoHit::Translate);

        // Release far outside the canvas AND outside the bbox.
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(400.0, 300.0))]);
        assert_eq!(
            selection_drag(&app),
            GizmoHit::None,
            "the global mouseup outside the canvas must drop the handle"
        );
        assert!(
            app.projects.current().transform.is_some(),
            "a release must only end the drag, never the session"
        );
    }

    /// (d) L1 canvas-exit/re-entry state preservation, now starting from a
    /// press OUTSIDE the canvas: returning to the press anchor restores the
    /// start bbox exactly (no drift, no phantom start/end).
    #[test]
    fn outside_press_drag_reentry_restores_the_start_bbox() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.ctx = ctx.clone();
        app.panel_dock_demo = false;
        lift_transform_outside_canvas(&mut app);

        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(170.0, 170.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(170.0, 170.0))]);
        // Further out, re-enter the canvas, then return to the press anchor.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(300.0, 300.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(170.0, 170.0))]);

        assert_eq!(
            selection_bbox(&app),
            (150.0, 150.0, 190.0, 190.0),
            "returning to the press anchor must restore the start bbox"
        );
        assert_eq!(selection_drag(&app), GizmoHit::Translate);
    }

    /// (e) Inside-canvas drag is unchanged: pressing and dragging inside the
    /// draw rect still tracks the pointer with the same absolute delta.
    #[test]
    fn inside_canvas_gizmo_drag_is_unchanged() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.ctx = ctx.clone();
        app.panel_dock_demo = false;
        app.lift_transform(Rect2i::new(20, 20, 40, 40)); // bbox (20,20)-(60,60)

        // Press the bbox centre (40,40) and drag +10 px right.
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(40.0, 40.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(40.0, 40.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(50.0, 40.0))]);

        assert_eq!(selection_drag(&app), GizmoHit::Translate);
        let (min_x, min_y, _, _) = selection_bbox(&app);
        assert!(
            (min_x - 30.0).abs() < 1e-3 && (min_y - 20.0).abs() < 1e-3,
            "an inside-canvas drag must move by the pointer delta, got ({min_x},{min_y})"
        );

        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(50.0, 40.0))]);
        assert_eq!(selection_drag(&app), GizmoHit::None);
    }

    #[test]
    /// L1-A guard: without a transform the Fieldier selection clamp is
    /// unchanged — a drag past the canvas edge keeps extending at the far edge.
    fn fieldier_selection_drag_still_clamps_without_a_transform() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        assert!(app.projects.current().transform.is_none());

        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(80.0, 80.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(400.0, 400.0))]);

        let far = CANVAS_WIDTH as i32 - 1;
        assert_eq!(
            app.projects
                .current()
                .gesture
                .marquee()
                .map(|(_, rect)| rect),
            Some(Rect2i::new(60, 60, far - 59, far - 59)),
            "the Fieldier marquee must still clamp to the far canvas edge"
        );
    }

    #[test]
    fn press_on_a_ui_widget_does_not_arm_the_tool() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let button_rect =
            egui::Rect::from_min_size(egui::pos2(300.0, 300.0), egui::vec2(60.0, 24.0));
        let button_center = button_rect.center();
        let frame = |app: &mut App, events: Vec<egui::Event>| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(800.0, 600.0),
                )),
                predicted_dt: 1.0 / 60.0,
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        app.ui_frame(ui);
                        ui.put(button_rect, egui::Button::new("Test"));
                    });
            });
            output.textures_delta.clear();
        };
        frame(&mut app, vec![move_to(button_center)]);
        frame(&mut app, vec![press(button_center)]);
        frame(&mut app, vec![move_to(egui::pos2(50.0, 50.0))]);
        frame(&mut app, vec![release(egui::pos2(50.0, 50.0))]);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(50, 50), Some(Color::TRANSPARENT));
        assert_eq!(app.projects.current().undo.undo_len(), 0);
    }

    #[test]
    fn fill_and_eyedropper_act_once_per_canvas_entry() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        // A WHITE barrier at x=16 splits the canvas so the two fills target
        // distinct regions and both produce an undo step.
        for y in 0..CANVAS_HEIGHT {
            app.projects
                .current_mut()
                .layers
                .active_layer_mut()
                .buffer
                .set_pixel(16, y, Color::WHITE);
        }
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fill)]);
        let outside = egui::pos2(300.0, 200.0);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![press(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(8.0, 8.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(20.0, 20.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(20.0, 20.0))]);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(8, 8), Some(RED));
        assert_eq!(buf.get_pixel(20, 20), Some(RED));
        assert_eq!(app.projects.current().undo.undo_len(), 2);

        // Eyedropper: samples once per canvas entry.
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(30, 30, RED);
        app.projects
            .current_mut()
            .layers
            .active_layer_mut()
            .buffer
            .set_pixel(40, 40, Color::rgb(0, 255, 0));
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Eyedropper)]);
        app.projects.current_mut().color = Color::WHITE;
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![press(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(30.0, 30.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(40.0, 40.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(40.0, 40.0))]);
        assert_eq!(app.projects.current().color, Color::rgb(0, 255, 0));
    }

    #[test]
    /// Given a Select drag that left and re-entered the canvas, when it is released, then the selection keeps its original origin instead of re-anchoring.
    fn select_drag_keeps_its_origin_after_leaving_and_re_entering_the_canvas() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fieldier)]);
        let outside = egui::pos2(300.0, 200.0);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![press(egui::pos2(60.0, 60.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(65.0, 65.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(68.0, 68.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(70.0, 70.0))]);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(70.0, 70.0))]);
        assert_eq!(
            app.projects.current().selection.as_ref().map(|s| s.rect()),
            Some(Rect2i::new(60, 60, 11, 11))
        );
    }

    #[test]
    fn release_outside_the_canvas_still_commits_the_stroke() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let outside = egui::pos2(300.0, 200.0);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![press(outside)]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(50.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(egui::pos2(60.0, 50.0))]);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);
        run_app_frame(&mut app, &ctx, vec![release(outside)]);
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(50, 50), Some(RED));
        assert_eq!(buf.get_pixel(60, 50), Some(RED));
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    fn press_paints_one_stamp_without_moving() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        app.projects
            .current_mut()
            .draw_settings_mut(Tool::Pencil)
            .size = 3;
        let at = egui::pos2(50.0, 50.0);

        // Given: the pointer over the canvas. When: the primary button is
        // pressed (no movement at all).
        run_app_frame(&mut app, &ctx, vec![move_to(at)]);
        run_app_frame(&mut app, &ctx, vec![press(at)]);

        // Then: the press frame already stamped the size-3 footprint.
        {
            let buf = &app.projects.current().layers.active_layer().buffer;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (x, y) = (50 + dx, 50 + dy);
                    assert_eq!(
                        buf.get_pixel(x as usize, y as usize),
                        Some(RED),
                        "press must stamp ({x},{y})"
                    );
                }
            }
            for (x, y) in [(48, 50), (52, 50), (50, 48), (50, 52)] {
                assert_eq!(
                    buf.get_pixel(x as usize, y as usize),
                    Some(Color::TRANSPARENT),
                    "({x},{y}) lies outside the footprint"
                );
            }
        }

        // A held button without movement re-stamps the same footprint only.
        run_app_frame(&mut app, &ctx, Vec::new());
        {
            let buf = &app.projects.current().layers.active_layer().buffer;
            assert_eq!(buf.get_pixel(50, 50), Some(RED));
            assert_eq!(buf.get_pixel(52, 50), Some(Color::TRANSPARENT));
        }

        // When: the button is released. Then: one stroke, one undo step.
        run_app_frame(&mut app, &ctx, vec![release(at)]);
        assert!(app.projects.current().stroke.is_none());
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    fn shift_starts_a_line_gesture_without_painting_during_the_drag() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(40.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_shift(start)]);

        // Then: line mode is latched at the press, with the anchor recorded and
        // nothing painted.
        assert!(app.line_mode, "Shift+press must latch line mode");
        assert_eq!(app.line_anchor, Some((40, 64)));
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(40, 64),
            Some(Color::TRANSPARENT),
            "the press must not paint in line mode"
        );

        // When: the pointer drags to the end. Then: only the preview moves.
        let end = egui::pos2(100.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        assert_eq!(app.line_end, Some((100, 64)));
        {
            let buf = &app.projects.current().layers.active_layer().buffer;
            for x in 40..=100 {
                assert_eq!(
                    buf.get_pixel(x, 64),
                    Some(Color::TRANSPARENT),
                    "the canvas must not change mid-line at ({x},64)"
                );
            }
        }
        assert!(app.projects.current().stroke.is_none());
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        run_app_frame(&mut app, &ctx, vec![release(end)]);
    }

    #[test]
    fn releasing_the_line_commits_one_undoable_stroke() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(40.0, 64.0);
        let end = egui::pos2(100.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_shift(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);

        // Then: exactly one undo step covering the whole segment.
        assert_eq!(app.projects.current().undo.undo_len(), 1);
        assert!(!app.line_mode, "the latch is cleared on release");
        {
            let buf = &app.projects.current().layers.active_layer().buffer;
            for x in 40..=100 {
                assert_eq!(buf.get_pixel(x, 64), Some(RED), "line pixel ({x},64)");
            }
            assert_eq!(buf.get_pixel(39, 64), Some(Color::TRANSPARENT));
            assert_eq!(buf.get_pixel(101, 64), Some(Color::TRANSPARENT));
        }

        // And: a single undo clears the whole line.
        app.undo_document();
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(40, 64), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(100, 64), Some(Color::TRANSPARENT));
        assert_eq!(app.projects.current().undo.undo_len(), 0);
    }

    #[test]
    fn line_thickness_and_shape_match_the_draw_settings() {
        let ctx = egui::Context::default();
        let apply_settings = |app: &mut App| {
            let settings = app.projects.current_mut().draw_settings_mut(Tool::Pencil);
            settings.size = 5;
            settings.shape = BrushShape::Round;
        };
        let start = egui::pos2(40.0, 64.0);
        let end = egui::pos2(100.0, 64.0);

        // A normal press-stamp drag.
        let mut normal = app_with_canvas(&ctx);
        normal.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        apply_settings(&mut normal);
        run_app_frame(&mut normal, &ctx, vec![move_to(start)]);
        run_app_frame(&mut normal, &ctx, vec![press(start)]);
        run_app_frame(&mut normal, &ctx, vec![move_to(end)]);
        run_app_frame(&mut normal, &ctx, vec![release(end)]);

        // The same segment as a Shift+press line.
        let mut line = app_with_canvas(&ctx);
        line.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        apply_settings(&mut line);
        run_app_frame(&mut line, &ctx, vec![move_to(start)]);
        run_app_frame(&mut line, &ctx, vec![press_shift(start)]);
        run_app_frame(&mut line, &ctx, vec![move_to(end)]);
        run_app_frame(&mut line, &ctx, vec![release(end)]);

        let normal_buf = &normal.projects.current().layers.active_layer().buffer;
        let line_buf = &line.projects.current().layers.active_layer().buffer;
        for y in 0..CANVAS_HEIGHT {
            for x in 0..CANVAS_WIDTH {
                assert_eq!(
                    line_buf.get_pixel(x, y),
                    normal_buf.get_pixel(x, y),
                    "({x},{y}) must match a normal stroke of the same settings"
                );
            }
        }
        // The round size-5 footprint reaches 2 px up but not 3.
        assert_eq!(line_buf.get_pixel(40, 64), Some(RED));
        assert_eq!(line_buf.get_pixel(40, 62), Some(RED));
        assert_eq!(line_buf.get_pixel(40, 61), Some(Color::TRANSPARENT));
    }

    #[test]
    fn releasing_shift_mid_gesture_keeps_line_mode() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(40.0, 64.0);
        let mid = egui::pos2(70.0, 64.0);
        let end = egui::pos2(100.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_shift(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(mid)]);
        // Release Shift mid-gesture.
        run_app_frame(
            &mut app,
            &ctx,
            vec![modifiers_changed(egui::Modifiers::NONE), move_to(end)],
        );

        assert!(
            app.line_mode,
            "line mode must stay latched after Shift is released"
        );
        assert_eq!(app.line_end, Some((100, 64)));
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(100, 64),
            Some(Color::TRANSPARENT),
            "no paint until the button is released"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 0);

        run_app_frame(&mut app, &ctx, vec![release(end)]);
        assert!(!app.line_mode);
        let buf = &app.projects.current().layers.active_layer().buffer;
        for x in 40..=100 {
            assert_eq!(
                buf.get_pixel(x, 64),
                Some(RED),
                "line pixel ({x},64) after Shift release"
            );
        }
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    fn ctrl_snaps_the_line_angle_to_22_5_degrees() {
        let start = egui::pos2(60.0, 64.0);
        // dx = 14, dy = 15 → 47.0°, the nearest 22.5° multiple is 45°.
        let near45 = egui::pos2(74.0, 79.0);

        let ctx = egui::Context::default();
        let mut held = app_with_canvas(&ctx);
        run_app_frame(&mut held, &ctx, vec![move_to(start)]);
        run_app_frame(&mut held, &ctx, vec![press_shift(start)]);
        run_app_frame(
            &mut held,
            &ctx,
            vec![
                modifiers_changed(egui::Modifiers {
                    ctrl: true,
                    ..egui::Modifiers::NONE
                }),
                move_to(near45),
            ],
        );
        let snapped = held
            .line_end
            .expect("the line end is tracked while dragging");
        assert_eq!(
            snapped,
            (75, 79),
            "a near-45° drag must snap exactly onto 45°"
        );
        assert_eq!(
            snapped.0 - 60,
            snapped.1 - 64,
            "the snapped ray has equal offsets"
        );
        run_app_frame(&mut held, &ctx, vec![release(near45)]);

        // Without Ctrl the same drag keeps its free angle.
        let free_ctx = egui::Context::default();
        let mut free = app_with_canvas(&free_ctx);
        run_app_frame(&mut free, &free_ctx, vec![move_to(start)]);
        run_app_frame(&mut free, &free_ctx, vec![press_shift(start)]);
        run_app_frame(&mut free, &free_ctx, vec![move_to(near45)]);
        assert_eq!(
            free.line_end,
            Some((74, 79)),
            "without Ctrl the free angle is kept"
        );
        run_app_frame(&mut free, &free_ctx, vec![release(near45)]);
    }

    #[test]
    fn line_mode_respects_scatter() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        {
            let settings = app.projects.current_mut().draw_settings_mut(Tool::Pencil);
            settings.size = 1;
            settings.shape = BrushShape::Square;
            settings.scatter = 4;
        }
        let start = egui::pos2(30.0, 64.0);
        let end = egui::pos2(70.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_shift(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);

        let buf = &app.projects.current().layers.active_layer().buffer;
        let mut off_line = 0;
        for x in 0..CANVAS_WIDTH {
            for y in 0..CANVAS_HEIGHT {
                if buf.get_pixel(x, y) == Some(RED) {
                    assert!(
                        (26..=74).contains(&x) && (60..=68).contains(&y),
                        "({x},{y}) lies outside the scatter band around the line"
                    );
                    if y != 64 {
                        off_line += 1;
                    }
                }
            }
        }
        assert!(
            off_line > 0,
            "scatter must displace stamps off the exact line"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    fn line_mode_applies_the_tail_taper() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        {
            let settings = app.projects.current_mut().draw_settings_mut(Tool::Pencil);
            settings.size = 16;
            settings.shape = BrushShape::Square;
            settings.tail = -50;
        }
        let start = egui::pos2(16.0, 64.0);
        let end = egui::pos2(116.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_shift(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);

        let buf = &app.projects.current().layers.active_layer().buffer;
        let column_height = |x: usize| {
            (0..buf.height())
                .filter(|&y| buf.get_pixel(x, y) == Some(RED))
                .count()
        };
        let start_h = column_height(16);
        let end_h = column_height(116);
        assert_eq!(start_h, 16, "the line starts at the base size");
        assert!(
            end_h < start_h,
            "the stepped tail shrinks toward the end: {start_h} -> {end_h}"
        );
        assert!(end_h >= 1, "the taper stays at least 1 px");
    }

    #[test]
    fn normal_mode_is_unchanged_without_shift() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(60.0, 64.0);
        let end = egui::pos2(80.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press(start)]);

        // Then: a plain press still stamps immediately and starts a stroke.
        assert!(!app.line_mode, "a plain press must not latch line mode");
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(60, 64),
            Some(RED),
            "the normal press-stamps-immediately rule is preserved"
        );
        assert!(
            app.projects.current().stroke.is_some(),
            "the stroke is in flight"
        );

        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);
        let buf = &app.projects.current().layers.active_layer().buffer;
        for x in 60..=80 {
            assert_eq!(
                buf.get_pixel(x, 64),
                Some(RED),
                "normal drag pixel ({x},64)"
            );
        }
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    fn alt_starts_a_dynamic_line_gesture_without_painting() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(40.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_alt(start)]);

        assert!(app.line_mode, "Alt+press must latch a line gesture");
        assert!(app.line_dynamic, "the latch must be the dynamic variant");
        assert_eq!(app.line_anchor, Some((40, 64)));
        assert!(app.projects.current().transform.is_none());
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(40, 64),
            Some(Color::TRANSPARENT),
            "the press must not paint in dynamic line mode"
        );

        // Releasing Alt mid-gesture must not cancel the latched gesture.
        run_app_frame(
            &mut app,
            &ctx,
            vec![
                modifiers_changed(egui::Modifiers::NONE),
                move_to(egui::pos2(100.0, 64.0)),
            ],
        );
        assert!(
            app.line_mode,
            "the dynamic latch must survive an Alt release"
        );
        assert_eq!(app.line_end, Some((100, 64)));
        assert!(app.projects.current().transform.is_none());
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        run_app_frame(&mut app, &ctx, vec![release(egui::pos2(100.0, 64.0))]);
    }

    #[test]
    fn releasing_alt_line_creates_a_transform_object_with_four_gizmos() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        let start = egui::pos2(40.0, 64.0);
        let end = egui::pos2(90.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_alt(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);

        assert!(!app.line_mode, "the latch clears on release");
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "no canvas write on release"
        );
        let curve = app
            .projects
            .current()
            .transform
            .as_ref()
            .and_then(TransformSession::curve)
            .expect("the Alt line must become a transform object");
        let expected: Vec<(f32, f32)> = (1..=4)
            .map(|part| (40.0 + 50.0 * part as f32 / 5.0, 64.0))
            .collect();
        assert_eq!(
            curve.gizmos().to_vec(),
            expected,
            "the four gizmos must split the line into five equal parts"
        );
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(40, 64),
            Some(Color::TRANSPARENT),
            "nothing is written until the object is committed"
        );
    }

    #[test]
    fn dragging_a_gizmo_bends_the_curve() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        let start = egui::pos2(40.0, 64.0);
        let end = egui::pos2(90.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_alt(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);

        // The first gizmo sits at 1/5 of the line: (50, 64).
        let gizmo = egui::pos2(50.0, 64.0);
        let moved = egui::pos2(50.0, 40.0);
        run_app_frame(&mut app, &ctx, vec![move_to(gizmo)]);
        run_app_frame(&mut app, &ctx, vec![press(gizmo)]);
        run_app_frame(&mut app, &ctx, vec![move_to(moved)]);
        run_app_frame(&mut app, &ctx, vec![release(moved)]);

        let curve = app
            .projects
            .current()
            .transform
            .as_ref()
            .and_then(TransformSession::curve)
            .expect("dragging a gizmo keeps the curve session");
        assert_eq!(
            curve.gizmo(0),
            Some((50.0, 40.0)),
            "the gizmo must follow the drag"
        );
        assert!(
            curve.samples().iter().any(|&(_, y)| y <= 44),
            "the spline must bend through the moved control point"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "a gizmo drag must not paint"
        );
    }

    #[test]
    fn double_click_on_empty_area_commits_one_undoable_stroke() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.panel_dock_demo = false;
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(40.0, 64.0);
        let end = egui::pos2(90.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_alt(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);
        assert!(app.projects.current().transform.is_some());

        // Two clicks far from the curve (empty area) form a double-click.
        let empty = egui::pos2(40.0, 110.0);
        let mut clock = ctx.input(|i| i.time);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(empty)]);
        for _ in 0..2 {
            run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(empty)]);
            run_app_frame_animated(&mut app, &ctx, &mut clock, vec![release(empty)]);
        }

        assert!(
            app.projects.current().transform.is_none(),
            "the commit must clear the object"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            1,
            "the curve lands as one undo step"
        );
        let buf = &app.projects.current().layers.active_layer().buffer;
        for x in 40..=90 {
            assert_eq!(buf.get_pixel(x, 64), Some(RED), "curve pixel ({x},64)");
        }
    }

    #[test]
    fn double_click_on_a_gizmo_does_not_commit() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.panel_dock_demo = false;
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(40.0, 64.0);
        let end = egui::pos2(90.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_alt(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);

        // The second gizmo is at 2/5 of the line: (60, 64).
        let gizmo = egui::pos2(60.0, 64.0);
        let mut clock = ctx.input(|i| i.time);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(gizmo)]);
        for _ in 0..2 {
            run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(gizmo)]);
            run_app_frame_animated(&mut app, &ctx, &mut clock, vec![release(gizmo)]);
        }

        assert!(
            app.projects.current().transform.is_some(),
            "a double-click on a gizmo must not commit the object"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 0);
    }

    #[test]
    fn shift_line_mode_still_commits_directly() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let start = egui::pos2(40.0, 64.0);
        let end = egui::pos2(100.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press_shift(start)]);
        run_app_frame(&mut app, &ctx, vec![move_to(end)]);
        run_app_frame(&mut app, &ctx, vec![release(end)]);

        assert!(!app.line_mode);
        assert!(
            app.projects.current().transform.is_none(),
            "Shift-line must not create a transform object"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 1);
        let buf = &app.projects.current().layers.active_layer().buffer;
        for x in 40..=100 {
            assert_eq!(buf.get_pixel(x, 64), Some(RED), "line pixel ({x},64)");
        }
    }

    #[test]
    fn brush_keeps_painting_while_its_footprint_overlaps_the_canvas() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        {
            let settings = app.projects.current_mut().draw_settings_mut(Tool::Pencil);
            settings.size = 16;
            settings.shape = BrushShape::Square;
        }
        let start = egui::pos2(10.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(start)]);
        run_app_frame(&mut app, &ctx, vec![press(start)]);

        // When: the cursor moves past the right canvas edge (x = 128). The
        // size-16 footprint (125..=140) still overlaps the canvas, so the
        // stroke must continue and paint its in-bounds part.
        let outside = egui::pos2(133.0, 64.0);
        run_app_frame(&mut app, &ctx, vec![move_to(outside)]);

        // Then: the whole run to the canvas edge is painted — no gap.
        {
            let buf = &app.projects.current().layers.active_layer().buffer;
            for x in 10..CANVAS_WIDTH {
                assert_eq!(buf.get_pixel(x, 64), Some(RED), "no gap at ({x},64)");
            }
            // The press footprint starts at x = 2; nothing reaches x = 0..=1.
            assert_eq!(buf.get_pixel(0, 64), Some(Color::TRANSPARENT));
            assert_eq!(buf.get_pixel(1, 64), Some(Color::TRANSPARENT));
        }

        run_app_frame(&mut app, &ctx, vec![release(outside)]);
        assert_eq!(app.projects.current().undo.undo_len(), 1);
    }

    #[test]
    fn alt_wheel_changes_scatter_without_zooming_or_resizing() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Pencil);
        {
            let settings = app.projects.current_mut().draw_settings_mut(Tool::Pencil);
            settings.size = 7;
            settings.shape = BrushShape::Round;
            settings.scatter = 8;
        }

        app.handle_interactions(CanvasInteractions {
            scatter_scroll: 40.0,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).scatter,
            9,
            "one Alt+scroll notch up adds one scatter step"
        );
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).size,
            7,
            "Alt+scroll must not resize the brush"
        );
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).shape,
            BrushShape::Round,
            "Alt+scroll must not cycle the brush shape"
        );
        assert_eq!(
            app.projects.current().camera.zoom_percent(),
            100,
            "Alt+scroll must not zoom"
        );

        app.handle_interactions(CanvasInteractions {
            scatter_scroll: -40.0,
            ..Default::default()
        });
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).scatter,
            8,
            "one Alt+scroll notch down removes one scatter step"
        );

        for _ in 0..100 {
            app.handle_interactions(CanvasInteractions {
                scatter_scroll: 40.0,
                ..Default::default()
            });
        }
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).scatter,
            DrawTool::MAX_SCATTER,
            "scatter grows clamp at MAX_SCATTER"
        );

        for _ in 0..100 {
            app.handle_interactions(CanvasInteractions {
                scatter_scroll: -40.0,
                ..Default::default()
            });
        }
        assert_eq!(
            app.projects.current().draw_settings(Tool::Pencil).scatter,
            0,
            "scatter shrinks clamp at 0"
        );
    }

    #[test]
    fn alt_wheel_is_ignored_by_non_draw_tools() {
        let mut app = App::default();
        app.projects
            .current_mut()
            .tool_state
            .select_tool(Tool::Fill);
        for tool in [Tool::Pencil, Tool::Eraser, Tool::Draw] {
            app.projects.current_mut().draw_settings_mut(tool).scatter = 5;
        }

        app.handle_interactions(CanvasInteractions {
            scatter_scroll: 40.0,
            ..Default::default()
        });

        for tool in [Tool::Pencil, Tool::Eraser, Tool::Draw] {
            assert_eq!(
                app.projects.current().draw_settings(tool).scatter,
                5,
                "{tool:?} must keep its scatter while a non-draw tool is active"
            );
        }
        assert_eq!(
            app.projects.current().camera.zoom_percent(),
            100,
            "an ignored Alt+scroll must not zoom either"
        );
    }

    /// Run one full-app frame on the App's OWN egui context, mirroring
    /// `App::redraw`. Keyboard shortcuts read `self.ctx`, so a frame driven
    /// through any other context would never see the synthetic key events.
    fn run_own_ctx_frame(app: &mut App, events: Vec<egui::Event>) {
        let ctx = app.ctx.clone();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| app.ui_frame(ui));
        });
        output.textures_delta.clear();
    }

    fn app_with_own_canvas() -> App {
        let mut app = App::default();
        let ctx = app.ctx.clone();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [CANVAS_WIDTH, CANVAS_HEIGHT],
            &vec![0; CANVAS_WIDTH * CANVAS_HEIGHT * 4],
        );
        app.canvas_texture =
            Some(ctx.load_texture("canvas-keyboard-test", image, egui::TextureOptions::NEAREST));
        app.texture_dirty = false;
        app
    }

    fn paint_red_pixel(app: &mut App) {
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let outside = egui::pos2(300.0, 200.0);
        run_own_ctx_frame(app, vec![move_to(outside)]);
        run_own_ctx_frame(app, vec![press(outside)]);
        run_own_ctx_frame(app, vec![move_to(egui::pos2(50.0, 50.0))]);
        run_own_ctx_frame(app, vec![release(egui::pos2(50.0, 50.0))]);
    }

    /// Send a Ctrl-modified key as winit reports it on non-macOS: the
    /// `ModifiersChanged` event sets BOTH `ctrl` and `command`, then the key
    /// press carries the same modifiers.
    fn ctrl_key_frame(app: &mut App, key: egui::Key) {
        let modifiers = egui::Modifiers {
            ctrl: true,
            command: true,
            ..egui::Modifiers::NONE
        };
        run_own_ctx_frame(
            app,
            vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
        );
    }

    /// Send a key with no modifiers, as winit reports it for a bare key press.
    fn plain_key_frame(app: &mut App, key: egui::Key) {
        run_own_ctx_frame(
            app,
            vec![
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }

    /// Drain the per-layer dirty rects and report whether any was marked,
    /// i.e. whether the canvas would re-upload on the next `sync_texture`.
    fn canvas_dirty(app: &mut App) -> bool {
        app.projects
            .current_mut()
            .layers
            .iter_mut()
            .any(|layer| layer.buffer.take_pixels_changed().is_some())
    }

    #[test]
    fn ctrl_z_undoes_the_last_action_end_to_end() {
        let mut app = app_with_own_canvas();
        paint_red_pixel(&mut app);
        assert_eq!(app.projects.current().undo.undo_len(), 1);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(50, 50),
            Some(RED)
        );
        // Drain the stroke's dirty rect so the post-undo signal is meaningful.
        let _ = canvas_dirty(&mut app);

        ctrl_key_frame(&mut app, egui::Key::Z);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(50, 50),
            Some(Color::TRANSPARENT),
            "Ctrl+Z must revert the painted pixel"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        assert_eq!(app.projects.current().undo.redo_len(), 1);
        assert!(
            canvas_dirty(&mut app),
            "undo must mark the canvas dirty so the texture re-uploads"
        );
    }

    #[test]
    fn ctrl_y_redoes_it_end_to_end() {
        let mut app = app_with_own_canvas();
        paint_red_pixel(&mut app);
        ctrl_key_frame(&mut app, egui::Key::Z);
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(50, 50),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(app.projects.current().undo.redo_len(), 1);
        // Drain the undo's dirty rect so the post-redo signal is meaningful.
        let _ = canvas_dirty(&mut app);

        ctrl_key_frame(&mut app, egui::Key::Y);

        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(50, 50),
            Some(RED),
            "Ctrl+Y must re-apply the undone pixel"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 1);
        assert_eq!(app.projects.current().undo.redo_len(), 0);
        assert!(
            canvas_dirty(&mut app),
            "redo must mark the canvas dirty so the texture re-uploads"
        );
    }
    // Selection clipping: every pixel-writing tool honours the selection
    // -----------------------------------------------------------------------

    fn install_masked_selection(app: &mut App, rect: Rect2i, mask: Vec<bool>) {
        let selection = Selection::capture_mask(
            &app.projects.current_mut().layers.active_layer().buffer,
            rect,
            mask,
        )
        .expect("an in-bounds masked selection");
        app.projects.current_mut().selection = Some(selection);
    }

    /// Every canvas pixel currently holding `color`, in `(x, y)` order.
    fn pixels_holding(app: &App, color: Color) -> Vec<(i32, i32)> {
        let buffer = &app.projects.current().layers.active_layer().buffer;
        (0..buffer.width() as i32)
            .flat_map(|x| (0..buffer.height() as i32).map(move |y| (x, y)))
            .filter(|&(x, y)| buffer.get_pixel(x as usize, y as usize) == Some(color))
            .collect()
    }

    /// Drag the active stroke tool along the diagonal from `a` to `b`.
    fn drag_stroke(app: &mut App, a: (i32, i32), b: (i32, i32)) {
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some(a),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some(b),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some(b),
            ..Default::default()
        });
    }

    /// Latch a LINE gesture over the diagonal and release it.
    fn release_line_gesture(app: &mut App, a: (i32, i32), b: (i32, i32), dynamic: bool) {
        app.line_mode = true;
        app.line_dynamic = dynamic;
        app.line_anchor = Some(a);
        app.line_end = Some(b);
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some(b),
            ..Default::default()
        });
    }

    #[test]
    /// Given a masked selection, when the pen strokes a diagonal through and
    /// past it, then only the cells the mask contains are written.
    fn pen_stroke_is_clipped_to_the_committed_mask() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        let mut mask = vec![true; 16];
        mask[5] = false;
        install_masked_selection(&mut app, Rect2i::new(4, 4, 4, 4), mask);

        drag_stroke(&mut app, (2, 2), (9, 9));

        assert_eq!(
            pixels_holding(&app, RED),
            vec![(4, 4), (6, 6), (7, 7)],
            "the pen must write only the selected cells: (5,5) is masked out and the \
             path outside the rect must stay transparent"
        );
    }

    #[test]
    /// Given a selection over a red field, when the eraser strokes through and
    /// past it, then pixels outside the selection keep their color.
    fn eraser_stroke_is_clipped_to_the_committed_mask() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(0, 0, 16, 16));
        marquee(&mut app, (4, 4), (7, 7));
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Eraser)]);

        drag_stroke(&mut app, (2, 2), (9, 9));

        let buffer = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(
            buffer.get_pixel(5, 5),
            Some(Color::TRANSPARENT),
            "the selected cell is erased"
        );
        assert_eq!(buffer.get_pixel(2, 2), Some(RED), "before the selection");
        assert_eq!(buffer.get_pixel(9, 9), Some(RED), "after the selection");
    }

    #[test]
    /// Given a selection, when a Shift+LINE gesture commits a diagonal across
    /// it, then only the in-selection cells are painted.
    fn line_commit_is_clipped_to_the_committed_mask() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        marquee(&mut app, (4, 4), (7, 7));
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Pencil)]);

        release_line_gesture(&mut app, (2, 2), (9, 9), false);

        assert_eq!(
            pixels_holding(&app, RED),
            vec![(4, 4), (5, 5), (6, 6), (7, 7)],
            "the LINE commit must write only inside the selection"
        );
    }

    #[test]
    /// Given a selection, when a dynamic-curve gesture commits a diagonal
    /// across it, then only the in-selection cells are painted.
    fn dynamic_curve_commit_is_clipped_to_the_committed_mask() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        marquee(&mut app, (4, 4), (7, 7));
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Pencil)]);

        release_line_gesture(&mut app, (2, 2), (9, 9), true);
        assert!(
            app.projects.current().transform.is_some(),
            "the dynamic release builds the curve transform object first"
        );
        // A double-click far from the curve commits it as one stroke.
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((40, 110)),
            ..Default::default()
        });

        assert!(
            app.projects.current().transform.is_none(),
            "the double-click commits the curve"
        );
        assert_eq!(
            pixels_holding(&app, RED),
            vec![(4, 4), (5, 5), (6, 6), (7, 7)],
            "the dynamic-curve commit must write only inside the selection"
        );
    }

    #[test]
    /// Given a selection over a transparent canvas, when the bucket is applied
    /// inside it, then the contiguous fill stops at the selection boundary.
    fn fill_is_clipped_to_the_committed_mask() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        marquee(&mut app, (4, 4), (7, 7));
        app.apply_toolbar_events(vec![ToolbarEvent::ToolSelected(Tool::Fill)]);

        app.apply_fill((5, 5));

        assert_eq!(
            pixels_holding(&app, RED),
            (4..8)
                .flat_map(|x| (4..8).map(move |y| (x, y)))
                .collect::<Vec<_>>(),
            "the fill must cover exactly the selection, not the whole canvas"
        );
    }

    #[test]
    /// Given a selection, when the bucket is applied to a seed outside it,
    /// then nothing is painted and no undo step is pushed.
    fn fill_seed_outside_the_committed_mask_is_a_noop() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        marquee(&mut app, (4, 4), (7, 7));

        app.apply_fill((20, 20));

        assert!(
            pixels_holding(&app, RED).is_empty(),
            "a seed outside the selection must not fill anything"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "a no-op fill pushes no undo step"
        );
    }

    #[test]
    /// Given no selection, when the same pen diagonal is stroked, then the
    /// whole line is painted — the clip is absent, not the whole canvas.
    fn no_selection_paint_remains_unclipped() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        assert!(app.projects.current().selection.is_none());

        drag_stroke(&mut app, (2, 2), (9, 9));

        assert_eq!(
            pixels_holding(&app, RED),
            (2..=9).map(|n| (n, n)).collect::<Vec<_>>(),
            "without a selection the stroke must still cover its whole path"
        );
    }
    // Transform lifecycle: lift on Ctrl+click, release ends the drag, and only
    // a double-click on empty space (or a tool switch) commits; Esc cancels and
    // restores the lifted selection
    // -----------------------------------------------------------------------

    /// Seed a 2×2 RED block, marquee it, and lift it into a selection transform
    /// ready to be dragged with the Translate gizmo.
    fn lifted_red_transform() -> App {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        app
    }

    /// Translate the live selection transform by `(+2, +1)` and release the
    /// drag (which, by contract, does not commit).
    fn translate_and_release(app: &mut App) {
        app.gizmo_hovered = GizmoHit::Translate;
        app.handle_interactions(CanvasInteractions {
            stroke_started: true,
            stroke_point: Some((5, 5)),
            ..Default::default()
        });
        // Q1 problem 2: no dead zone — the first moved sample applies, and the
        // move lands absolute from the press on the Δ (+2, +1) tests assert.
        app.handle_interactions(CanvasInteractions {
            stroke_point: Some((7, 6)),
            ..Default::default()
        });
        app.handle_interactions(CanvasInteractions {
            stroke_ended: true,
            stroke_point: Some((7, 6)),
            ..Default::default()
        });
        // The commit under test must be the step that follows, never the
        // release that just ended the drag.
        assert!(
            app.projects.current().transform.is_some(),
            "the drag release must leave the session live"
        );
    }

    #[test]
    /// Given a committed selection, when the pointer Ctrl+clicks inside it
    /// without dragging, then the selection is lifted into a transform session
    /// even though no drag ever started.
    fn ctrl_click_on_a_selected_area_enters_transform_without_drag() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));
        let before = app
            .projects
            .current()
            .layers
            .active_layer()
            .buffer
            .as_bytes()
            .to_vec();

        set_modifiers(
            &app,
            egui::Modifiers {
                command: true,
                ..egui::Modifiers::NONE
            },
        );
        app.handle_interactions(CanvasInteractions {
            clicked: Some((4, 4)),
            ..Default::default()
        });
        set_modifiers(&app, egui::Modifiers::NONE);

        assert!(
            app.projects.current().transform.is_some(),
            "Ctrl+click inside the selection must lift it even without a drag"
        );
        assert!(
            app.projects.current().selection.is_none(),
            "the lifted pixels now float above the canvas"
        );
        // Part C: the lift cuts the selected pixels immediately (the hole is
        // visible at once), but pushes no undo entry.
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_ne!(
            buf.as_bytes(),
            before.as_slice(),
            "the lift must cut the source, not leave it byte-identical"
        );
        assert_eq!(
            buf.get_pixel(4, 4),
            Some(Color::TRANSPARENT),
            "the lift clears the selected pixel"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "the lift itself pushes no undo entry"
        );
        assert!(
            !app.projects
                .current()
                .transform
                .as_ref()
                .unwrap()
                .expect_selection()
                .source
                .snapshot
                .is_empty(),
            "the snapshot is kept so Esc/commit can restore the cut pixels"
        );
    }

    #[test]
    /// Given a translated selection transform, when a double-click lands just
    /// outside the gizmo-inflated bbox, then the transform commits.
    fn selection_transform_double_click_on_empty_space_commits() {
        let mut app = lifted_red_transform();
        translate_and_release(&mut app);
        let (min_x, min_y, max_x, max_y) = app
            .projects
            .current()
            .transform
            .as_ref()
            .expect("the session survives the release")
            .expect_selection()
            .object
            .canvas_bbox();
        let radius = GIZMO_HIT_RADIUS / app.projects.current().camera.scale() as f32;
        let on_object = |x: f32, y: f32| {
            x > min_x - radius && x < max_x + radius && y > min_y - radius && y < max_y + radius
        };
        assert!(
            !on_object(max_x + radius + 1.0, (min_y + max_y) / 2.0),
            "the fixture's commit point must be just outside the inflated bbox"
        );

        app.gizmo_hovered = GizmoHit::None;
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((
                (max_x + radius + 1.0) as i32,
                ((min_y + max_y) / 2.0) as i32,
            )),
            ..Default::default()
        });

        assert!(
            app.projects.current().transform.is_none(),
            "a double-click on empty space commits the session"
        );
        assert_eq!(app.projects.current().undo.undo_len(), 1);
        let buffer = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buffer.get_pixel(6, 5), Some(RED), "moved pixel (6,5)");
        assert_eq!(buffer.get_pixel(7, 6), Some(RED), "moved pixel (7,6)");
    }

    #[test]
    /// Given a live selection transform, when a double-click lands while a
    /// gizmo is hovered this frame, then the session stays live.
    fn selection_transform_double_click_on_a_gizmo_does_not_commit() {
        let mut app = lifted_red_transform();
        translate_and_release(&mut app);

        app.gizmo_hovered = GizmoHit::Rotate;
        app.handle_interactions(CanvasInteractions {
            double_clicked: Some((7, 6)),
            ..Default::default()
        });

        assert!(
            app.projects.current().transform.is_some(),
            "a double-click on a gizmo must keep the session live"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "no commit, no undo step"
        );
    }

    #[test]
    /// Given a translated selection transform, when Esc is pressed, then the
    /// transform cancels and the original selection is restored — nothing is
    /// pasted at the destination, no undo entry is pushed.
    fn selection_transform_escape_cancels_and_restores_the_selection() {
        let mut app = lifted_red_transform();
        translate_and_release(&mut app);

        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);

        assert!(
            app.projects.current().transform.is_none(),
            "Esc cancels the selection transform"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "a cancelled transform pushes no undo step"
        );
        let buffer = &app.projects.current().layers.active_layer().buffer;
        // Source pixels are still RED at the original spot (nothing was cut).
        for y in 4..=5 {
            for x in 4..=5 {
                assert_eq!(
                    buffer.get_pixel(x, y),
                    Some(RED),
                    "source lost at ({x},{y})"
                );
            }
        }
        // Nothing was pasted at the drag destination.
        assert_eq!(
            buffer.get_pixel(6, 5),
            Some(Color::TRANSPARENT),
            "nothing pasted at (6,5)"
        );
        assert_eq!(
            buffer.get_pixel(7, 6),
            Some(Color::TRANSPARENT),
            "nothing pasted at (7,6)"
        );
        // The selection is restored to the original source rect.
        let restored = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("the cancelled transform must restore the selection");
        assert_eq!(
            restored.rect(),
            Rect2i::new(4, 4, 2, 2),
            "the selection is restored to its original source rect"
        );
    }

    #[test]
    /// L1-C: Enter confirms a live selection transform; Esc cancels it without
    /// touching the document (no undo step, source pixels restored).
    fn enter_commits_and_escape_cancels_the_transform_session() {
        // Enter commits a moved selection as one undo step.
        let mut app = lifted_red_transform();
        translate_and_release(&mut app);
        assert!(app.projects.current().transform.is_some());
        send_key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
        assert!(
            app.projects.current().transform.is_none(),
            "Enter must commit the selection transform"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            1,
            "an Enter commit is exactly one undo step"
        );
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(6, 5),
            Some(RED),
            "the committed pixels stay in the document"
        );

        // Esc cancels.
        let mut app = lifted_red_transform();
        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(
            app.projects.current().transform.is_none(),
            "Esc must cancel the selection transform"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "a cancelled transform pushes no undo step"
        );
        let buffer = &app.projects.current().layers.active_layer().buffer;
        for y in 4..=5 {
            for x in 4..=5 {
                assert_eq!(
                    buffer.get_pixel(x, y),
                    Some(RED),
                    "Esc restores the lifted source pixel ({x},{y})"
                );
            }
        }
    }

    #[test]
    /// L1-C: a double-click in the letterbox (outside the canvas draw rect)
    /// still commits the floating selection via the global fallback.
    fn canvas_outside_double_click_commits_the_transform() {
        let ctx = egui::Context::default();
        let mut app = app_with_canvas(&ctx);
        // The global commit fallback reads `self.ctx`; point it at the harness
        // context so the double-click input is visible.
        app.ctx = ctx.clone();
        app.panel_dock_demo = false;
        app.lift_transform(Rect2i::new(20, 20, 40, 40));
        assert!(app.projects.current().transform.is_some());

        // (200,150) is letterbox: outside the 128x128 canvas and clear of the
        // dock chrome.
        let outside = egui::pos2(200.0, 150.0);
        let mut clock = ctx.input(|i| i.time);
        run_app_frame_animated(&mut app, &ctx, &mut clock, vec![move_to(outside)]);
        for _ in 0..2 {
            run_app_frame_animated(&mut app, &ctx, &mut clock, vec![press(outside)]);
            run_app_frame_animated(&mut app, &ctx, &mut clock, vec![release(outside)]);
        }

        assert!(
            app.projects.current().transform.is_none(),
            "a double-click outside the canvas must commit the floating selection"
        );
    }

    #[test]
    /// Given a live dynamic-curve transform, when Esc is pressed, then the
    /// curve is cancelled and nothing is written.
    fn curve_transform_escape_still_cancels() {
        let mut app = App::default();
        app.apply_toolbar_events(vec![ToolbarEvent::ColorChanged(RED)]);
        release_line_gesture(&mut app, (10, 10), (20, 20), true);
        assert!(app.projects.current().transform.is_some());
        let before = app
            .projects
            .current()
            .layers
            .active_layer()
            .buffer
            .as_bytes()
            .to_vec();

        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);

        assert!(
            app.projects.current().transform.is_none(),
            "Esc cancels the curve transform"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "a cancelled curve pushes no undo step"
        );
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .as_bytes()
                .to_vec(),
            before,
            "a cancelled curve must leave the document byte-identical"
        );
    }

    #[test]
    /// Given a committed selection and no live transform, when Esc is pressed,
    /// then the selection is cleared and the pixels are untouched.
    fn escape_without_a_transform_still_clears_the_selection() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));
        assert!(app.projects.current().selection.is_some());
        let before = app
            .projects
            .current()
            .layers
            .active_layer()
            .buffer
            .as_bytes()
            .to_vec();

        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);

        assert!(
            app.projects.current().selection.is_none(),
            "Esc clears the selection when no transform is live"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "clearing a selection pushes no undo step"
        );
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .as_bytes()
                .to_vec(),
            before,
            "Esc must not touch the document"
        );
    }

    #[test]
    /// Given a committed RECTANGULAR selection transform, when it commits, then
    /// a selection is re-captured at the transformed destination so the moved
    /// area stays selected.
    fn selection_transform_commit_recaptures_a_rect_selection_at_the_destination() {
        let mut app = lifted_red_transform();
        assert!(
            app.projects
                .current()
                .selection
                .as_ref()
                .is_none_or(|sel| sel.is_rectangular()),
            "the fixture lifts a rectangular selection"
        );
        translate_and_release(&mut app);

        app.transform_session_commit();

        assert_eq!(
            app.projects
                .current()
                .selection
                .as_ref()
                .map(Selection::rect),
            Some(Rect2i::new(6, 5, 2, 2)),
            "the transformed area must stay selected at its new position"
        );
    }

    #[test]
    /// Part C: a Ctrl lift CUTS the selected pixels out of the source layer
    /// immediately (the hole is visible at once) and pushes no undo entry; the
    /// snapshot stays on the session for restore/commit.
    fn lift_cuts_the_selected_pixels_from_the_source() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        marquee(&mut app, (4, 4), (5, 5));
        app.lift_transform(Rect2i::new(4, 4, 2, 2));

        let buf = &app.projects.current().layers.active_layer().buffer;
        for y in 4..=5 {
            for x in 4..=5 {
                assert_eq!(
                    buf.get_pixel(x, y),
                    Some(Color::TRANSPARENT),
                    "the lift must cut ({x},{y}) immediately"
                );
            }
        }
        assert_eq!(
            app.projects.current().undo.undo_len(),
            0,
            "the lift itself pushes no undo entry"
        );
        let t = app
            .projects
            .current()
            .transform
            .as_ref()
            .unwrap()
            .expect_selection();
        assert_eq!(t.source.rect, Rect2i::new(4, 4, 2, 2));
        assert_eq!(t.source.snapshot.len(), 2 * 2 * 4);
        assert!(t.cut_source);
    }

    #[test]
    /// Part C: Esc on a live lift RESTORES the cut pixels byte-exactly (and the
    /// original selection), without pushing any undo entry.
    fn escape_restores_the_cut_pixels() {
        let mut app = lifted_red_transform();
        assert_eq!(
            app.projects
                .current()
                .layers
                .active_layer()
                .buffer
                .get_pixel(4, 4),
            Some(Color::TRANSPARENT),
            "the lift cut the source before Esc"
        );
        translate_and_release(&mut app);

        send_key(&mut app, egui::Key::Escape, egui::Modifiers::NONE);

        assert!(app.projects.current().transform.is_none());
        assert_eq!(app.projects.current().undo.undo_len(), 0);
        let buf = &app.projects.current().layers.active_layer().buffer;
        for y in 4..=5 {
            for x in 4..=5 {
                assert_eq!(buf.get_pixel(x, y), Some(RED), "Esc restores ({x},{y})");
            }
        }
        assert_eq!(
            buf.get_pixel(6, 5),
            Some(Color::TRANSPARENT),
            "Esc pastes nothing at the drag destination"
        );
        assert_eq!(
            app.projects
                .current()
                .selection
                .as_ref()
                .expect("Esc restores the original selection")
                .rect(),
            Rect2i::new(4, 4, 2, 2)
        );
    }

    #[test]
    /// Part C: committing a MASKED transform keeps the OLD source mask area cut
    /// and selects the NEW transformed pixels' area as a mask (alpha > 0), as a
    /// single undo step.
    fn commit_leaves_source_cut_and_selects_transformed_area() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(4, 4, 2, 2));
        let mut mask = vec![true; 4];
        mask[3] = false; // hole at (5,5)
        install_masked_selection(&mut app, Rect2i::new(4, 4, 2, 2), mask.clone());
        app.lift_transform(Rect2i::new(4, 4, 2, 2));
        translate_and_release(&mut app);

        app.transform_session_commit();

        assert!(
            app.projects.current().transform.is_none(),
            "the transform must commit"
        );
        assert_eq!(
            app.projects.current().undo.undo_len(),
            1,
            "the commit is exactly one undo step"
        );
        let buf = &app.projects.current().layers.active_layer().buffer;
        // The old selected mask area stays cut; the masked-out cell was never
        // lifted so it survives.
        assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(5, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(4, 5), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(5, 5), Some(RED));
        // The transformed pixels are pasted at the destination.
        assert_eq!(buf.get_pixel(6, 5), Some(RED));
        assert_eq!(buf.get_pixel(7, 5), Some(RED));
        assert_eq!(buf.get_pixel(6, 6), Some(RED));
        // The NEW selection is a mask over the transformed area, not a plain
        // rect.
        let sel = app
            .projects
            .current()
            .selection
            .as_ref()
            .expect("a masked commit selects the transformed area");
        assert!(!sel.is_rectangular(), "the transformed area keeps its mask");
        assert_eq!(sel.rect(), Rect2i::new(6, 5, 2, 2));
        assert_eq!(sel.mask(), Some([true, true, true, false].as_slice()));
    }

    #[test]
    /// Part C: undo after a commit restores the original source pixels AND
    /// removes the paste in ONE step; redo re-applies both.
    fn undo_after_commit_restores_source_and_removes_paste() {
        let mut app = lifted_red_transform();
        translate_and_release(&mut app);
        app.transform_session_commit();
        {
            let buf = &app.projects.current().layers.active_layer().buffer;
            assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
            assert_eq!(buf.get_pixel(6, 5), Some(RED));
        }

        app.undo_document();

        {
            let buf = &app.projects.current().layers.active_layer().buffer;
            for y in 4..=5 {
                for x in 4..=5 {
                    assert_eq!(
                        buf.get_pixel(x, y),
                        Some(RED),
                        "undo restores source ({x},{y})"
                    );
                }
            }
            for y in 5..=6 {
                for x in 6..=7 {
                    assert_eq!(
                        buf.get_pixel(x, y),
                        Some(Color::TRANSPARENT),
                        "undo removes the paste at ({x},{y})"
                    );
                }
            }
        }
        assert_eq!(app.projects.current().undo.undo_len(), 0);

        app.redo_document();
        let buf = &app.projects.current().layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(4, 4), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(6, 5), Some(RED));
    }

    #[test]
    /// Part B: copy respects the mask shape — holes stay transparent.
    fn copy_masks_holes_transparent() {
        let mut app = App::default();
        seed_red_rect(&mut app, Rect2i::new(2, 2, 4, 4));
        let mut mask = vec![false; 16];
        mask[0] = true;
        mask[15] = true;
        app.projects.current_mut().selection = Selection::capture_mask(
            &app.projects.current().layers.active_layer().buffer,
            Rect2i::new(2, 2, 4, 4),
            mask,
        );
        app.projects.current_mut().os_clipboard_available = false;

        app.copy_selection();

        let clip = app
            .projects
            .current()
            .clipboard
            .as_ref()
            .expect("copy fills the internal clipboard");
        assert_eq!((clip.width(), clip.height()), (4, 4));
        assert_eq!(clip.get_pixel(0, 0), Some(RED));
        assert_eq!(
            clip.get_pixel(1, 0),
            Some(Color::TRANSPARENT),
            "a hole must be written transparent"
        );
        assert_eq!(clip.get_pixel(3, 3), Some(RED));
    }
}
