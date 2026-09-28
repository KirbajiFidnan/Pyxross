//! Structural undo commands + the `LayerStackController` facade (D59).
//!
//! Layer add/remove/reorder/property gestures are [`Command`]s on the same
//! [`UndoStack`] as strokes and fills.  Unlike [`ReverseDeltaCommand`], these
//! commands never store pixel bytes — they store the affected [`Layer`] object
//! (or its id and before/after property values) and restore the layer array by
//! object identity (D59).
//!
//! Transform commits ([`apply_layer_commits`], D32/D35/D36) are the exception:
//! a finished transform session is folded into ONE [`CompositeCommand`] of
//! [`ReverseDeltaCommand`] children — cut the lifted source rect, blit each
//! transformed commit onto the layer active at commit time — so the whole
//! gesture stays a single undo step and doubles as a free layer move.
//!
//! [`LayerStackController`] is the facade the UI talks to: every mutating
//! method follows the *can_apply → redo → push* discipline — it checks the
//! operation is applicable, applies it (the command's "done" state), then
//! pushes the command onto the shared [`UndoStack`].  One gesture = one undo
//! step.

use std::any::Any;

use crate::core::math::Rect2i;
use crate::core::model::{BlendMode, Layer, LayerId, LayerStack};
use crate::core::transform::LayerCommit;
use crate::core::undo::{
    Command, CommandContext, CompositeCommand, DeltaRecorder, ReverseDeltaCommand, UndoStack,
};

// ---------------------------------------------------------------------------
// LayerProperty
// ---------------------------------------------------------------------------

/// A single layer attribute that can be toggled/set and undone.
#[derive(Clone, PartialEq, Debug)]
pub enum LayerProperty {
    Visible(bool),
    Opacity(f32),
    Blend(BlendMode),
    Rename(String),
}

impl LayerProperty {
    fn apply(self, layers: &mut LayerStack, id: LayerId) -> bool {
        match self {
            LayerProperty::Visible(v) => layers.set_visible(id, v),
            LayerProperty::Opacity(o) => layers.set_opacity(id, o),
            LayerProperty::Blend(b) => layers.set_blend(id, b),
            LayerProperty::Rename(name) => layers.set_name(id, &name),
        }
    }
}

// ---------------------------------------------------------------------------
// LayerAddCommand
// ---------------------------------------------------------------------------

/// Undoable "add layer".  Stores the added [`Layer`] (with its id) and the
/// position it was inserted at, so undo removes it and redo restores the
/// exact same object identity.
pub struct LayerAddCommand {
    layer: Layer,
    position: usize,
    undone: bool,
}

impl LayerAddCommand {
    pub fn new(layer: Layer, position: usize) -> Self {
        Self {
            layer,
            position,
            undone: false,
        }
    }
}

