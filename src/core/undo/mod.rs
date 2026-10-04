//! Undo system — reverse-delta commands, composite commands, unbounded stack.
//!
//! Every mutation is captured as a reverse-delta: the before and after RGBA8
//! bytes of the affected region of a specific layer.  One undo step corresponds
//! to one user gesture (stroke, fill, move).  `CompositeCommand` groups child
//! commands so that a single gesture that touches multiple layers or regions
//! still rolls back in one step.  The `UndoStack` is unbounded (no capacity
//! limit) per D13/D45.

use std::any::Any;

use crate::core::buffer::PixelBuffer;
use crate::core::math::Rect2i;
use crate::core::model::{LayerId, LayerStack};
use crate::core::tile_edit;
use crate::core::tilemap::{TileCell, TileId, TilePalette};

// ---------------------------------------------------------------------------
// CommandContext
// ---------------------------------------------------------------------------

/// Read-only consult context given to every command's undo/redo.
/// Commands reach the layer stack through here — never through a bare buffer.
pub struct CommandContext<'a> {
    pub layers: &'a mut LayerStack,
    /// The project's tile palette, mutable — used by the integrated tile
    /// model's write-back commands (editing a canvas cell writes through to
    /// the ROOT tile data, so undo/redo restore the root bytes and re-bake
    /// every instance).
    pub palette: &'a mut TilePalette,
}

impl<'a> CommandContext<'a> {
    /// Build a context from a layer stack and a palette.
    pub fn new(layers: &'a mut LayerStack, palette: &'a mut TilePalette) -> Self {
        Self { layers, palette }
    }
}

// ---------------------------------------------------------------------------
// Command trait
// ---------------------------------------------------------------------------

/// A reversible mutation of the document's layer stack.
///
/// `name()` returns a static label for the UI (e.g. "Stroke", "Fill").
/// `undo` / `redo` receive a [`CommandContext`] through which they reach the
/// [`LayerStack`] — commands never touch a bare buffer directly.  They return
/// `true` on success, `false` when the command is already in the target state
/// (idempotent no-op), the layer it targets no longer exists, or the region is
/// invalid.
pub trait Command: Any {
    fn name(&self) -> &str;
    fn undo(&mut self, context: &mut CommandContext) -> bool;
    fn redo(&mut self, context: &mut CommandContext) -> bool;
    /// Type-erased access for downcasting (undo-coalescing in layer_commands.rs).
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

// ---------------------------------------------------------------------------
// ReverseDeltaCommand
// ---------------------------------------------------------------------------

/// Stores before/after RGBA8 bytes for a rectangular region of one layer and
/// replays them onto that layer's buffer.
pub struct ReverseDeltaCommand {
    name: &'static str,
    layer: LayerId,
    region: Rect2i,
    before: Vec<u8>,
    after: Vec<u8>,
    undone: bool,
}

impl ReverseDeltaCommand {
    /// Build from caller-supplied before/after byte slices.
    pub fn new(
        name: &'static str,
        layer: LayerId,
        region: Rect2i,
        before: Vec<u8>,
        after: Vec<u8>,
    ) -> Self {
        Self {
            name,
            layer,
            region,
            before,
            after,
            undone: false,
        }
    }

    /// Snapshot `after` from the current buffer state at `region`.
    pub fn from_recorder(
        name: &'static str,
        layer: LayerId,
        region: Rect2i,
        before: &[u8],
        buf: &PixelBuffer,
    ) -> Self {
        let after = capture_region(buf, region);
        Self {
            name,
            layer,
            region,
            before: before.to_vec(),
            after,
            undone: false,
        }
    }

    /// The canvas region this command touches.
    pub fn region(&self) -> Rect2i {
        self.region
    }

    /// The layer this command targets.
    pub fn layer_id(&self) -> LayerId {
        self.layer
    }

    /// The region's before bytes.
    pub fn before(&self) -> &[u8] {
        &self.before
    }

    /// The region's after bytes.
    pub fn after(&self) -> &[u8] {
        &self.after
    }
}

impl Command for ReverseDeltaCommand {
    fn name(&self) -> &str {
        self.name
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
        let Some(layer) = context.layers.layer_mut(self.layer) else {
            return false;
        };
        if !layer.buffer.blit_region(self.region, &self.before) {
            return false;
        }
        self.undone = true;
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        if !self.undone {
            return false;
        }
        let Some(layer) = context.layers.layer_mut(self.layer) else {
            return false;
        };
        if !layer.buffer.blit_region(self.region, &self.after) {
            return false;
        }
        self.undone = false;
        true
    }
}

// ---------------------------------------------------------------------------
// CompositeCommand
// ---------------------------------------------------------------------------

/// Groups multiple commands into one undo step (e.g. a gesture that paints on
/// several layers).  Undo replays children in reverse order; redo in forward.
pub struct CompositeCommand {
    name: &'static str,
    commands: Vec<Box<dyn Command>>,
}

impl CompositeCommand {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            commands: Vec::new(),
        }
    }

    pub fn push(&mut self, cmd: Box<dyn Command>) {
        self.commands.push(cmd);
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn commands_mut(&mut self) -> &mut Vec<Box<dyn Command>> {
        &mut self.commands
    }

    /// The canvas regions of every child that is a [`ReverseDeltaCommand`]
    /// (children of other shapes contribute nothing).
    pub fn regions(&self) -> Vec<Rect2i> {
        self.commands
            .iter()
            .filter_map(|cmd| {
                cmd.as_any()
                    .downcast_ref::<ReverseDeltaCommand>()
                    .map(|c| c.region())
            })
            .collect()
    }

    /// The per-child pixel deltas `(layer, region, before, after)` of every
    /// child that is a [`ReverseDeltaCommand`] (like [`Self::regions`], but
    /// carrying the bytes too, for the tile write-back path).
    pub fn child_pixel_deltas(&self) -> Vec<(LayerId, Rect2i, &[u8], &[u8])> {
        self.commands
            .iter()
            .filter_map(|cmd| {
                cmd.as_any()
                    .downcast_ref::<ReverseDeltaCommand>()
                    .map(|c| (c.layer_id(), c.region(), c.before(), c.after()))
            })
            .collect()
    }
}

