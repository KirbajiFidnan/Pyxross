//! Project-local editing context owned by the application shell.
//!
//! A panel receives a short-lived borrow of the active session through
//! [`ProjectStore::current`] or [`ProjectStore::current_mut`].  It never owns a
//! session, document, or layer reference.  The store is intentionally free of
//! egui and winit so project switching is testable without a host.

use std::collections::BTreeMap;
use std::path::PathBuf;

use std::collections::HashMap;

use crate::core::anim::AnimationController;
use crate::core::brush::{BrushShape, BrushSpec, ScatterShape};
use crate::core::buffer::PixelBuffer;
use crate::core::camera::Camera;
use crate::core::clipboard::ClipboardRegion;
use crate::core::color::Color;
use crate::core::document::Document;
use crate::core::layer_commands::TransformSource;
use crate::core::math::Rect2i;
use crate::core::model::frame::Frame;
use crate::core::model::region::Region;
use crate::core::model::sequence::AnimationSequence;
use crate::core::model::{Layer, LayerId, LayerStack};
use crate::core::palette::Palette;
use crate::core::select::{SelectMode, Selection};
use crate::core::transform::{CurveTransform, TransformAlgorithm, TransformObject};
use crate::core::undo::UndoStack;
use crate::input::{Tool, ToolState};
use crate::io::RecoveryJournal;
use crate::render::gizmo::GizmoHit;
use crate::render::overlay::BoundarySegment;

/// Stable identity for an open project. Ids are monotonic within a store and
/// are never reused after a project closes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProjectId(u64);

impl ProjectId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectTab {
    pub id: ProjectId,
    pub name: String,
    pub active: bool,
    pub dirty: bool,
}

/// Whether a state category belongs to a project, the shell, or the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerClass {
    ProjectLocal,
    ShellGlobal,
    HostLocal,
}

impl OwnerClass {
    pub const fn is_project_local(self) -> bool {
        matches!(self, Self::ProjectLocal)
    }
    pub const fn is_shell_global(self) -> bool {
        matches!(self, Self::ShellGlobal)
    }
    pub const fn is_host_local(self) -> bool {
        matches!(self, Self::HostLocal)
    }
}

/// The task-2 ownership contract. Keep this exhaustive when App gains fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ownership {
    pub category: &'static str,
    pub owner: OwnerClass,
    pub fields: &'static str,
    pub close_policy: &'static str,
}

impl Ownership {
    pub const ALL: &'static [Self] = &[
        Self { category: "document/layers", owner: OwnerClass::ProjectLocal, fields: "layers", close_policy: "discard with session" },
        Self { category: "project metadata/path/dirty", owner: OwnerClass::ProjectLocal, fields: "id, name, path, modified", close_policy: "guard dirty" },
        Self { category: "save/recovery/autosave", owner: OwnerClass::ProjectLocal, fields: "save_generation, recovery_key, recovery_pending, autosave_generation, last_autosave, recovery", close_policy: "clear with session" },
        Self { category: "undo", owner: OwnerClass::ProjectLocal, fields: "undo", close_policy: "discard with session" },
        Self { category: "selection", owner: OwnerClass::ProjectLocal, fields: "selection, selection_snapshot, gesture", close_policy: "discard with session" },
        Self { category: "transform", owner: OwnerClass::ProjectLocal, fields: "transform, stroke, transform_algorithm", close_policy: "discard with session" },
        Self { category: "clipboard", owner: OwnerClass::ProjectLocal, fields: "clipboard, os_clipboard_available", close_policy: "discard with session" },
        Self { category: "active layer/frame", owner: OwnerClass::ProjectLocal, fields: "layers, animation", close_policy: "discard with session" },
        Self { category: "camera", owner: OwnerClass::ProjectLocal, fields: "camera", close_policy: "discard with session" },
        Self { category: "animation/playback", owner: OwnerClass::ProjectLocal, fields: "sequence, animation", close_policy: "discard with session" },
        Self { category: "palette/colors", owner: OwnerClass::ProjectLocal, fields: "palettes, active_palette, color, secondary_color", close_policy: "discard with session" },
        Self { category: "tool/brush/grid", owner: OwnerClass::ProjectLocal, fields: "tool_state, pen_draw_settings, eraser_draw_settings, grid_visible", close_policy: "discard with session" },
        Self { category: "canvas texture", owner: OwnerClass::ProjectLocal, fields: "canvas_generation", close_policy: "discard with session" },
        Self { category: "dialogs", owner: OwnerClass::ProjectLocal, fields: "tile_size, new_draft_w, new_draft_h, dialog_open, pending_action, last_error", close_policy: "discard with session" },
        Self { category: "keymap", owner: OwnerClass::ShellGlobal, fields: "App.keymap", close_policy: "retain" },
        Self { category: "theme", owner: OwnerClass::ShellGlobal, fields: "App.theme", close_policy: "retain" },
        Self { category: "panel placement/UI memory", owner: OwnerClass::ShellGlobal, fields: "future layout owner", close_policy: "retain" },
        Self { category: "host input/render state", owner: OwnerClass::HostLocal, fields: "window, renderer, egui_state, ctx", close_policy: "retain per host" },
    ];
}