impl Command for LayerAddCommand {
    fn name(&self) -> &str {
        "Add Layer"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        if context.layers.remove_layer(self.layer.id).is_none() {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        if !context
            .layers
            .insert_layer(self.layer.clone(), self.position)
        {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// LayerRemoveCommand
// ---------------------------------------------------------------------------

/// Undoable "remove layer".  Stores the removed [`Layer`] and its original
/// position; undo re-inserts it with the same id, redo removes it again.
pub struct LayerRemoveCommand {
    layer: Layer,
    position: usize,
    undone: bool,
}

impl LayerRemoveCommand {
    pub fn new(layer: Layer, position: usize) -> Self {
        Self {
            layer,
            position,
            undone: false,
        }
    }
}

impl Command for LayerRemoveCommand {
    fn name(&self) -> &str {
        "Remove Layer"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        if !context
            .layers
            .insert_layer(self.layer.clone(), self.position)
        {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        if context.layers.remove_layer(self.layer.id).is_none() {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// LayerReorderCommand
// ---------------------------------------------------------------------------

/// Undoable "move layer".  Stores the from/to indices; undo moves back to
/// `from`, redo moves to `to`.
pub struct LayerReorderCommand {
    id: LayerId,
    from: usize,
    to: usize,
    undone: bool,
}

impl LayerReorderCommand {
    pub fn new(id: LayerId, from: usize, to: usize) -> Self {
        Self {
            id,
            from,
            to,
            undone: false,
        }
    }
}

impl Command for LayerReorderCommand {
    fn name(&self) -> &str {
        "Reorder Layer"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        if !context.layers.reorder(self.id, self.from) {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        if !context.layers.reorder(self.id, self.to) {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// LayerPropertyCommand
// ---------------------------------------------------------------------------

/// Undoable visibility/opacity/blend change.  Stores the before/after values;
/// undo applies `before`, redo applies `after`.
pub struct LayerPropertyCommand {
    id: LayerId,
    before: LayerProperty,
    after: LayerProperty,
    undone: bool,
}

impl LayerPropertyCommand {
    pub fn new(id: LayerId, before: LayerProperty, after: LayerProperty) -> Self {
        Self {
            id,
            before,
            after,
            undone: false,
        }
    }

    /// The layer this command targets.
    pub fn id(&self) -> LayerId {
        self.id
    }

    /// The property value before the change (what undo restores).
    pub fn before(&self) -> LayerProperty {
        self.before.clone()
    }

    /// The property value after the change (what redo applies).
    pub fn after(&self) -> LayerProperty {
        self.after.clone()
    }

    /// Whether the command is currently in its undone state.
    pub fn is_undone(&self) -> bool {
        self.undone
    }

    /// Rewrite the `after` value (used by opacity-slider coalescing).
    pub fn set_after(&mut self, after: LayerProperty) {
        self.after = after;
    }
}

impl Command for LayerPropertyCommand {
    fn name(&self) -> &str {
        match &self.after {
            LayerProperty::Visible(_) => "Set Visibility",
            LayerProperty::Opacity(_) => "Set Opacity",
            LayerProperty::Blend(_) => "Set Blend",
            LayerProperty::Rename(_) => "Rename Layer",
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        if self.undone {
            return false;
        }
        if !self.before.clone().apply(context.layers, self.id) {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        if !self.after.clone().apply(context.layers, self.id) {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// LayerStackController
// ---------------------------------------------------------------------------

/// Facade for structural layer gestures (D59).
///
/// Each mutating method applies the operation to the [`LayerStack`] and pushes
/// exactly one undoable [`Command`] onto the shared [`UndoStack`] — the
/// *can_apply → redo → push* discipline.  Methods return `false` (and push
/// nothing) when the operation is not applicable: unknown id, no-op change,
/// or removing the last remaining layer.
pub struct LayerStackController<'a> {
    pub(crate) stack: &'a mut UndoStack,
}

impl<'a> LayerStackController<'a> {
    pub fn new(stack: &'a mut UndoStack) -> Self {
        Self { stack }
    }

    /// Appends a new layer on top and pushes an undoable "Add Layer" command.
    pub fn add_layer(&mut self, layers: &mut LayerStack, name: &str) -> Option<LayerId> {
        let id = layers.add_layer(name);
        let position = layers.len() - 1;
        let layer = layers
            .layer(id)
            .expect("just-added layer must exist")
            .clone();
        self.stack
            .push(Box::new(LayerAddCommand::new(layer, position)));
        Some(id)
    }

    /// Removes a layer and pushes an undoable "Remove Layer" command.
    /// Returns `false` when the id is unknown or it is the last layer.
    /// Removing a group routes through the subtree-removal command so undo
    /// restores the group's whole subtree.
    pub fn remove_layer(&mut self, layers: &mut LayerStack, id: LayerId) -> bool {
        let Some(layer) = layers.layer(id) else {
            return false;
        };
        if layer.is_group {
            return self.remove_subtree(layers, id);
        }
        let position = match layers.iter().position(|l| l.id == id) {
            Some(p) => p,
            None => return false,
        };
        let Some(layer) = layers.remove_layer(id) else {
            return false;
        };
        self.stack
            .push(Box::new(LayerRemoveCommand::new(layer, position)));
        true
    }

    /// Moves a layer to `new_index` and pushes an undoable "Reorder Layer"
    /// command.  Returns `false` when the move is rejected (unknown id, bad
    /// index, or no-op).
    pub fn move_layer(&mut self, layers: &mut LayerStack, id: LayerId, new_index: usize) -> bool {
        let Some(from) = layers.iter().position(|l| l.id == id) else {
            return false;
        };
        if !layers.reorder(id, new_index) {
            return false;
        }
        self.stack
            .push(Box::new(LayerReorderCommand::new(id, from, new_index)));
        true
    }

    /// Sets visibility and pushes an undoable command.  Returns `false` on
    /// unknown id or no-op.
    pub fn set_visible(&mut self, layers: &mut LayerStack, id: LayerId, visible: bool) -> bool {
        let Some(layer) = layers.layer(id) else {
            return false;
        };
        let before = layer.visible;
        if !layers.set_visible(id, visible) {
            return false;
        }
        self.stack.push(Box::new(LayerPropertyCommand::new(
            id,
            LayerProperty::Visible(before),
            LayerProperty::Visible(visible),
        )));
        true
    }

    /// Sets opacity (clamped to `0..=1`) and pushes an undoable command.
    /// Returns `false` on unknown id or no-op.
    pub fn set_opacity(&mut self, layers: &mut LayerStack, id: LayerId, opacity: f32) -> bool {
        let Some(layer) = layers.layer(id) else {
            return false;
        };
        let before = layer.opacity;
        if !layers.set_opacity(id, opacity) {
            return false;
        }
        self.stack.push(Box::new(LayerPropertyCommand::new(
            id,
            LayerProperty::Opacity(before),
            LayerProperty::Opacity(opacity),
        )));
        true
    }

    /// Sets opacity (clamped to `0..=1`) and pushes an undoable command; if the
    /// top of the undo stack is an un-undone `LayerPropertyCommand` for the SAME
    /// layer changing the SAME property (Opacity), the change coalesces into that
    /// command (its `after` is rewritten) so a continuous slider drag produces
    /// exactly ONE undo step. Returns false on unknown id or no-op.
    pub fn set_opacity_coalesced(
        &mut self,
        layers: &mut LayerStack,
        id: LayerId,
        opacity: f32,
    ) -> bool {
        let Some(layer) = layers.layer(id) else {
            return false;
        };
        let before = layer.opacity;
        if !layers.set_opacity(id, opacity) {
            return false;
        }
        // Read back the clamped value actually on the layer (set_opacity clamps
        // to 0..=1 and maps NaN to 0.0) — never trust the raw input.
        let applied = layers
            .layer(id)
            .expect("layer must still exist after a successful set_opacity")
            .opacity;

        let coalesced = match self.stack.peek_top_mut() {
            Some(top) => match top.as_any_mut().downcast_mut::<LayerPropertyCommand>() {
                Some(cmd)
                    if cmd.id() == id
                        && matches!(cmd.after(), LayerProperty::Opacity(_))
                        && !cmd.is_undone() =>
                {
                    cmd.set_after(LayerProperty::Opacity(applied));
                    true
                }
                _ => false,
            },
            None => false,
        };

        if coalesced {
            true
        } else {
            self.stack.push(Box::new(LayerPropertyCommand::new(
                id,
                LayerProperty::Opacity(before),
                LayerProperty::Opacity(applied),
            )));
            true
        }
    }

    /// Sets the blend mode and pushes an undoable command.  Returns `false`
    /// on unknown id or no-op.
    pub fn set_blend(&mut self, layers: &mut LayerStack, id: LayerId, blend: BlendMode) -> bool {
        let Some(layer) = layers.layer(id) else {
            return false;
        };
        let before = layer.blend;
        if !layers.set_blend(id, blend) {
            return false;
        }
        self.stack.push(Box::new(LayerPropertyCommand::new(
            id,
            LayerProperty::Blend(before),
            LayerProperty::Blend(blend),
        )));
        true
    }

    /// Sets the layer name and pushes an undoable "Rename Layer" command.
    /// Returns `false` on unknown id or no-op.
    pub fn set_name(&mut self, layers: &mut LayerStack, id: LayerId, name: String) -> bool {
        let Some(layer) = layers.layer(id) else {
            return false;
        };
        let old = layer.name.clone();
        if !layers.set_name(id, &name) {
            return false;
        }
        self.stack.push(Box::new(LayerPropertyCommand::new(
            id,
            LayerProperty::Rename(old),
            LayerProperty::Rename(name),
        )));
        true
    }

    /// Undo the top command on the shared stack.
    pub fn undo(&mut self, layers: &mut LayerStack) -> bool {
        let mut context = CommandContext { layers };
        self.stack.undo(&mut context)
    }

    /// Redo the top command on the shared stack.
    pub fn redo(&mut self, layers: &mut LayerStack) -> bool {
        let mut context = CommandContext { layers };
        self.stack.redo(&mut context)
    }

    pub fn can_undo(&self) -> bool {
        self.stack.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.stack.can_redo()
    }

    pub fn undo_len(&self) -> usize {
        self.stack.undo_len()
    }

    pub fn redo_len(&self) -> usize {
        self.stack.redo_len()
    }

    /// Name of the command at the top of the undo stack, for UI display.
    pub fn top_undo_name(&self) -> Option<&str> {
        self.stack.top_undo_name()
    }
}

// ---------------------------------------------------------------------------
// Transform commit (D32/D35/D36)
// ---------------------------------------------------------------------------

/// Undo label stamped on every transform-derived command — one gesture lands
/// on the undo stack as a single "Transform" step (D35).
pub const TRANSFORM_UNDO_NAME: &str = "Transform";

/// A lifted transform source: which layer the selection was lifted from, the
/// canvas rect it occupied, and the original pixels.  Enough to cut the
/// source back out at commit time.
pub struct TransformSource {
    /// Layer the selection was lifted from (the cut target).
    pub layer_id: LayerId,
    /// Canvas rect of the lifted region (clipped to the canvas at capture).
    pub rect: Rect2i,
    /// Original RGBA8 pixels of `rect`, `rect.w * rect.h * 4` bytes.
    /// Zero outside the mask when the lift carried one.
    pub snapshot: Vec<u8>,
    /// Row-major selection flags over `rect.area()` cells, `true` = selected.
    /// `None` when the lift had no mask (rectangular selection, or no
    /// selection at all); the cut and the paste then behave exactly like the
    /// unmasked path.
    pub mask: Option<Vec<bool>>,
}

/// Rounds a [`LayerCommit`]'s float top-left to an integer canvas rect.
fn commit_rect(commit: &LayerCommit) -> Rect2i {
    Rect2i::new(
        commit.dst.0.round() as i32,
        commit.dst.1.round() as i32,
        commit.w as i32,
        commit.h as i32,
    )
}

/// Clips a commit's destination rect to the canvas, returning the clipped
/// rect and its sub-buffer bytes.  Returns `None` when the commit lies fully
/// outside the canvas (nothing visible to write).
fn crop_commit(commit: &LayerCommit, canvas: Rect2i) -> Option<(Rect2i, Vec<u8>)> {
    let full = commit_rect(commit);
    let clipped = full.clamp_to(canvas);
    if clipped.is_empty() {
        return None;
    }
    let dx = (clipped.x - full.x) as usize;
    let dy = (clipped.y - full.y) as usize;
    let cw = clipped.w as usize;
    let ch = clipped.h as usize;
    let mut out = vec![0u8; cw * ch * 4];
    for row in 0..ch {
        let src = ((dy + row) * full.w as usize + dx) * 4;
        let dst = row * cw * 4;
        out[dst..dst + cw * 4].copy_from_slice(&commit.buf[src..src + cw * 4]);
    }
    Some((clipped, out))
}

/// Applies a finished transform session as ONE undoable "Transform" gesture
/// (D35): the source rect is cut from `source.layer_id`, then each transformed
/// commit is blitted onto `target_layer` — the layer active at commit time —
/// so switching layers mid-gesture becomes a free layer move
/// (FEATURES.md §3.4 / D36).  The `usize` mini-stack ids inside
/// [`LayerCommit`] stay internal to the session; the UI maps them onto
/// `target_layer`.
///
/// Returns `None` when nothing changed (fully transparent source, or an
/// identity transform pasted back onto the source layer), mirroring
/// `move_selection_to_command`.
pub fn apply_layer_commits(
    source: &TransformSource,
    commits: &[LayerCommit],
    target_layer: LayerId,
    layers: &mut LayerStack,
) -> Option<CompositeCommand> {
    apply_layer_commits_with(source, commits, target_layer, layers, true)
}

/// Mask-aware variant of [`apply_layer_commits`]: when `cut_source` is false
/// the lifted source rect is left untouched, so the transform pastes a COPY of
/// the selected pixels (Alt+drag) instead of moving the originals.
pub fn apply_layer_commits_with(
    source: &TransformSource,
    commits: &[LayerCommit],
    target_layer: LayerId,
    layers: &mut LayerStack,
    cut_source: bool,
) -> Option<CompositeCommand> {
    let canvas = Rect2i::new(0, 0, layers.width() as i32, layers.height() as i32);
    debug_assert_eq!(source.snapshot.len(), source.rect.area() as usize * 4);

    let mut composite = CompositeCommand::new(TRANSFORM_UNDO_NAME);

    // Identity guard: pasting the source back onto itself, byte for byte at
    // the same rect, changes nothing — avoid polluting the undo stack.
    if cut_source && target_layer == source.layer_id {
        if let [commit] = commits {
            if let Some((rect, bytes)) = crop_commit(commit, canvas) {
                if rect == source.rect && bytes == source.snapshot {
                    return None;
                }
            }
        }
    }

    // Cut: zero the lifted source rect on its layer.  Skip when the source is
    // already fully transparent, so a pure no-op gesture leaves no entry.  A
    // masked lift only clears the selected cells — the snapshot already
    // carries zeros outside the mask, and a full-rect clear would erase
    // pixels the selection never covered.
    if cut_source && source.snapshot.iter().any(|&b| b != 0) {
        if let Some(layer) = layers.layer_mut(source.layer_id) {
            // The cut's after-state doubles as the blit source: with a mask
            // only the selected cells are zeroed (and written), so the delta's
            // redo reproduces the cut instead of erasing masked-out cells.
            let after = match &source.mask {
                Some(mask) if mask.len() == source.rect.area() as usize => {
                    let mut after = source.snapshot.clone();
                    for (i, px) in after.chunks_exact_mut(4).enumerate() {
                        if mask[i] {
                            px.copy_from_slice(&[0u8; 4]);
                        }
                    }
                    after
                }
                _ => vec![0u8; source.snapshot.len()],
            };
            let cleared = match &source.mask {
                Some(mask) => layer
                    .buffer
                    .blit_region_masked(source.rect, &after, Some(mask)),
                None => layer.buffer.blit_region(source.rect, &after),
            };
            if cleared {
                composite.push(Box::new(ReverseDeltaCommand::new(
                    TRANSFORM_UNDO_NAME,
                    source.layer_id,
                    source.rect,
                    source.snapshot.clone(),
                    after,
                )));
            }
        }
    }

    // Paste: blit the in-canvas part of every transformed commit onto the
    // target layer, each captured as its own delta child.
    for commit in commits {
        let Some((rect, bytes)) = crop_commit(commit, canvas) else {
            continue; // fully off-canvas: nothing visible to write.
        };
        let Some(layer) = layers.layer_mut(target_layer) else {
            continue;
        };
        let Some(recorder) =
            DeltaRecorder::begin(TRANSFORM_UNDO_NAME, target_layer, &layer.buffer, rect)
        else {
            continue;
        };
        // A masked lift pastes only the selected cells too: the lifted
        // buffer's zeros outside the mask must never reach the destination.
        let pasted = match &source.mask {
            Some(mask) => layer.buffer.blit_region_masked(rect, &bytes, Some(mask)),
            None => layer.buffer.blit_region(rect, &bytes),
        };
        if pasted && !recorder.is_empty_delta(&layer.buffer) {
            composite.push(Box::new(recorder.finish(&layer.buffer)));
        }
    }

    if composite.is_empty() {
        return None;
    }
    Some(composite)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::buffer::PixelBuffer;
    use crate::core::color::Color;
    use crate::core::transform::{LayerBuffer, TransformObject};

    fn ids(layers: &LayerStack) -> Vec<LayerId> {
        layers.iter().map(|l| l.id).collect()
    }

    #[test]
    fn add_undo_redo_round_trip() {
        let mut layers = LayerStack::new(4, 4);
        let mut stack = UndoStack::new();
        let original = layers.active_layer_id();

        let mut ctl = LayerStackController::new(&mut stack);
        let added = ctl.add_layer(&mut layers, "New").unwrap();
        assert_eq!(layers.len(), 2);
        assert_eq!(ctl.top_undo_name(), Some("Add Layer"));

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.len(), 1);
        assert_eq!(layers.layer(added), None);
        assert_eq!(layers.active_layer_id(), original);

        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.len(), 2);
        let restored = layers.layer(added).unwrap();
        assert_eq!(restored.name, "New");
        assert_eq!(restored.id, added);
        assert_eq!(ids(&layers), vec![original, added]);
    }

    #[test]
    fn add_undo_redo_restores_layer_state_at_add_time() {
        let mut layers = LayerStack::new(2, 2);
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);
        let added = ctl.add_layer(&mut layers, "Painted").unwrap();
        layers
            .layer_mut(added)
            .unwrap()
            .buffer
            .fill(Color::rgb(10, 20, 30));

        assert!(ctl.undo(&mut layers));
        assert!(ctl.redo(&mut layers));
        assert_eq!(
            layers.layer(added).unwrap().buffer.as_bytes(),
            vec![0, 0, 0, 0].repeat(4)
        );
    }

    #[test]
    fn remove_undo_restores_layer_at_position() {
        let mut layers = LayerStack::new(2, 2);
        let a = layers.active_layer_id();
        let b = layers.add_layer("B");
        let c = layers.add_layer("C");
        layers
            .layer_mut(b)
            .unwrap()
            .buffer
            .fill(Color::rgb(1, 2, 3));
        let mut stack = UndoStack::new();

        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.remove_layer(&mut layers, b));
        assert_eq!(ids(&layers), vec![a, c]);
        assert_eq!(ctl.top_undo_name(), Some("Remove Layer"));

        assert!(ctl.undo(&mut layers));
        assert_eq!(ids(&layers), vec![a, b, c]);
        assert_eq!(
            layers.layer(b).unwrap().buffer.as_bytes(),
            vec![1, 2, 3, 255].repeat(4)
        );

        assert!(ctl.redo(&mut layers));
        assert_eq!(ids(&layers), vec![a, c]);
    }

    #[test]
    fn remove_last_layer_is_rejected_without_push() {
        let mut layers = LayerStack::new(2, 2);
        let a = layers.active_layer_id();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(!ctl.remove_layer(&mut layers, a));
        assert_eq!(layers.len(), 1);
        assert!(!ctl.can_undo());
    }

    #[test]
    fn remove_unknown_layer_is_rejected_without_push() {
        let mut layers = LayerStack::new(2, 2);
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(!ctl.remove_layer(&mut layers, LayerId::new(999)));
        assert!(!ctl.can_undo());
    }

    #[test]
    fn reorder_undo_redo() {
        let mut layers = LayerStack::new(1, 1);
        let a = layers.active_layer_id();
        let b = layers.add_layer("B");
        let c = layers.add_layer("C");
        let mut stack = UndoStack::new();

        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.move_layer(&mut layers, a, 2));
        assert_eq!(ids(&layers), vec![b, c, a]);

        assert!(ctl.undo(&mut layers));
        assert_eq!(ids(&layers), vec![a, b, c]);

        assert!(ctl.redo(&mut layers));
        assert_eq!(ids(&layers), vec![b, c, a]);
    }

    #[test]
    fn reorder_noop_is_rejected_without_push() {
        let mut layers = LayerStack::new(1, 1);
        let a = layers.active_layer_id();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(!ctl.move_layer(&mut layers, a, 0));
        assert!(!ctl.can_undo());
    }

    #[test]
    fn property_visible_undo_redo() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();

        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.set_visible(&mut layers, id, false));
        assert!(!layers.layer(id).unwrap().visible);
        assert_eq!(ctl.top_undo_name(), Some("Set Visibility"));

        assert!(ctl.undo(&mut layers));
        assert!(layers.layer(id).unwrap().visible);

        assert!(ctl.redo(&mut layers));
        assert!(!layers.layer(id).unwrap().visible);
    }

    #[test]
    fn property_opacity_undo_redo() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();

        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.set_opacity(&mut layers, id, 0.5));
        assert_eq!(layers.layer(id).unwrap().opacity, 0.5);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().opacity, 1.0);

        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().opacity, 0.5);
    }

    #[test]
    fn property_blend_undo_redo() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();

        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.set_blend(&mut layers, id, BlendMode::Multiply));
        assert_eq!(layers.layer(id).unwrap().blend, BlendMode::Multiply);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().blend, BlendMode::Normal);

        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().blend, BlendMode::Multiply);
    }

    #[test]
    fn noop_setters_do_not_push() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(!ctl.set_visible(&mut layers, id, true));
        assert!(!ctl.set_opacity(&mut layers, id, 1.0));
        assert!(!ctl.set_blend(&mut layers, id, BlendMode::Normal));
        assert!(!ctl.can_undo());
    }