impl Command for CompositeCommand {
    fn name(&self) -> &str {
        self.name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        // Reverse order: last pushed undone first.
        let mut all_ok = true;
        for cmd in self.commands.iter_mut().rev() {
            if !cmd.undo(context) {
                all_ok = false;
            }
        }
        all_ok
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        // Forward order.
        let mut all_ok = true;
        for cmd in self.commands.iter_mut() {
            if !cmd.redo(context) {
                all_ok = false;
            }
        }
        all_ok
    }
}

// ---------------------------------------------------------------------------
// TilemapEditCommand
// ---------------------------------------------------------------------------

/// One Tile-tool gesture: a set of per-cell tilemap edits, committed as a
/// single undo step.
///
/// `diffs` holds `(cell, before, after)` in deterministic (cell-sorted) order.
/// `buffer_deltas` holds each edited cell's FOOTPRINT rect and its before/after
/// buffer bytes (the integrated model bakes the oriented tile into the buffer,
/// so a stamp changes BOTH the tilemap cell and the buffer footprint; undo
/// restores the pre-stamp buffer bytes, redo re-applies the baked bytes).
/// `undo` restores the `before` cells + buffer, `redo` re-applies the `after`
/// cells + buffer, by writing them back through [`TileMap::set_cell`] /
/// [`PixelBuffer::blit_region`] on the layer (a missing layer/tilemap is a safe
/// no-op returning `false`). Because `set_cell` bumps the tilemap epoch, the
/// render cache re-rasterizes automatically after undo/redo.
pub struct TilemapEditCommand {
    layer: LayerId,
    diffs: Vec<((u32, u32), Option<TileCell>, Option<TileCell>)>,
    buffer_deltas: Vec<(Rect2i, Vec<u8>, Vec<u8>)>,
}

impl TilemapEditCommand {
    /// Build a command that reverts `diffs`' `before` on undo and re-applies
    /// `after` on redo.
    pub fn new(
        layer: LayerId,
        diffs: Vec<((u32, u32), Option<TileCell>, Option<TileCell>)>,
    ) -> Self {
        Self {
            layer,
            diffs,
            buffer_deltas: Vec::new(),
        }
    }

    /// Attach per-cell buffer footprint deltas `(rect, before, after)` so a
    /// stamp gesture undoes its buffer bake in one step.
    pub fn with_buffer_deltas(mut self, deltas: Vec<(Rect2i, Vec<u8>, Vec<u8>)>) -> Self {
        self.buffer_deltas = deltas;
        self
    }

    /// The number of cell diffs this gesture recorded.
    pub fn len(&self) -> usize {
        self.diffs.len()
    }

    /// Whether no cells were changed.
    pub fn is_empty(&self) -> bool {
        self.diffs.is_empty()
    }

    /// The per-cell buffer footprint deltas `(rect, before, after)`.
    pub fn buffer_deltas(&self) -> &[(Rect2i, Vec<u8>, Vec<u8>)] {
        &self.buffer_deltas
    }
}

impl Command for TilemapEditCommand {
    fn name(&self) -> &str {
        "Tile"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        let Some(layer) = context.layers.layer_mut(self.layer) else {
            return false;
        };
        let Some(tm) = layer.tilemap.as_mut() else {
            return false;
        };
        for (cell, before, _after) in &self.diffs {
            tm.set_cell(*cell, *before);
        }
        // Restore the pre-stamp buffer bytes (the footprint keeps the baked
        // content of the restored cell; a cleared cell's last baked content
        // becomes ordinary visible pixels and is untouched).
        for (rect, before, _after) in &self.buffer_deltas {
            let _ = layer.buffer.blit_region(*rect, before);
        }
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        let Some(layer) = context.layers.layer_mut(self.layer) else {
            return false;
        };
        let Some(tm) = layer.tilemap.as_mut() else {
            return false;
        };
        for (cell, _before, after) in &self.diffs {
            tm.set_cell(*cell, *after);
        }
        // Re-bake the after cells onto the buffer (maintains the invariant).
        let palette = &mut *context.palette;
        for (cell, _before, after) in &self.diffs {
            if after.is_none() {
                continue;
            }
            let _ = tm.blit_cell(palette, &mut layer.buffer, cell.0, cell.1);
        }
        true
    }
}