/// The complete draw configuration for one tool: brush footprint, stamp
/// scatter and size taper. Pen and Eraser each own an independent bundle so
/// switching tools never disturbs the other's setup.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DrawSettings {
    pub size: u8,
    pub shape: BrushShape,
    pub scatter: u8,
    pub scatter_shape: ScatterShape,
    pub tail: i8,
}

impl Default for DrawSettings {
    fn default() -> Self {
        Self {
            size: BrushSpec::MIN_SIZE,
            shape: BrushShape::Square,
            scatter: 0,
            scatter_shape: ScatterShape::Square,
            tail: 0,
        }
    }
}

/// All state that must survive switching away from a project.
pub struct ProjectSession {
    pub id: ProjectId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub tile_size: u32,
    pub layers: LayerStack,
    pub undo: UndoStack,
    pub camera: Camera,
    pub selection: Option<Selection>,
    pub selection_snapshot: Option<Selection>,
    /// The in-flight Fieldier selection gesture; `Idle` when none is running.
    pub gesture: SelectGesture,
    /// Whether the OS image clipboard is available on this host (reported).
    pub os_clipboard_available: bool,
    pub clipboard: Option<ClipboardRegion>,
    pub stroke: Option<crate::core::stroke_command::StrokeSession>,
    pub transform: Option<TransformSession>,
    pub sequence: AnimationSequence,
    pub animation: AnimationController,
    pub palettes: Vec<Palette>,
    pub active_palette: usize,
    pub color: Color,
    pub secondary_color: Color,
    pub tool_state: ToolState,
    pub pen_draw_settings: DrawSettings,
    pub eraser_draw_settings: DrawSettings,
    /// The free-angle rotation algorithm applied to newly-lifted (and live)
    /// selection transforms. A fresh session seeds its `TransformObject` from
    /// this, so the Tool Property panel's selector survives lift/cancel/lift.
    pub transform_algorithm: TransformAlgorithm,
    pub grid_visible: bool,
    pub canvas_generation: u64,
    pub modified: bool,
    pub save_generation: u64,
    pub recovery_key: String,
    pub recovery_pending: bool,
    pub autosave_generation: u64,
    pub new_draft_w: u32,
    pub new_draft_h: u32,
    pub dialog_open: bool,
    pub pending_action: Option<PendingAction>,
    pub last_autosave: Option<std::time::Instant>,
    pub recovery: Option<RecoveryJournal>,
    pub last_error: Option<String>,
}