    #[test]
    fn unknown_layer_property_ops_are_rejected_without_push() {
        let mut layers = LayerStack::new(1, 1);
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(!ctl.set_visible(&mut layers, LayerId::new(999), false));
        assert!(!ctl.set_opacity(&mut layers, LayerId::new(999), 0.5));
        assert!(!ctl.set_blend(&mut layers, LayerId::new(999), BlendMode::Add));
        assert!(!ctl.can_undo());
    }

    #[test]
    fn controller_undo_redo_passthrough() {
        let mut layers = LayerStack::new(1, 1);
        let a = layers.active_layer_id();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        let b = ctl.add_layer(&mut layers, "B").unwrap();
        assert!(ctl.set_visible(&mut layers, b, false));
        assert_eq!(ctl.undo_len(), 2);

        assert!(ctl.undo(&mut layers));
        assert!(layers.layer(b).unwrap().visible);
        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.len(), 1);
        assert_eq!(layers.active_layer_id(), a);

        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.len(), 2);
        assert!(ctl.redo(&mut layers));
        assert!(!layers.layer(b).unwrap().visible);
    }

    #[test]
    fn structural_and_stroke_commands_share_stack() {
        use crate::core::brush::{BrushShape, BrushSpec, DrawMode, Stroke};
        use crate::core::math::Rect2i;
        use crate::core::stroke_command::StrokeSession;

        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let mut stack = UndoStack::new();

        let spec = BrushSpec::new(1, BrushShape::Square);
        let stroke = Stroke::new(spec, DrawMode::Pen, Color::WHITE);
        let canvas = Rect2i::new(0, 0, 8, 8);
        let mut session =
            StrokeSession::begin(&layers.active_layer().buffer, stroke, canvas, lid).unwrap();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            session.stroke_mut().start(buf, 2, 2);
        }
        stack.push(Box::new(
            session.finish(&layers.active_layer().buffer).unwrap(),
        ));

        let mut ctl = LayerStackController::new(&mut stack);
        let added = ctl.add_layer(&mut layers, "New").unwrap();
        assert_eq!(ctl.undo_len(), 2);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.layer(added), None);
        assert!(ctl.undo(&mut layers));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 2),
            Some(Color::TRANSPARENT)
        );

        assert!(ctl.redo(&mut layers));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 2),
            Some(Color::WHITE)
        );
        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.layer(added).unwrap().name, "New");
    }

    #[test]
    fn remove_undo_after_later_add_keeps_identity() {
        let mut layers = LayerStack::new(2, 2);
        let a = layers.active_layer_id();
        let b = layers.add_layer("B");
        let mut stack = UndoStack::new();

        let mut ctl = LayerStackController::new(&mut stack);
        assert!(ctl.remove_layer(&mut layers, b));
        let c = ctl.add_layer(&mut layers, "C").unwrap();
        assert_ne!(c, b);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.layer(c), None);
        assert!(ctl.undo(&mut layers));
        assert_eq!(ids(&layers), vec![a, b]);
        assert_eq!(layers.layer(b).unwrap().name, "B");
    }

    #[test]
    fn coalesce_happy_path() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.9));
        assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.5));
        assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.3));
        assert_eq!(ctl.undo_len(), 1);
        assert_eq!(layers.layer(id).unwrap().opacity, 0.3);

        assert!(ctl.undo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().opacity, 1.0);

        assert!(ctl.redo(&mut layers));
        assert_eq!(layers.layer(id).unwrap().opacity, 0.3);
    }

    #[test]
    fn accessor_values() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();
        {
            let mut ctl = LayerStackController::new(&mut stack);
            assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.8));
            assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.4));
            assert!(ctl.set_opacity(&mut layers, id, 0.6));
        }

        // Top command is the plain set: before = pre-change 0.4, after = 0.6.
        let top = stack.peek_top().unwrap();
        let cmd = top.as_any().downcast_ref::<LayerPropertyCommand>().unwrap();
        assert_eq!(cmd.id(), id);
        assert_eq!(cmd.before(), LayerProperty::Opacity(0.4));
        assert_eq!(cmd.after(), LayerProperty::Opacity(0.6));
        assert!(!cmd.is_undone());

        // Undo moves the plain set to the redo stack; the coalesced command
        // (before = pre-drag 1.0, after = 0.4) is now on top of the undo stack.
        assert!(stack.undo(&mut CommandContext {
            layers: &mut layers
        }));
        let top = stack.peek_top().unwrap();
        let cmd = top.as_any().downcast_ref::<LayerPropertyCommand>().unwrap();
        assert_eq!(cmd.before(), LayerProperty::Opacity(1.0));
        assert_eq!(cmd.after(), LayerProperty::Opacity(0.4));
        assert!(!cmd.is_undone());

        // is_undone() flips through undo/redo on a directly-driven command.
        let mut direct =
            LayerPropertyCommand::new(id, LayerProperty::Opacity(1.0), LayerProperty::Opacity(0.5));
        let mut context = CommandContext {
            layers: &mut layers,
        };
        assert!(direct.undo(&mut context));
        assert!(direct.is_undone());
        assert!(direct.redo(&mut context));
        assert!(!direct.is_undone());
    }

    #[test]
    fn no_coalesce_on_other_layer() {
        let mut layers = LayerStack::new(1, 1);
        let a = layers.active_layer_id();
        let b = layers.add_layer("B");
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        // A's drag coalesces into one command.
        assert!(ctl.set_opacity_coalesced(&mut layers, a, 0.5));
        assert!(ctl.set_opacity_coalesced(&mut layers, a, 0.4));
        // B's drag must NOT coalesce into A's command.
        assert!(ctl.set_opacity_coalesced(&mut layers, b, 0.6));
        assert!(ctl.set_opacity_coalesced(&mut layers, b, 0.3));
        assert_eq!(ctl.undo_len(), 2);
    }

    #[test]
    fn no_coalesce_on_undone_top() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();
        {
            let mut ctl = LayerStackController::new(&mut stack);
            assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.5));
            assert!(ctl.undo(&mut layers));
            assert_eq!(layers.layer(id).unwrap().opacity, 1.0);

            // The undone command sits on the redo stack; a fresh command must
            // be pushed instead of coalescing into it (push clears redo).
            assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.3));
            assert_eq!(ctl.undo_len(), 1);
            assert_eq!(ctl.redo_len(), 0);
        }

        // The fresh command is a full command: before = pre-change 1.0,
        // after = 0.3 — it did not coalesce into the undone command.
        let top = stack.peek_top().unwrap();
        let cmd = top.as_any().downcast_ref::<LayerPropertyCommand>().unwrap();
        assert_eq!(cmd.before(), LayerProperty::Opacity(1.0));
        assert_eq!(cmd.after(), LayerProperty::Opacity(0.3));
        assert!(!cmd.is_undone());

        // Undoing it returns to the original opacity.
        assert!(stack.undo(&mut CommandContext {
            layers: &mut layers
        }));
        assert_eq!(layers.layer(id).unwrap().opacity, 1.0);
    }

    #[test]
    fn no_coalesce_across_property() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(ctl.set_visible(&mut layers, id, false));
        assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.5));
        assert_eq!(ctl.undo_len(), 2);
    }

    #[test]
    fn unknown_layer_returns_false() {
        let mut layers = LayerStack::new(1, 1);
        let mut stack = UndoStack::new();
        let mut ctl = LayerStackController::new(&mut stack);

        assert!(!ctl.set_opacity_coalesced(&mut layers, LayerId::new(999), 0.5));
        assert_eq!(ctl.undo_len(), 0);
    }

    #[test]
    fn interleaved_with_plain_set_opacity() {
        let mut layers = LayerStack::new(1, 1);
        let id = layers.active_layer_id();
        let mut stack = UndoStack::new();
        {
            let mut ctl = LayerStackController::new(&mut stack);
            // Coalesced drag: 1.0 -> 0.9 -> 0.5 (one command, before = 1.0, after = 0.5).
            assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.9));
            assert!(ctl.set_opacity_coalesced(&mut layers, id, 0.5));
            // Plain set: 0.5 -> 0.7 (new command, before = 0.5, after = 0.7).
            assert!(ctl.set_opacity(&mut layers, id, 0.7));
            assert_eq!(ctl.undo_len(), 2);
        }

        let top = stack.peek_top().unwrap();
        let cmd = top.as_any().downcast_ref::<LayerPropertyCommand>().unwrap();
        assert_eq!(cmd.before(), LayerProperty::Opacity(0.5));
        assert_eq!(cmd.after(), LayerProperty::Opacity(0.7));

        assert!(stack.undo(&mut CommandContext {
            layers: &mut layers
        }));
        let top = stack.peek_top().unwrap();
        let cmd = top.as_any().downcast_ref::<LayerPropertyCommand>().unwrap();
        assert_eq!(cmd.before(), LayerProperty::Opacity(1.0));
        assert_eq!(cmd.after(), LayerProperty::Opacity(0.5));
    }

    // --- transform commit (D32/D35/D36) ---

    fn ctx(layers: &mut LayerStack) -> CommandContext<'_> {
        CommandContext { layers }
    }

    /// Deterministic per-pixel pattern fill (same formula as select.rs).
    fn pattern_fill(buf: &mut PixelBuffer) {
        let w = buf.width();
        let h = buf.height();
        for y in 0..h {
            for x in 0..w {
                let r = ((x * 17 + y * 31) & 0xFF) as u8;
                let g = ((x * 13 + y * 23) & 0xFF) as u8;
                let b = ((x * 11 + y * 37) & 0xFF) as u8;
                buf.set_pixel(x, y, Color::rgba(r, g, b, 255));
            }
        }
    }

    #[test]
    fn transform_commit_round_trip_via_undo_stack() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();

        let rect = Rect2i::new(1, 1, 2, 2);
        let snapshot = layers
            .active_layer()
            .buffer
            .export_region(rect, None)
            .unwrap();
        let source = TransformSource {
            layer_id: lid,
            rect,
            snapshot: snapshot.clone(),
            mask: None,
        };

        // Real pipeline: lift → rotate 90° (square bbox stays in place) → commit.
        let mut obj = TransformObject::lift(
            LayerBuffer {
                layer_id: 0,
                w: 2,
                h: 2,
                buf: snapshot,
            },
            (1.0, 1.0),
        );
        obj.rotate_by(std::f32::consts::FRAC_PI_2);
        let commits = obj.commit();
        assert_eq!(commits.len(), 1);
        assert_eq!((commits[0].w, commits[0].h), (2, 2));
        let original_rect = layers
            .active_layer()
            .buffer
            .export_region(rect, None)
            .unwrap();
        assert_ne!(commits[0].buf, original_rect);

        let mut cmd = apply_layer_commits(&source, &commits, lid, &mut layers).unwrap();
        let transformed = layers.active_layer().buffer.as_bytes().to_vec();
        assert_ne!(transformed, original);

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &transformed[..]);
    }

    #[test]
    fn transform_commit_to_target_layer_moves_pixels() {
        let mut layers = LayerStack::new(4, 4);
        let src = layers.active_layer_id();
        let dst = layers.add_layer("Target");
        pattern_fill(&mut layers.layer_mut(src).unwrap().buffer);

        let rect = Rect2i::new(1, 1, 2, 2);
        let snapshot = layers
            .layer(src)
            .unwrap()
            .buffer
            .export_region(rect, None)
            .unwrap();
        let source = TransformSource {
            layer_id: src,
            rect,
            snapshot: snapshot.clone(),
            mask: None,
        };

        // Identity transform committed onto a DIFFERENT layer = free layer move.
        let obj = TransformObject::lift(
            LayerBuffer {
                layer_id: 0,
                w: 2,
                h: 2,
                buf: snapshot,
            },
            (1.0, 1.0),
        );
        let commits = obj.commit();
        let mut cmd = apply_layer_commits(&source, &commits, dst, &mut layers).unwrap();

        assert_eq!(
            layers.layer(src).unwrap().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(
            layers.layer(dst).unwrap().buffer.get_pixel(1, 1),
            Some(Color::rgba(48, 36, 48, 255))
        );

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.layer(src).unwrap().buffer.get_pixel(1, 1),
            Some(Color::rgba(48, 36, 48, 255))
        );
        assert_eq!(
            layers.layer(dst).unwrap().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
    }

    #[test]
    fn transform_commit_fully_off_canvas_cuts_source() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();

        let rect = Rect2i::new(1, 1, 2, 2);
        let snapshot = layers
            .active_layer()
            .buffer
            .export_region(rect, None)
            .unwrap();
        let source = TransformSource {
            layer_id: lid,
            rect,
            snapshot: snapshot.clone(),
            mask: None,
        };

        // Lifted at (10, 10): the 2x2 commit lands fully outside the canvas.
        let obj = TransformObject::lift(
            LayerBuffer {
                layer_id: 0,
                w: 2,
                h: 2,
                buf: snapshot,
            },
            (10.0, 10.0),
        );
        let commits = obj.commit();
        let mut cmd = apply_layer_commits(&source, &commits, lid, &mut layers).unwrap();

        // Cut semantics: the gesture moved the pixels fully off-canvas, so the
        // source rect is now transparent and nothing was pasted.
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
        assert_ne!(layers.active_layer().buffer.as_bytes(), &original[..]);

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_ne!(layers.active_layer().buffer.as_bytes(), &original[..]);
    }

    #[test]
    fn transform_commit_noop_returns_none() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();

        // Fully transparent source: cut skipped, paste invisible → None.
        let rect = Rect2i::new(1, 1, 2, 2);
        let snapshot = layers
            .active_layer()
            .buffer
            .export_region(rect, None)
            .unwrap();
        assert!(snapshot.iter().all(|&b| b == 0));
        let source = TransformSource {
            layer_id: lid,
            rect,
            snapshot: snapshot.clone(),
            mask: None,
        };
        let mut obj = TransformObject::lift(
            LayerBuffer {
                layer_id: 0,
                w: 2,
                h: 2,
                buf: snapshot,
            },
            (1.0, 1.0),
        );
        obj.rotate_by(std::f32::consts::FRAC_PI_2);
        let commits = obj.commit();
        assert!(apply_layer_commits(&source, &commits, lid, &mut layers).is_none());

        // Identity transform pasted back onto the source layer → None, and the
        // buffer is untouched.
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let rect = Rect2i::new(1, 1, 2, 2);
        let snapshot = layers
            .active_layer()
            .buffer
            .export_region(rect, None)
            .unwrap();
        let source = TransformSource {
            layer_id: lid,
            rect,
            snapshot: snapshot.clone(),
            mask: None,
        };
        let obj = TransformObject::lift(
            LayerBuffer {
                layer_id: 0,
                w: 2,
                h: 2,
                buf: snapshot,
            },
            (1.0, 1.0),
        );
        let commits = obj.commit();
        let expect = layers.active_layer().buffer.as_bytes().to_vec();
        assert!(apply_layer_commits(&source, &commits, lid, &mut layers).is_none());
        assert_eq!(layers.active_layer().buffer.as_bytes(), &expect[..]);
    }

    #[test]
    fn transform_commit_masked_cut_only_clears_selected_cells() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let rect = Rect2i::new(1, 1, 2, 2);
        let snapshot = layers
            .active_layer()
            .buffer
            .export_region(rect, None)
            .unwrap();
        // Row-major mask: cells (1,1) and (2,2) selected, (2,1) and (1,2) not.
        let source = TransformSource {
            layer_id: lid,
            rect,
            snapshot: snapshot.clone(),
            mask: Some(vec![true, false, false, true]),
        };
        // Lifted far off-canvas: the commit lands outside, so only the cut
        // runs — and with a mask it may only clear the selected cells.
        let obj = TransformObject::lift(
            LayerBuffer {
                layer_id: 0,
                w: 2,
                h: 2,
                buf: snapshot,
            },
            (10.0, 10.0),
        );
        let commits = obj.commit();
        let mut cmd = apply_layer_commits(&source, &commits, lid, &mut layers).unwrap();

        // Selected cells were cut to transparent.
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 2),
            Some(Color::TRANSPARENT)
        );
        // Masked-out cells keep their original pattern pixels, through both
        // the cut and the undo (the delta never covered them).
        let kept_21 = layers.active_layer().buffer.get_pixel(2, 1);
        let kept_12 = layers.active_layer().buffer.get_pixel(1, 2);
        assert_eq!(kept_21, Some(Color::rgba(65, 49, 59, 255)));
        assert_eq!(kept_12, Some(Color::rgba(79, 59, 85, 255)));
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.get_pixel(2, 1), kept_21);
        assert_eq!(layers.active_layer().buffer.get_pixel(1, 2), kept_12);
        // Redo reproduces exactly the cut: selected cells zeroed, the rest
        // untouched.
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(layers.active_layer().buffer.get_pixel(2, 1), kept_21);
    }

    #[test]
    fn transform_commit_masked_paste_skips_unselected_cells() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let rect = Rect2i::new(1, 1, 2, 2);
        let snapshot = layers
            .active_layer()
            .buffer
            .export_region(rect, None)
            .unwrap();
        // Row-major mask selecting cells (1,1) and (1,2); the lifted buffer
        // zeroes the other two, like export_region with the mask does.
        let mut lifted = snapshot.clone();
        lifted[4..8].copy_from_slice(&[0u8; 4]);
        lifted[12..16].copy_from_slice(&[0u8; 4]);
        let source = TransformSource {
            layer_id: lid,
            rect,
            snapshot: snapshot.clone(),
            mask: Some(vec![true, false, true, false]),
        };
        let kept_21 = layers.active_layer().buffer.get_pixel(2, 1);
        let kept_22 = layers.active_layer().buffer.get_pixel(2, 2);

        // Rotate 180° about the source centre: the commit lands back on the
        // source rect, carrying the lifted zeros toward the masked-out cells.
        let mut obj = TransformObject::lift(
            LayerBuffer {
                layer_id: 0,
                w: 2,
                h: 2,
                buf: lifted,
            },
            (1.0, 1.0),
        );
        obj.rotate_by(std::f32::consts::PI);
        let commits = obj.commit();
        let mut cmd = apply_layer_commits(&source, &commits, lid, &mut layers).unwrap();

        // Masked-out destination cells never saw the paste.
        assert_eq!(layers.active_layer().buffer.get_pixel(2, 1), kept_21);
        assert_eq!(layers.active_layer().buffer.get_pixel(2, 2), kept_22);
        // Selected cells were cut, then pasted from the lifted zeros.
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 2),
            Some(Color::TRANSPARENT)
        );
        // Undo restores the document byte-for-byte.
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(layers.active_layer().buffer.get_pixel(2, 1), kept_21);
    }

    #[test]
    fn crop_commit_clips_and_offsets_bytes() {
        // Commit occupies (-1, 0, 3, 2): the left column hangs off-canvas on a
        // 4x4 canvas.  Row 0 = pixels 1,2,3; row 1 = 4,5,6 (RGBA each).
        let mut buf = Vec::new();
        for v in 1..=6u8 {
            buf.extend_from_slice(&[v, v, v, 255]);
        }
        let commit = LayerCommit {
            layer_id: 0,
            dst: (-1.0, 0.0),
            w: 3,
            h: 2,
            buf,
        };
        let (rect, bytes) = crop_commit(&commit, Rect2i::new(0, 0, 4, 4)).unwrap();

        assert_eq!(rect, Rect2i::new(0, 0, 2, 2));
        let mut expect = Vec::new();
        for v in [2u8, 3, 5, 6] {
            expect.extend_from_slice(&[v, v, v, 255]);
        }
        assert_eq!(bytes, expect);
    }

    #[test]
    fn crop_commit_fully_off_canvas_returns_none() {
        let commit = LayerCommit {
            layer_id: 0,
            dst: (10.0, 10.0),
            w: 2,
            h: 2,
            buf: vec![0u8; 16],
        };
        assert!(crop_commit(&commit, Rect2i::new(0, 0, 4, 4)).is_none());
    }
}