// ---------------------------------------------------------------------------
// TilePixelEditCommand
// ---------------------------------------------------------------------------

/// One pixel-edit gesture's write-back to the ROOT tile data: per-tile diffs
/// that a single undo restores (and redo re-applies), followed by a re-bake of
/// every instance of the edited tiles.
///
/// `diffs` holds one entry per edited tile, in deterministic
/// `(tile_id, ty, tx)` sorted order (see [`TilePixelDiff`]).
pub struct TilePixelEditCommand {
    diffs: Vec<TilePixelDiff>,
}

/// The changed root pixels of one tile: the coordinates and their before/after
/// RGBA8 bytes (4 bytes per pixel, `pixels` and `before`/`after` are parallel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TilePixelDiff {
    pub tile_id: TileId,
    /// Sorted by `(ty, tx)` — deterministic order.
    pub pixels: Vec<(u32, u32)>,
    /// The root byte before the edit (the FIRST root-before per pixel).
    pub before: Vec<u8>,
    /// The root byte after the edit (the LAST after per pixel).
    pub after: Vec<u8>,
}

impl TilePixelEditCommand {
    pub fn new(diffs: Vec<TilePixelDiff>) -> Self {
        Self { diffs }
    }

    /// The per-tile diffs, in deterministic `(tile_id, ty, tx)` order.
    pub fn diffs(&self) -> &[TilePixelDiff] {
        &self.diffs
    }
}

impl Command for TilePixelEditCommand {
    fn name(&self) -> &str {
        "Tile Edit"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn undo(&mut self, context: &mut CommandContext) -> bool {
        // Restore the root bytes and re-bake every instance.
        for diff in &self.diffs {
            let Some(tile) = context.palette.get_mut(diff.tile_id) else {
                continue;
            };
            let tw = usize::from(tile.w);
            for (i, &(tx, ty)) in diff.pixels.iter().enumerate() {
                let index = (ty as usize * tw + tx as usize) * 4;
                let end = (index + 4).min(tile.pixels.len());
                if index < end {
                    tile.pixels[index..end].copy_from_slice(&diff.before[i * 4..i * 4 + 4]);
                }
            }
        }
        context.palette.change_epoch = context.palette.change_epoch.wrapping_add(1);
        let ids: Vec<TileId> = self.diffs.iter().map(|d| d.tile_id).collect();
        let layers = &mut *context.layers;
        let palette = &*context.palette;
        tile_edit::re_stamp_tile_instances(layers, palette, &ids);
        true
    }

    fn redo(&mut self, context: &mut CommandContext) -> bool {
        for diff in &self.diffs {
            let Some(tile) = context.palette.get_mut(diff.tile_id) else {
                continue;
            };
            let tw = usize::from(tile.w);
            for (i, &(tx, ty)) in diff.pixels.iter().enumerate() {
                let index = (ty as usize * tw + tx as usize) * 4;
                let end = (index + 4).min(tile.pixels.len());
                if index < end {
                    tile.pixels[index..end].copy_from_slice(&diff.after[i * 4..i * 4 + 4]);
                }
            }
        }
        context.palette.change_epoch = context.palette.change_epoch.wrapping_add(1);
        let ids: Vec<TileId> = self.diffs.iter().map(|d| d.tile_id).collect();
        let layers = &mut *context.layers;
        let palette = &*context.palette;
        tile_edit::re_stamp_tile_instances(layers, palette, &ids);
        true
    }
}

// ---------------------------------------------------------------------------
// UndoStack
// ---------------------------------------------------------------------------

/// Unbounded undo/redo stack.  Each entry is one user gesture (possibly a
/// `CompositeCommand`).  Pushing a new command clears the redo list.
pub struct UndoStack {
    undo: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Push a command that is expected to be in its "done" state (i.e. redo has
    /// been applied).  Clears the redo stack because a new gesture invalidates
    /// future redo history.
    pub fn push(&mut self, cmd: Box<dyn Command>) {
        self.redo.clear();
        self.undo.push(cmd);
    }

    /// Pop the top undo command, call `undo(context)`.  On success the command
    /// moves to the redo stack; on failure it stays on undo (no state change).
    pub fn undo(&mut self, context: &mut CommandContext) -> bool {
        let mut cmd = match self.undo.pop() {
            Some(c) => c,
            None => return false,
        };
        if cmd.undo(context) {
            self.redo.push(cmd);
            true
        } else {
            self.undo.push(cmd);
            false
        }
    }

    /// Pop the top redo command, call `redo(context)`.  On success the command
    /// moves to the undo stack; on failure it stays on redo.
    pub fn redo(&mut self, context: &mut CommandContext) -> bool {
        let mut cmd = match self.redo.pop() {
            Some(c) => c,
            None => return false,
        };
        if cmd.redo(context) {
            self.undo.push(cmd);
            true
        } else {
            self.redo.push(cmd);
            false
        }
    }

    /// Drop both stacks.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Name of the command at the top of the undo stack, for UI display
    /// (e.g. "Undo Stroke").
    pub fn top_undo_name(&self) -> Option<&str> {
        self.undo.last().map(|c| c.name())
    }

    /// Borrow the top command without popping (None when the stack is empty).
    pub fn peek_top(&self) -> Option<&dyn Command> {
        self.undo.last().map(|c| c.as_ref())
    }

    /// Mutably borrow the top command without popping (None when empty).
    pub fn peek_top_mut(&mut self) -> Option<&mut dyn Command> {
        self.undo.last_mut().map(|c| c.as_mut())
    }
}

// ---------------------------------------------------------------------------
// DeltaRecorder
// ---------------------------------------------------------------------------

/// Pre-mutation capture helper (D46).  Call `begin` before the mutation to
/// snapshot the region, then `finish` afterwards to build a
/// `ReverseDeltaCommand`.
pub struct DeltaRecorder {
    name: &'static str,
    layer: LayerId,
    region: Rect2i,
    before: Vec<u8>,
}

impl DeltaRecorder {
    /// Snapshot the region bytes before a mutation.  Returns `None` when the
    /// region is invalid for the buffer.
    pub fn begin(
        name: &'static str,
        layer: LayerId,
        buf: &PixelBuffer,
        region: Rect2i,
    ) -> Option<Self> {
        if !is_valid_region(buf, region) {
            return None;
        }
        let before = capture_region(buf, region);
        Some(Self {
            name,
            layer,
            region,
            before,
        })
    }