/// The in-flight Fieldier selection gesture.
///
/// Selection and movement are separate states, not one another's side effect:
/// a gesture is only ever one of these, so "marquee anchor without a marquee"
/// or "a move destination with no drag" cannot be represented.  `base` is the
/// selection as it stood before a combining gesture started, so Add/Subtract/
/// Intersect can merge into it — it only exists while that gesture is live.
#[derive(Default)]
pub enum SelectGesture {
    /// No selection gesture in flight.
    #[default]
    Idle,
    /// A rectangle drag: the press anchor, the box drawn so far, and the
    /// combining mode chosen at press (D78).
    Marquee {
        origin: (i32, i32),
        rect: Rect2i,
        base: Option<Selection>,
        mode: SelectMode,
    },
    /// A drag from inside the selection: the press anchor, the destination
    /// box, and the translated mask boundary to preview.  The move commits as
    /// one cut+paste undo step on release (D76 supersedes D50's area-only
    /// move).  `preview` is `None` for a rectangular selection (the plain
    /// destination box is the preview) and the translated mask's boundary
    /// segments for a mask selection, so the notches and protrusions stay
    /// visible during the drag.
    AreaMove {
        origin: (i32, i32),
        dest: Rect2i,
        preview: Option<Vec<BoundarySegment>>,
    },
    /// A freehand Lasso drag: the traced polygon so far, the pre-gesture
    /// selection a Shift/Alt gesture merges into, and the combining mode
    /// chosen at press (D78).
    Lasso {
        points: Vec<(i32, i32)>,
        base: Option<Selection>,
        mode: SelectMode,
    },
    /// An Alt+drag grid-cell border: the press anchor, the live pointer, the
    /// whole grid cells crossed so far, the pre-gesture selection, and the
    /// combining mode chosen at press (D78).
    TileBorder {
        origin: (i32, i32),
        current: (i32, i32),
        cells: Vec<Rect2i>,
        base: Option<Selection>,
        mode: SelectMode,
    },
}

/// An Alt+drag grid-border gesture's `(press, current, crossed cells)`.
pub type TileBorderSnapshot<'a> = ((i32, i32), (i32, i32), &'a [Rect2i]);

impl SelectGesture {
    /// The destination box while a selection-area move is in flight.
    pub fn move_dest(&self) -> Option<Rect2i> {
        match self {
            Self::AreaMove { dest, .. } => Some(*dest),
            _ => None,
        }
    }

    /// The rectangle a marquee has drawn so far, with its press anchor.
    pub fn marquee(&self) -> Option<((i32, i32), Rect2i)> {
        match self {
            Self::Marquee { origin, rect, .. } => Some((*origin, *rect)),
            _ => None,
        }
    }

    /// The freehand polygon a lasso has drawn so far, or `None` when no lasso
    /// is in flight.
    pub fn lasso_points(&self) -> Option<&[(i32, i32)]> {
        match self {
            Self::Lasso { points, .. } => Some(points),
            _ => None,
        }
    }

    /// The live polygon's last point, or `None` when no lasso is in flight.
    pub fn lasso_last(&self) -> Option<(i32, i32)> {
        match self {
            Self::Lasso { points, .. } => points.last().copied(),
            _ => None,
        }
    }

    /// The `(press, current)` endpoints and crossed cells of an Alt+drag
    /// grid-border gesture, or `None`.
    pub fn tile_border(&self) -> Option<TileBorderSnapshot<'_>> {
        match self {
            Self::TileBorder {
                origin,
                current,
                cells,
                ..
            } => Some((*origin, *current, cells)),
            _ => None,
        }
    }

    /// Whether a gesture that draws a preview box is in flight.
    pub fn is_dragging_box(&self) -> bool {
        matches!(
            self,
            Self::Marquee { .. } | Self::AreaMove { .. } | Self::TileBorder { .. }
        )
    }

    /// Whether no selection gesture is in flight. The canvas widget reports
    /// `stroke_started` on the first drag *delta*, not on the press, so a live
    /// gesture is what tells a new press apart from the current drag.
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }
}