    /// Build a `ReverseDeltaCommand` using the captured `before` and the
    /// current buffer state as `after`.
    pub fn finish(&self, buf: &PixelBuffer) -> ReverseDeltaCommand {
        let after = capture_region(buf, self.region);
        ReverseDeltaCommand::new(
            self.name,
            self.layer,
            self.region,
            self.before.clone(),
            after,
        )
    }

    pub fn region(&self) -> Rect2i {
        self.region
    }

    /// The captured before bytes.
    pub fn before_bytes(&self) -> &[u8] {
        &self.before
    }

    /// Returns `true` when the buffer region is unchanged since `begin`.
    pub fn is_empty_delta(&self, buf: &PixelBuffer) -> bool {
        let current = capture_region(buf, self.region);
        self.before == current
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check that `region` fits inside the buffer (matches `blit_region` bounds).
fn is_valid_region(buf: &PixelBuffer, region: Rect2i) -> bool {
    region.x >= 0
        && region.y >= 0
        && !region.is_empty()
        && (region.x as usize) + (region.w as usize) <= buf.width()
        && (region.y as usize) + (region.h as usize) <= buf.height()
}

/// Efficiently copy the raw RGBA8 bytes of `region` from `buf`.
fn capture_region(buf: &PixelBuffer, region: Rect2i) -> Vec<u8> {
    let rw = region.w as usize;
    let rh = region.h as usize;
    let buf_w = buf.width();
    let bytes = buf.as_bytes();
    let mut out = Vec::with_capacity(rw * rh * 4);
    let x0 = region.x as usize;
    let y0 = region.y as usize;
    for row in 0..rh {
        let start = (y0 + row) * buf_w * 4 + x0 * 4;
        let end = start + rw * 4;
        out.extend_from_slice(&bytes[start..end]);
    }
    out
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -- helpers for tests --------------------------------------------------

    /// Build a `CommandContext` borrowing the given layer stack. The palette is
    /// leaked so the returned context outlives the `&mut ctx(...)` temporary
    /// (test-only; no leak concern in tests).
    fn ctx(layers: &mut LayerStack) -> CommandContext<'_> {
        let palette = Box::leak(Box::new(TilePalette::new()));
        CommandContext {
            layers,
            palette: &mut *palette,
        }
    }

    /// Fill a `PixelBuffer` with a known per-pixel pattern derived from (x, y).
    fn pattern_fill(buf: &mut PixelBuffer) {
        let w = buf.width();
        let h = buf.height();
        for y in 0..h {
            for x in 0..w {
                let r = ((x * 17 + y * 31) & 0xFF) as u8;
                let g = ((x * 13 + y * 23) & 0xFF) as u8;
                let b = ((x * 11 + y * 37) & 0xFF) as u8;
                buf.as_bytes_mut()[y * w * 4 + x * 4] = r;
                buf.as_bytes_mut()[y * w * 4 + x * 4 + 1] = g;
                buf.as_bytes_mut()[y * w * 4 + x * 4 + 2] = b;
                buf.as_bytes_mut()[y * w * 4 + x * 4 + 3] = 255;
            }
        }
    }

    /// A test-only command that always fails its undo (returns false).
    struct FailingUndoCmd;

    impl Command for FailingUndoCmd {
        fn name(&self) -> &str {
            "FailingUndo"
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn undo(&mut self, _context: &mut CommandContext) -> bool {
            false
        }
        fn redo(&mut self, _context: &mut CommandContext) -> bool {
            true
        }
    }

    // -- ReverseDeltaCommand round-trip -------------------------------------

    #[test]
    fn reverse_delta_round_trip() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let before_bytes: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 3) as u8).collect();
        let after_bytes: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 7 + 1) as u8).collect();
        let region = Rect2i::new(0, 0, 4, 4);

        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.blit_region(region, &after_bytes);
            assert_eq!(buf.as_bytes(), &after_bytes[..]);
        }

        let mut cmd = ReverseDeltaCommand::new(
            "Stroke",
            lid,
            region,
            before_bytes.clone(),
            after_bytes.clone(),
        );

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &before_bytes[..]);

        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &after_bytes[..]);

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &before_bytes[..]);

        // Already undone → no-op.
        assert!(!cmd.undo(&mut ctx(&mut layers)));

        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &after_bytes[..]);

        // Already redone → no-op.
        assert!(!cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &after_bytes[..]);
    }

    // -- ReverseDeltaCommand targets a non-active layer ---------------------

    #[test]
    fn reverse_delta_targets_non_active_layer() {
        let mut layers = LayerStack::new(4, 4);
        let active = layers.active_layer_id();
        let second = layers.add_layer("Layer 2");
        assert_ne!(active, second);

        let region = Rect2i::new(0, 0, 2, 2);
        let before = vec![0u8; 2 * 2 * 4];
        let after = vec![77u8; 2 * 2 * 4];

        // Apply the "after" state to the second (non-active) layer.
        layers
            .layer_mut(second)
            .unwrap()
            .buffer
            .blit_region(region, &after);

        let mut cmd =
            ReverseDeltaCommand::new("Stroke", second, region, before.clone(), after.clone());

        // Undo restores the second layer, leaving the active layer untouched.
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            read_region(&layers.layer(second).unwrap().buffer, region),
            before
        );
        assert_eq!(
            read_region(&layers.layer(active).unwrap().buffer, region),
            vec![0u8; 2 * 2 * 4]
        );

        // Redo re-applies to the second layer only.
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            read_region(&layers.layer(second).unwrap().buffer, region),
            after
        );
        assert_eq!(
            read_region(&layers.layer(active).unwrap().buffer, region),
            vec![0u8; 2 * 2 * 4]
        );
    }

    // -- ReverseDeltaCommand on a removed layer -----------------------------

    #[test]
    fn reverse_delta_removed_layer_returns_false_without_panic() {
        let mut layers = LayerStack::new(4, 4);
        let active = layers.active_layer_id();
        let second = layers.add_layer("Layer 2");

        let region = Rect2i::new(0, 0, 2, 2);
        let before = vec![0u8; 2 * 2 * 4];
        let after = vec![55u8; 2 * 2 * 4];
        layers
            .layer_mut(second)
            .unwrap()
            .buffer
            .blit_region(region, &after);

        let mut cmd = ReverseDeltaCommand::new("Stroke", second, region, before, after.clone());

        // Remove the layer the command targets.
        assert!(layers.remove_layer(second).is_some());

        // Undo/redo on a missing layer: false, no panic, nothing else changes.
        assert!(!cmd.undo(&mut ctx(&mut layers)));
        assert!(!cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.len(), 1);
        assert_eq!(
            read_region(&layers.layer(active).unwrap().buffer, region),
            vec![0u8; 2 * 2 * 4]
        );
    }

    // -- UndoStack push / undo / redo sequence ------------------------------

    #[test]
    fn stack_push_undo_redo() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let region = Rect2i::new(2, 2, 3, 3);

        let before = vec![0u8; 3 * 3 * 4];
        let mut after = vec![0u8; 3 * 3 * 4];
        for i in (0..after.len()).step_by(4) {
            after[i] = 100;
            after[i + 1] = 200;
            after[i + 2] = 50;
            after[i + 3] = 255;
        }

        // Buffer starts all-zero; blit "after" to simulate the stroke having
        // happened.
        layers.active_layer_mut().buffer.blit_region(region, &after);

        let cmd = ReverseDeltaCommand::new("Stroke", lid, region, before, after.clone());
        let mut stack = UndoStack::new();
        stack.push(Box::new(cmd));

        assert!(stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(stack.top_undo_name(), Some("Stroke"));

        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            read_region(&layers.active_layer().buffer, region),
            vec![0u8; 3 * 3 * 4]
        );
        assert!(stack.can_redo());
        assert_eq!(stack.undo_len(), 0);
        assert_eq!(stack.redo_len(), 1);

        assert!(stack.redo(&mut ctx(&mut layers)));
        assert_eq!(read_region(&layers.active_layer().buffer, region), after);
        assert!(!stack.can_redo());
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 0);

        // A new push clears the redo list.
        let cmd2 = ReverseDeltaCommand::new(
            "Fill",
            lid,
            Rect2i::new(0, 0, 2, 2),
            vec![0u8; 2 * 2 * 4],
            vec![42u8; 2 * 2 * 4],
        );
        stack.push(Box::new(cmd2));
        assert!(!stack.can_redo());
        assert_eq!(stack.undo_len(), 2);
    }

    // -- UndoStack failure safety -------------------------------------------

    #[test]
    fn stack_failure_safety() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();

        let good = ReverseDeltaCommand::new(
            "Good",
            lid,
            Rect2i::new(0, 0, 2, 2),
            vec![0u8; 2 * 2 * 4],
            vec![100u8; 2 * 2 * 4],
        );
        layers
            .active_layer_mut()
            .buffer
            .blit_region(Rect2i::new(0, 0, 2, 2), &[100u8; 2 * 2 * 4]);
        let mut stack = UndoStack::new();
        stack.push(Box::new(good));
        stack.push(Box::new(FailingUndoCmd));

        // Top command fails undo → stack unchanged, nothing moved to redo.
        assert!(!stack.undo(&mut ctx(&mut layers)));
        assert_eq!(stack.undo_len(), 2);
        assert_eq!(stack.redo_len(), 0);
        assert_eq!(stack.top_undo_name(), Some("FailingUndo"));
    }

    // -- UndoStack with a removed layer -------------------------------------

    #[test]
    fn stack_undo_removed_layer_returns_false_and_keeps_remaining_layers() {
        let mut layers = LayerStack::new(4, 4);
        let active = layers.active_layer_id();
        let second = layers.add_layer("Layer 2");

        let region = Rect2i::new(0, 0, 2, 2);
        let before = vec![0u8; 2 * 2 * 4];
        let after = vec![88u8; 2 * 2 * 4];
        layers
            .layer_mut(second)
            .unwrap()
            .buffer
            .blit_region(region, &after);

        let mut stack = UndoStack::new();
        stack.push(Box::new(ReverseDeltaCommand::new(
            "Stroke",
            second,
            region,
            before,
            after.clone(),
        )));

        assert!(layers.remove_layer(second).is_some());

        // The command stays on the undo stack (failure → no state change) and
        // the remaining layer is untouched.
        assert!(!stack.undo(&mut ctx(&mut layers)));
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 0);
        assert_eq!(layers.len(), 1);
        assert_eq!(
            read_region(&layers.layer(active).unwrap().buffer, region),
            vec![0u8; 2 * 2 * 4]
        );
    }

    // -- CompositeCommand undo / redo ordering ------------------------------

    #[test]
    fn composite_undo_redo_ordering() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let region_a = Rect2i::new(0, 0, 2, 2);
        let region_b = Rect2i::new(2, 2, 2, 2);

        let before_a = vec![0u8; 2 * 2 * 4];
        let after_a = vec![10u8; 2 * 2 * 4];
        let before_b = vec![0u8; 2 * 2 * 4];
        let after_b = vec![20u8; 2 * 2 * 4];

        // Isolated reference: undoing StrokeB alone from the fully-applied
        // state leaves region_a untouched and restores region_b.
        let mut ref_layers = LayerStack::new(8, 8);
        {
            let buf = &mut ref_layers.active_layer_mut().buffer;
            buf.blit_region(region_a, &after_a);
            buf.blit_region(region_b, &after_b);
        }
        let mut single_b =
            ReverseDeltaCommand::new("StrokeB", lid, region_b, before_b.clone(), after_b.clone());
        single_b.undo(&mut ctx(&mut ref_layers));
        let intermediate_ref = ref_layers.active_layer().buffer.as_bytes().to_vec();

        // Manual reverse-order undo captures the composite's midpoint: after
        // the first child undo (StrokeB), region_a is still after_a.
        let mut manual_layers = LayerStack::new(8, 8);
        {
            let buf = &mut manual_layers.active_layer_mut().buffer;
            buf.blit_region(region_a, &after_a);
            buf.blit_region(region_b, &after_b);
        }
        let mut b_cmd =
            ReverseDeltaCommand::new("StrokeB", lid, region_b, before_b.clone(), after_b.clone());
        b_cmd.undo(&mut ctx(&mut manual_layers));
        let midpoint = manual_layers.active_layer().buffer.as_bytes().to_vec();
        let mut a_cmd =
            ReverseDeltaCommand::new("StrokeA", lid, region_a, before_a.clone(), after_a.clone());
        a_cmd.undo(&mut ctx(&mut manual_layers));

        // Composite on the fully-applied state.
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.blit_region(region_a, &after_a);
            buf.blit_region(region_b, &after_b);
        }

        let mut composite = CompositeCommand::new("Gesture");
        composite.push(Box::new(ReverseDeltaCommand::new(
            "StrokeA",
            lid,
            region_a,
            before_a.clone(),
            after_a.clone(),
        )));
        composite.push(Box::new(ReverseDeltaCommand::new(
            "StrokeB",
            lid,
            region_b,
            before_b.clone(),
            after_b.clone(),
        )));

        let mut stack = UndoStack::new();
        stack.push(Box::new(composite));

        // Undo: StrokeB first (reverse order), then StrokeA.
        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.as_bytes(),
            manual_layers.active_layer().buffer.as_bytes()
        );
        assert_eq!(
            read_region(&layers.active_layer().buffer, region_a),
            before_a
        );
        assert_eq!(
            read_region(&layers.active_layer().buffer, region_b),
            before_b
        );

        // The midpoint matches the isolated result of undoing StrokeB alone.
        assert_eq!(midpoint, intermediate_ref);

        // Redo: StrokeA first (forward order), then StrokeB.
        assert!(stack.redo(&mut ctx(&mut layers)));
        assert_eq!(
            read_region(&layers.active_layer().buffer, region_a),
            after_a
        );
        assert_eq!(
            read_region(&layers.active_layer().buffer, region_b),
            after_b
        );
    }

    // -- CompositeCommand failure: one failing child ------------------------

    #[test]
    fn composite_failure_one_child() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let region = Rect2i::new(0, 0, 2, 2);

        let mut composite = CompositeCommand::new("MixedGesture");
        composite.push(Box::new(ReverseDeltaCommand::new(
            "Good",
            lid,
            region,
            vec![0u8; 2 * 2 * 4],
            vec![99u8; 2 * 2 * 4],
        )));
        composite.push(Box::new(FailingUndoCmd));

        layers
            .active_layer_mut()
            .buffer
            .blit_region(region, &[99u8; 2 * 2 * 4]);

        // Whole composite returns false, but the Good child still applied its
        // undo: the region is partially restored to its before bytes.
        assert!(!composite.undo(&mut ctx(&mut layers)));
        assert_eq!(
            read_region(&layers.active_layer().buffer, region),
            vec![0u8; 2 * 2 * 4]
        );
    }

    // -- TilemapEditCommand ------------------------------------------------

    #[test]
    fn tilemap_edit_command_undo_redos_and_name() {
        use crate::core::tilemap::{TileCell, TileId, TileMap};

        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        layers.layer_mut(lid).unwrap().tilemap = Some(TileMap::new(4, 2, 2));
        let before = TileCell::new(TileId(1));
        let after = TileCell {
            tile_id: TileId(2),
            rotation: 1,
            flip_x: true,
            flip_y: false,
        };
        let mut cmd = TilemapEditCommand::new(
            lid,
            vec![((0, 0), None, Some(after)), ((1, 1), Some(before), None)],
        );
        assert_eq!(cmd.name(), "Tile");
        assert_eq!(cmd.len(), 2);

        // Apply the "after" state (as the placer did via set_cell).
        let tm = layers.layer_mut(lid).unwrap().tilemap.as_mut().unwrap();
        tm.set_cell((0, 0), Some(after));
        tm.set_cell((1, 1), None);

        // Undo restores the before cells.
        assert!(cmd.undo(&mut ctx(&mut layers)));
        let tm = layers.layer(lid).unwrap().tilemap.as_ref().unwrap();
        assert_eq!(tm.cell((0, 0)), None);
        assert_eq!(tm.cell((1, 1)), Some(before));

        // Redo re-applies the after cells.
        assert!(cmd.redo(&mut ctx(&mut layers)));
        let tm = layers.layer(lid).unwrap().tilemap.as_ref().unwrap();
        assert_eq!(tm.cell((0, 0)), Some(after));
        assert_eq!(tm.cell((1, 1)), None);

        // Undo again round-trips deterministically.
        assert!(cmd.undo(&mut ctx(&mut layers)));
        let tm = layers.layer(lid).unwrap().tilemap.as_ref().unwrap();
        assert_eq!(tm.cell((0, 0)), None);
        assert_eq!(tm.cell((1, 1)), Some(before));
    }

    #[test]
    fn tilemap_edit_command_missing_layer_or_tilemap_is_safe_noop() {
        use crate::core::tilemap::{TileCell, TileId};

        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let cell = Some(TileCell::new(TileId(1)));

        // Missing tilemap on the target layer -> undo/redo return false safely.
        let mut cmd = TilemapEditCommand::new(lid, vec![((0, 0), None, cell)]);
        assert!(!cmd.undo(&mut ctx(&mut layers)));
        assert!(!cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.layer(lid).unwrap().tilemap, None);

        // Missing layer entirely -> false.
        let mut cmd = TilemapEditCommand::new(LayerId::new(999), vec![((0, 0), None, cell)]);
        assert!(!cmd.undo(&mut ctx(&mut layers)));
        assert!(!cmd.redo(&mut ctx(&mut layers)));
    }

    // -- DeltaRecorder begin / finish / round-trip --------------------------

    #[test]
    fn delta_recorder_round_trip() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let region = Rect2i::new(1, 1, 2, 2);

        pattern_fill(&mut layers.active_layer_mut().buffer);

        let before_region = read_region(&layers.active_layer().buffer, region);
        let recorder =
            DeltaRecorder::begin("Fill", lid, &layers.active_layer().buffer, region).unwrap();
        assert_eq!(&recorder.before, &before_region);

        layers
            .active_layer_mut()
            .buffer
            .blit_region(region, &[42u8; 2 * 2 * 4]);
        assert!(!recorder.is_empty_delta(&layers.active_layer().buffer));

        let mut cmd = recorder.finish(&layers.active_layer().buffer);
        assert_eq!(cmd.name(), "Fill");

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            read_region(&layers.active_layer().buffer, region),
            before_region
        );

        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            read_region(&layers.active_layer().buffer, region),
            vec![42u8; 2 * 2 * 4]
        );
    }

    // -- DeltaRecorder begin out-of-bounds returns None ---------------------

    #[test]
    fn delta_recorder_out_of_bounds() {
        let layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let buf = &layers.active_layer().buffer;

        assert!(DeltaRecorder::begin("X", lid, buf, Rect2i::new(-1, 0, 2, 2)).is_none());
        assert!(DeltaRecorder::begin("X", lid, buf, Rect2i::new(3, 0, 2, 2)).is_none());
        assert!(DeltaRecorder::begin("X", lid, buf, Rect2i::new(0, 3, 4, 2)).is_none());
        assert!(DeltaRecorder::begin("X", lid, buf, Rect2i::new(0, 0, 0, 0)).is_none());
        assert!(DeltaRecorder::begin("X", lid, buf, Rect2i::new(0, 0, -1, 1)).is_none());
    }

    // -- DeltaRecorder is_empty_delta true when no change -------------------

    #[test]
    fn delta_recorder_empty_delta() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let region = Rect2i::new(0, 0, 2, 2);
        pattern_fill(&mut layers.active_layer_mut().buffer);

        let recorder =
            DeltaRecorder::begin("Stroke", lid, &layers.active_layer().buffer, region).unwrap();
        assert!(recorder.is_empty_delta(&layers.active_layer().buffer));

        layers
            .active_layer_mut()
            .buffer
            .blit_region(region, &[0u8; 2 * 2 * 4]);
        assert!(!recorder.is_empty_delta(&layers.active_layer().buffer));
    }

    // -- DeltaRecorder begin captures correct before bytes ------------------

    #[test]
    fn delta_recorder_begin_captures_before() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let region = Rect2i::new(1, 0, 2, 3);

        {
            let buf = &mut layers.active_layer_mut().buffer;
            for y in 0..3 {
                for x in 0..2 {
                    buf.as_bytes_mut()[(y * 4 + (x + 1)) * 4] = (x as u8) * 10 + (y as u8);
                }
            }
        }

        let recorder =
            DeltaRecorder::begin("Test", lid, &layers.active_layer().buffer, region).unwrap();
        let expected_before = read_region(&layers.active_layer().buffer, region);
        assert_eq!(recorder.before, expected_before);
    }

    // -- top_undo_name ------------------------------------------------------

    #[test]
    fn top_undo_name() {
        let layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let mut stack = UndoStack::new();
        assert_eq!(stack.top_undo_name(), None);

        let cmd1 = ReverseDeltaCommand::new(
            "Stroke",
            lid,
            Rect2i::new(0, 0, 1, 1),
            vec![0u8; 4],
            vec![1u8; 4],
        );
        stack.push(Box::new(cmd1));
        assert_eq!(stack.top_undo_name(), Some("Stroke"));

        let cmd2 = ReverseDeltaCommand::new(
            "Fill",
            lid,
            Rect2i::new(0, 0, 1, 1),
            vec![0u8; 4],
            vec![2u8; 4],
        );
        stack.push(Box::new(cmd2));
        assert_eq!(stack.top_undo_name(), Some("Fill"));
    }

    // -- UndoStack clear ----------------------------------------------------

    #[test]
    fn stack_clear() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let mut stack = UndoStack::new();
        stack.push(Box::new(ReverseDeltaCommand::new(
            "A",
            lid,
            Rect2i::new(0, 0, 1, 1),
            vec![0u8; 4],
            vec![1u8; 4],
        )));
        stack.push(Box::new(ReverseDeltaCommand::new(
            "B",
            lid,
            Rect2i::new(0, 0, 1, 1),
            vec![0u8; 4],
            vec![2u8; 4],
        )));

        layers
            .active_layer_mut()
            .buffer
            .blit_region(Rect2i::new(0, 0, 1, 1), &[2u8; 4]);
        stack.undo(&mut ctx(&mut layers));

        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 1);

        stack.clear();
        assert_eq!(stack.undo_len(), 0);
        assert_eq!(stack.redo_len(), 0);
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
    }

    // -- UndoStack empty operations -----------------------------------------

    #[test]
    fn stack_empty_operations() {
        let mut layers = LayerStack::new(2, 2);
        let mut stack = UndoStack::new();
        assert!(!stack.undo(&mut ctx(&mut layers)));
        assert!(!stack.redo(&mut ctx(&mut layers)));
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(stack.top_undo_name(), None);
    }

    // -- CompositeCommand is_empty / len ------------------------------------

    #[test]
    fn composite_basics() {
        let layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let mut c = CompositeCommand::new("Group");
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);

        c.push(Box::new(ReverseDeltaCommand::new(
            "A",
            lid,
            Rect2i::new(0, 0, 1, 1),
            vec![0u8; 4],
            vec![1u8; 4],
        )));
        assert!(!c.is_empty());
        assert_eq!(c.len(), 1);

        assert_eq!(c.name(), "Group");
    }

    // -- Helper: read a region's bytes from a buffer ------------------------

    fn read_region(buf: &PixelBuffer, region: Rect2i) -> Vec<u8> {
        capture_region(buf, region)
    }
}