pub struct SelectionTransform {
    pub source: TransformSource,
    pub object: TransformObject,
    pub drag: GizmoHit,
    pub last_pt: (f32, f32),
    /// When false the source rect is left in place at commit, so the transform
    /// pastes a copy of the selected pixels.  The Fieldier only ever lifts a
    /// cut now (D76 removed the Alt copy lift); the flag remains for the
    /// transform-session contract.
    pub cut_source: bool,
    /// Object angle at the start of the current drag (degrees). Rotation is
    /// recomputed absolute as `start_angle + (θ(cur) − θ(start_pointer))`, so
    /// it never accumulates per-frame error and a grab-then-release never
    /// jumps.
    pub start_angle: f32,
    /// Pointer angle around the pivot captured at drag start (degrees). Paired
    /// with [`Self::start_angle`] for the absolute rotate delta.
    pub start_pointer_angle: f32,
    /// The handle the current start-state was captured for. A change of
    /// `drag` (a new gesture) re-captures the start state even when the App's
    /// begin-drag path was skipped.
    pub start_drag: GizmoHit,
    /// Canvas pointer at drag start: the MOVE dead-zone and axis origin.
    pub start_pointer: (f32, f32),
    /// Object `pos` at drag start, so a MOVE is computed absolute from the
    /// press (integer-snapped) rather than per-frame accumulated.
    pub start_pos: (f32, f32),
    /// LOCAL scaled dims (`object.local_scaled_dims()`) at drag start. A scale
    /// computes its target ABSOLUTELY as `round(start_local_dims * |ratio|)`,
    /// so the dragged handle tracks the cursor without compounding
    /// float/`round` drift.
    pub start_local_dims: (usize, usize),
    /// Physical CANVAS-space resize anchor captured ONCE from the rotated quad
    /// (the opposite corner / opposite edge midpoint / quad centre for Alt).
    /// It stays fixed for the whole gesture, so the dragged handle cannot
    /// drift and the anchor cannot jump when a flip toggles. The local-frame
    /// algorithm rotates this back by `start_angle` about the pivot.
    pub start_anchor: (f32, f32),
    /// Which side of the local box the anchor represents per axis
    /// (min / centre / max), from the handle's source-space anchor table. Used
    /// by the local resize to derive the new local min without inspecting the
    /// pointer's side (which the pre-Option-A code got wrong for edges).
    pub start_anchor_end: (AnchorEnd, AnchorEnd),
    /// Pivot at drag start (recorded for reference; the live algorithm uses
    /// the object's current pivot, which is recentred to the bbox centre after
    /// every scale).
    pub start_pivot: (f32, f32),
    /// Mirror state at drag start. The live flip is `start_flip XOR (ratio <
    /// 0)` — absolute from the start, never toggled frame-by-frame.
    pub start_flip_h: bool,
    /// Vertical mirror state at drag start (see [`Self::start_flip_h`]).
    pub start_flip_v: bool,
    /// Whether Alt was held at drag start. When true the anchor is the START
    /// bbox CENTRE, so the resize is symmetric about it (both sides move).
    pub start_alt: bool,
    /// Axis chosen from the initial movement direction after the MOVE dead
    /// zone; `None` until the gesture breaks out of it. Used only to constrain
    /// a Shift-held MOVE.
    pub move_axis: Option<MoveAxis>,
    /// Whether the drag start-state has been captured for the current gesture.
    pub drag_initialized: bool,
    /// Y2-B: true while a gizmo drag is in flight (set on press, cleared on
    /// release). The preview renders with the FAST algorithm
    /// ([`super::mod`-side `TRANSFORM_DRAG_PREVIEW_ALGORITHM`]) while this is
    /// true, and with the stored (selected) algorithm when idle. Commit never
    /// consults this flag; it always uses the stored algorithm.
    pub preview_dragging: bool,
}

/// The axis a Shift-held MOVE is locked to, chosen from the initial movement
/// direction once the pointer leaves the dead zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveAxis {
    Horizontal,
    Vertical,
}

/// Which side of the LOCAL box a resize anchor represents on one axis.
///
/// The source-space anchor table maps a handle to a corner/edge of the local
/// rect; `Min` means the anchor is the low edge, `Max` the high edge, and
/// `Center` the midpoint (edge handles' untracked axis, or Alt). The local
/// resize derives the new local min from this explicitly, instead of inferring
/// it from the pointer's side (which broke off-midpoint edge presses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorEnd {
    Min,
    Center,
    Max,
}

/// The Alt-driven dynamic line as a live transform object (Phase 2): a
/// bendable [`CurveTransform`] plus the gizmo drag in progress.
pub struct CurveTransformSession {
    pub curve: CurveTransform,
    /// Index of the gizmo currently being dragged, if any.
    pub drag: Option<usize>,
    /// Canvas-space anchor of the last drag sample.
    pub last_pt: (f32, f32),
}

/// A live in-canvas transform object: either a lifted pixel selection
/// (D32/D35/D36) or the Alt-driven dynamic curve. Both share the gesture
/// routing, overlay, cancel and commit-and-clear lifecycle.
pub enum TransformSession {
    Selection(SelectionTransform),
    Curve(CurveTransformSession),
}

impl TransformSession {
    /// The lifted-pixel selection, when this session is the selection variant.
    pub fn selection(&self) -> Option<&SelectionTransform> {
        match self {
            Self::Selection(selection) => Some(selection),
            Self::Curve(_) => None,
        }
    }

    /// Mutable lifted-pixel selection, when this session is that variant.
    pub fn selection_mut(&mut self) -> Option<&mut SelectionTransform> {
        match self {
            Self::Selection(selection) => Some(selection),
            Self::Curve(_) => None,
        }
    }

    /// The dynamic curve, when this session is the curve variant.
    pub fn curve(&self) -> Option<&CurveTransform> {
        match self {
            Self::Curve(curve) => Some(&curve.curve),
            Self::Selection(_) => None,
        }
    }

    /// Mutable dynamic curve session, when this session is that variant.
    pub fn curve_mut(&mut self) -> Option<&mut CurveTransformSession> {
        match self {
            Self::Curve(curve) => Some(curve),
            Self::Selection(_) => None,
        }
    }

    /// The selection variant, panicking when this is a curve session (tests).
    pub fn expect_selection(&self) -> &SelectionTransform {
        self.selection()
            .expect("expected a selection transform session")
    }

    /// Mutable selection variant, panicking on a curve session (tests).
    pub fn expect_selection_mut(&mut self) -> &mut SelectionTransform {
        self.selection_mut()
            .expect("expected a selection transform session")
    }
}

#[derive(Clone, PartialEq)]
pub enum PendingAction {
    New {
        project: ProjectId,
        width: u32,
        height: u32,
        tile: u32,
    },
    Open {
        project: ProjectId,
        path: PathBuf,
    },
    Close {
        project: ProjectId,
    },
}

pub struct ProjectFrameContext<'a> {
    session: &'a ProjectSession,
}

pub struct ProjectFrameMut<'a> {
    session: &'a mut ProjectSession,
}

impl<'a> ProjectFrameContext<'a> {
    pub fn id(&self) -> ProjectId {
        self.session.id
    }
    pub fn session(&self) -> &'a ProjectSession {
        self.session
    }
    pub fn to_document(&self) -> Document {
        self.session.to_document()
    }
}

impl<'a> ProjectFrameMut<'a> {
    pub fn session(&mut self) -> &mut ProjectSession {
        self.session
    }
    pub fn mark_dirty(&mut self) {
        self.session.mark_dirty();
    }
    pub fn mark_saved(&mut self) {
        self.session.mark_saved();
    }
    pub fn load_document(&mut self, document: &Document) {
        self.session.load_document(document);
    }
}

impl ProjectSession {
    pub fn to_document(&self) -> Document {
        let layers = self
            .layers
            .iter()
            .map(|layer| crate::core::document::LayerDoc {
                id: layer.id.as_u64(),
                name: layer.name.clone(),
                visible: layer.visible,
                opacity: layer.opacity,
                blend: layer.blend,
                parent: layer.parent.map(LayerId::as_u64),
                is_group: layer.is_group,
                pixels: if layer.is_group {
                    Vec::new()
                } else {
                    layer.buffer.as_bytes().to_vec()
                },
            })
            .collect();
        let frames: Vec<crate::core::document::FrameDoc> = self
            .sequence
            .iter()
            .enumerate()
            .map(|(index, frame)| crate::core::document::FrameDoc {
                region_id: index as u64 + 1,
                delay_ms: frame.delay_ms(),
            })
            .collect();
        let regions = self
            .sequence
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let rect = frame.region().rect();
                crate::core::document::RegionDoc {
                    id: index as u64 + 1,
                    x: rect.x.max(0) as u32,
                    y: rect.y.max(0) as u32,
                    width: rect.w.max(0) as u32,
                    height: rect.h.max(0) as u32,
                }
            })
            .collect();
        let sequences = vec![crate::core::document::SequenceDoc {
            name: self.sequence.name().to_string(),
            frame_ids: (0..frames.len() as u64).collect(),
            loop_flag: self.sequence.looping(),
            tags: Vec::new(),
        }];
        Document {
            canvas_width: self.layers.width() as u32,
            canvas_height: self.layers.height() as u32,
            tile_size: self.tile_size,
            layers,
            regions,
            frames,
            sequences,
            palette: self
                .palettes
                .get(self.active_palette)
                .map_or_else(Vec::new, |palette| palette.colors.clone()),
            editor: crate::core::document::EditorSettings::default(),
        }
    }

    pub fn load_document(&mut self, doc: &Document) {
        let width = doc.canvas_width as usize;
        let height = doc.canvas_height as usize;
        let mut layers = LayerStack::new(width, height);
        let default_id = layers.active_layer_id();
        let mut replaced_default = false;
        for (index, layer_doc) in doc.layers.iter().enumerate() {
            let mut buffer = PixelBuffer::new(width, height);
            if !layer_doc.is_group {
                buffer.as_bytes_mut().copy_from_slice(&layer_doc.pixels);
            }
            let layer = Layer {
                id: LayerId::new(layer_doc.id),
                name: layer_doc.name.clone(),
                visible: layer_doc.visible,
                opacity: layer_doc.opacity,
                blend: layer_doc.blend,
                buffer,
                parent: None,
                is_group: layer_doc.is_group,
            };
            if layer_doc.id == default_id.as_u64() {
                if let Some(default_layer) = layers.layer_mut(default_id) {
                    *default_layer = layer;
                }
                replaced_default = true;
            } else {
                layers.insert_layer(layer, index);
            }
        }
        if !doc.layers.is_empty() && !replaced_default {
            layers.remove_layer(default_id);
        }
        // The manifest is DFS pre-order, so the flat order already matches;
        // restore_structure re-applies the parent links (and validates them).
        // The io boundary rejects malformed parent graphs before this point.
        if !doc.layers.is_empty() {
            let entries: Vec<(LayerId, Option<LayerId>)> = doc
                .layers
                .iter()
                .map(|layer_doc| {
                    (
                        LayerId::new(layer_doc.id),
                        layer_doc.parent.map(LayerId::new),
                    )
                })
                .collect();
            layers.restore_structure(&entries);
        }
        self.layers = layers;

        let mut sequence = AnimationSequence::new(
            doc.sequences
                .first()
                .map_or("Animation 1", |item| item.name.as_str()),
        );
        if let Some(sequence_doc) = doc.sequences.first() {
            sequence.set_looping(sequence_doc.loop_flag);
        }
        let region_by_id: HashMap<u64, &crate::core::document::RegionDoc> = doc
            .regions
            .iter()
            .map(|region| (region.id, region))
            .collect();
        for (index, frame) in doc.frames.iter().enumerate() {
            if let Some(region) = region_by_id.get(&frame.region_id) {
                sequence.push(Frame::new(
                    Region::new(
                        Rect2i::new(
                            region.x as i32,
                            region.y as i32,
                            region.width as i32,
                            region.height as i32,
                        ),
                        format!("Frame {}", index + 1),
                    ),
                    frame.delay_ms,
                ));
            }
        }
        if sequence.is_empty() {
            sequence.push(Frame::new(
                Region::new(Rect2i::new(0, 0, width as i32, height as i32), "Frame 1"),
                100,
            ));
        }
        let delays = sequence.iter().map(|frame| frame.delay_ms()).collect();
        self.sequence = sequence;
        self.animation = AnimationController::new_from_delays(self.sequence.len(), delays);
        self.undo.clear();
        self.selection = None;
        self.selection_snapshot = None;
        self.gesture = SelectGesture::Idle;
        self.clipboard = None;
        self.stroke = None;
        self.transform = None;
        self.tile_size = doc.tile_size.max(1);
        if !doc.palette.is_empty() {
            self.palettes = vec![Palette {
                name: "Project".to_string(),
                version: Palette::CURRENT_VERSION,
                colors: doc.palette.clone(),
            }];
            self.active_palette = 0;
            if let Some(color) = self.palettes[0].color(0) {
                self.color = color;
            }
        }
        self.canvas_generation = self.canvas_generation.wrapping_add(1);
    }

    pub fn new(id: ProjectId, name: impl Into<String>, width: usize, height: usize) -> Self {
        let layers = LayerStack::new(width, height);
        let mut sequence = AnimationSequence::new("Animation 1");
        sequence.set_looping(true);
        sequence.push(Frame::new(
            Region::new(Rect2i::new(0, 0, width as i32, height as i32), "Frame 1"),
            100,
        ));
        let animation = AnimationController::new_from_delays(1, vec![100]);
        Self {
            id,
            name: name.into(),
            path: None,
            tile_size: 16,
            layers,
            undo: UndoStack::new(),
            camera: Camera::new(),
            selection: None,
            selection_snapshot: None,
            gesture: SelectGesture::Idle,
            clipboard: None,
            os_clipboard_available: false,
            stroke: None,
            transform: None,
            sequence,
            animation,
            palettes: vec![Palette::default()],
            active_palette: 0,
            color: Color::BLACK,
            secondary_color: Color::WHITE,
            tool_state: ToolState::new(),
            pen_draw_settings: DrawSettings::default(),
            eraser_draw_settings: DrawSettings::default(),
            transform_algorithm: TransformAlgorithm::default(),
            grid_visible: true,
            canvas_generation: 0,
            dialog_open: false,
            modified: false,
            save_generation: 0,
            recovery_key: format!("project-{}", id.raw()),
            recovery_pending: false,
            autosave_generation: 0,
            new_draft_w: width as u32,
            new_draft_h: height as u32,
            pending_action: None,
            last_autosave: None,
            recovery: None,
            last_error: None,
        }
    }

    pub const fn is_dirty(&self) -> bool {
        self.modified
    }
    pub fn mark_dirty(&mut self) {
        self.modified = true;
    }
    pub fn mark_saved(&mut self) {
        self.modified = false;
        self.save_generation += 1;
    }

    /// The draw-settings bundle for `tool`: the Eraser keeps its own; the Pen
    /// ([`Tool::Pencil`] / [`Tool::Draw`]) owns the shared pen bundle. The
    /// non-drawing tools have no brush of their own, so they resolve to the
    /// pen bundle — the toolbar slider then edits the pen without disturbing
    /// the eraser.
    pub fn draw_settings(&self, tool: Tool) -> &DrawSettings {
        match tool {
            Tool::Eraser => &self.eraser_draw_settings,
            Tool::Pencil | Tool::Draw | Tool::Fill | Tool::Eyedropper | Tool::Fieldier => {
                &self.pen_draw_settings
            }
        }
    }

    /// Mutable counterpart to [`Self::draw_settings`].
    pub fn draw_settings_mut(&mut self, tool: Tool) -> &mut DrawSettings {
        match tool {
            Tool::Eraser => &mut self.eraser_draw_settings,
            Tool::Pencil | Tool::Draw | Tool::Fill | Tool::Eyedropper | Tool::Fieldier => {
                &mut self.pen_draw_settings
            }
        }
    }
}

/// Close intent. The store still enforces the dirty guard before removal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseProject {
    Discard,
    Save,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseError {
    UnknownProject { project: ProjectId },
    Dirty { project: ProjectId },
    Cancelled { project: ProjectId },
    SaveRequired { project: ProjectId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivationToken {
    project: ProjectId,
    generation: u64,
}

impl ActivationToken {
    pub const fn project(self) -> ProjectId {
        self.project
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivationOutcome {
    token: Option<ActivationToken>,
    changed: bool,
}

impl ActivationOutcome {
    pub const fn token(self) -> Option<ActivationToken> {
        self.token
    }
    pub const fn changed(self) -> bool {
        self.changed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewProjectOutcome {
    project: ProjectId,
    activation: ActivationOutcome,
}

impl NewProjectOutcome {
    pub const fn project(self) -> ProjectId {
        self.project
    }
    pub const fn activation(self) -> ActivationOutcome {
        self.activation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CloseOutcome {
    closed: ProjectId,
    activation: Option<ActivationOutcome>,
}

impl CloseOutcome {
    pub const fn closed(self) -> ProjectId {
        self.closed
    }
    pub const fn activation(self) -> Option<ActivationOutcome> {
        self.activation
    }
}

/// Owns open sessions and resolves the active project for each frame.
pub struct ProjectStore {
    sessions: BTreeMap<ProjectId, ProjectSession>,
    active: Option<ProjectId>,
    next_id: u64,
    activation_generation: u64,
}

impl Default for ProjectStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectStore {
    pub const fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
            active: None,
            next_id: 1,
            activation_generation: 0,
        }
    }

    pub fn new_project(
        &mut self,
        name: impl Into<String>,
        width: usize,
        height: usize,
    ) -> NewProjectOutcome {
        let id = ProjectId::new(self.next_id);
        self.next_id += 1;
        self.sessions
            .insert(id, ProjectSession::new(id, name, width, height));
        let activation = self.activate_existing(id);
        NewProjectOutcome {
            project: id,
            activation,
        }
    }

    pub fn activate(&mut self, id: ProjectId) -> Result<ActivationOutcome, CloseError> {
        if !self.sessions.contains_key(&id) {
            return Err(CloseError::UnknownProject { project: id });
        }
        Ok(self.activate_existing(id))
    }

    fn activate_existing(&mut self, id: ProjectId) -> ActivationOutcome {
        let changed = self.active != Some(id);
        if changed {
            self.activation_generation = self.activation_generation.wrapping_add(1);
            self.active = Some(id);
        }
        ActivationOutcome {
            token: Some(ActivationToken {
                project: id,
                generation: self.activation_generation,
            }),
            changed,
        }
    }

    pub fn activation_token(&self) -> Option<ActivationToken> {
        self.active.map(|project| ActivationToken {
            project,
            generation: self.activation_generation,
        })
    }

    pub fn active_id(&self) -> Option<ProjectId> {
        self.active
    }
    pub fn contains(&self, id: ProjectId) -> bool {
        self.sessions.contains_key(&id)
    }
    pub fn tabs(&self) -> Vec<ProjectTab> {
        self.sessions
            .values()
            .map(|session| ProjectTab {
                id: session.id,
                name: session.name.clone(),
                active: self.active == Some(session.id),
                dirty: session.is_dirty(),
            })
            .collect()
    }
    pub fn current(&self) -> &ProjectSession {
        self.active
            .and_then(|id| self.sessions.get(&id))
            .expect("active project must resolve")
    }
    pub fn current_mut(&mut self) -> &mut ProjectSession {
        self.active
            .and_then(|id| self.sessions.get_mut(&id))
            .expect("active project must resolve")
    }
    pub fn session(&self, id: ProjectId) -> Option<&ProjectSession> {
        self.sessions.get(&id)
    }
    pub fn session_mut(&mut self, id: ProjectId) -> Option<&mut ProjectSession> {
        self.sessions.get_mut(&id)
    }
    pub fn find_by_path(&self, path: &std::path::Path) -> Option<ProjectId> {
        self.sessions
            .values()
            .find(|session| session.path.as_deref() == Some(path))
            .map(|session| session.id)
    }
    pub fn frame(&self) -> ProjectFrameContext<'_> {
        ProjectFrameContext {
            session: self.current(),
        }
    }
    pub fn frame_mut(&mut self) -> ProjectFrameMut<'_> {
        ProjectFrameMut {
            session: self.current_mut(),
        }
    }

    pub fn close(
        &mut self,
        id: ProjectId,
        intent: CloseProject,
    ) -> Result<CloseOutcome, CloseError> {
        let Some(session) = self.sessions.get(&id) else {
            return Err(CloseError::UnknownProject { project: id });
        };
        if intent == CloseProject::Cancel {
            return Err(CloseError::Cancelled { project: id });
        }
        if session.is_dirty() && intent == CloseProject::Discard {
            return Err(CloseError::Dirty { project: id });
        }
        if intent == CloseProject::Save {
            return Err(CloseError::SaveRequired { project: id });
        }
        self.sessions.remove(&id);
        let activation = if self.active == Some(id) {
            self.active = None;
            self.activation_generation = self.activation_generation.wrapping_add(1);
            let replacement = self
                .sessions
                .range(..id)
                .next_back()
                .map(|(project, _)| *project)
                .or_else(|| self.sessions.keys().next().copied());
            match replacement {
                Some(next) => Some(self.activate_existing(next)),
                None => Some(ActivationOutcome {
                    token: None,
                    changed: true,
                }),
            }
        } else {
            None
        };
        Ok(CloseOutcome {
            closed: id,
            activation,
        })
    }
}
