//! Select + Move tool core (D50).
//!
//! A selection is a canvas-clipped bounding box plus a snapshot of the active
//! layer's pixels in that box.  It never mutates pixel data on its own: it is
//! a passive record of *which* pixels are selected, and every pixel change
//! goes through a [`ReverseDeltaCommand`] (move, cut, delete).
//!
//! The selected region is described by a [`SelectionMask`] rather than a bare
//! `Option<Vec<bool>>`, so a new mask kind can be added without changing
//! [`Selection`]'s shape or its consumers.  `SelectionMask::Rectangle` is the
//! fast path (no per-cell storage); `SelectionMask::Bitmap` is a row-major
//! `rect.area()` boolean map used by the Magic Wand, the Lasso, and any
//! Add/Subtract/Intersect that produces a non-rectangular result.
//!
//! Moving a *rectangular* selection and committing it is a cut+paste
//! [`ReverseDeltaCommand`]: the source is cleared to transparent and the
//! snapshot is written at the destination (clipped to the canvas).  During a
//! drag nothing touches the buffer — the preview is an overlay drawn by the
//! render layer (D50: "preview overlay, no buffer mutation during drag").
//!
//! The mask shape operations ([`Selection::union_shape`],
//! [`Selection::intersect_shape`], [`Selection::subtract_shape`],
//! [`Selection::translated_shape`]) only compute a `(bbox, mask)` pair; the UI
//! re-captures the snapshot from the live buffer so the selection always holds
//! current pixels.  Operations whose result is provably a rectangle skip the
//! per-cell scan, so dragging a rectangular selection stays O(1) in the
//! selection's area rather than O(area) per mouse-move.

use crate::core::buffer::PixelBuffer;
use crate::core::math::Rect2i;
use crate::core::model::LayerId;
use crate::core::undo::{DeltaRecorder, ReverseDeltaCommand};

/// The static label stamped on every move-derived undo command.
const MOVE_UNDO_NAME: &str = "Move";

/// The static label stamped on every selection-delete-derived undo command.
const DELETE_UNDO_NAME: &str = "Delete";

/// The static label stamped on every selection-flip-derived undo command.
const FLIP_UNDO_NAME: &str = "Flip";

/// The static label stamped on every selection-rotate-derived undo command.
const ROTATE_UNDO_NAME: &str = "Rotate";

/// How a new selection gesture combines with the existing selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SelectMode {
    /// Replace the current selection with the new one.
    #[default]
    Replace,
    /// Union the new region into the current selection (Shift).
    Add,
    /// Remove the new region from the current selection (Alt / right button).
    Subtract,
    /// Keep only the pixels common to both selections.
    Intersect,
}

/// Row-major index of `(x, y)` inside `rect`, or `None` when outside.  Every
/// mask in this module indexes this way, so the arithmetic lives only here.
#[inline]
fn index_of(rect: Rect2i, x: i32, y: i32) -> Option<usize> {
    if !rect.contains(x, y) {
        return None;
    }
    Some(((y - rect.y) as i64 * rect.w as i64 + (x - rect.x) as i64) as usize)
}

/// Which pixels of a selection's bounding box are selected.  `Rectangle`
/// stores nothing per cell; `Bitmap` stores one flag per cell.  Use
/// [`Self::contains`] over [`Self::to_vec`] in hot paths so the rectangular
/// case never allocates.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SelectionMask {
    /// Every pixel of the bounding box is selected.
    Rectangle,
    /// A row-major `rect.area()` boolean map; `true` means selected.  Holes are
    /// transparent in the selection's snapshot.
    Bitmap(Vec<bool>),
}

impl SelectionMask {
    /// The mask that selects a whole bounding box.
    pub const fn rectangle() -> Self {
        Self::Rectangle
    }

    /// A per-cell mask over `rect.area()` pixels.
    pub fn bitmap(cells: Vec<bool>) -> Self {
        Self::Bitmap(cells)
    }

    /// Whether this mask selects its whole box without per-cell storage.
    pub fn is_rectangle(&self) -> bool {
        matches!(self, Self::Rectangle)
    }

    /// The per-cell flags, or `None` for the rectangular fast path.
    pub fn cells(&self) -> Option<&[bool]> {
        match self {
            Self::Rectangle => None,
            Self::Bitmap(cells) => Some(cells),
        }
    }

    /// Materializes the row-major flag map over `rect.area()` pixels.  Used by
    /// the `(bbox, mask)` shape API; prefer [`Self::contains`] in hot paths.
    pub fn to_vec(&self, rect: Rect2i) -> Vec<bool> {
        match self {
            Self::Rectangle => vec![true; rect.area().max(0) as usize],
            Self::Bitmap(cells) => cells.clone(),
        }
    }

    /// Number of selected cells in a box of `rect.area()` pixels.
    pub fn selected_count(&self, rect: Rect2i) -> usize {
        match self {
            Self::Rectangle => rect.area().max(0) as usize,
            Self::Bitmap(cells) => cells.iter().filter(|&&selected| selected).count(),
        }
    }

    /// Whether canvas pixel `(x, y)` inside `rect` is selected.
    pub fn contains(&self, rect: Rect2i, x: i32, y: i32) -> bool {
        match self {
            Self::Rectangle => rect.contains(x, y),
            Self::Bitmap(cells) => index_of(rect, x, y)
                .and_then(|idx| cells.get(idx).copied())
                .unwrap_or(false),
        }
    }
}

/// A marquee selection: a canvas-clipped box, a snapshot of the pixels it
/// captured, and the mask saying which of them are selected.  Scope is the
/// active layer only (D36).
#[derive(Clone)]
pub struct Selection {
    rect: Rect2i,
    snapshot: Vec<u8>,
    mask: SelectionMask,
    canvas: Rect2i,
}

impl Selection {
    /// Builds a selection of any mask kind over `rect`.
    ///
    /// The rect is clipped to the buffer's canvas and the mask is re-indexed
    /// against the clipped box.  An all-selected bitmap collapses to
    /// [`SelectionMask::Rectangle`], so a rectangular result never carries
    /// per-cell storage.  Returns `None` when the clipped box is empty, when a
    /// bitmap's length does not match `rect.area()`, or when no pixel is
    /// selected.
    pub fn from_mask(buf: &PixelBuffer, rect: Rect2i, mask: SelectionMask) -> Option<Self> {
        let canvas = Rect2i::new(0, 0, buf.width() as i32, buf.height() as i32);
        let clipped = rect.clamp_to(canvas);
        if clipped.is_empty() {
            return None;
        }
        let cells = match &mask {
            SelectionMask::Rectangle => {
                let snapshot = buf.export_region(clipped, None)?;
                return Some(Self {
                    rect: clipped,
                    snapshot,
                    mask: SelectionMask::Rectangle,
                    canvas,
                });
            }
            SelectionMask::Bitmap(cells) => cells,
        };
        if cells.len() != rect.area().max(0) as usize || rect.is_empty() {
            return None;
        }
        let mut cropped = vec![false; clipped.area() as usize];
        let mut all_true = true;
        for y in clipped.y..clipped.bottom() {
            for x in clipped.x..clipped.right() {
                let selected = index_of(rect, x, y)
                    .and_then(|idx| cells.get(idx).copied())
                    .unwrap_or(false);
                cropped[index_of(clipped, x, y)?] = selected;
                all_true &= selected;
            }
        }
        if !cropped.iter().any(|&selected| selected) {
            return None;
        }
        if all_true {
            let snapshot = buf.export_region(clipped, None)?;
            return Some(Self {
                rect: clipped,
                snapshot,
                mask: SelectionMask::Rectangle,
                canvas,
            });
        }
        // Snapshot with holes zeroed so a masked blit only writes selected px.
        let mut snapshot = vec![0u8; clipped.area() as usize * 4];
        for y in clipped.y..clipped.bottom() {
            for x in clipped.x..clipped.right() {
                let idx = index_of(clipped, x, y)?;
                if cropped[idx] {
                    if let Some(src) = buf.get_pixel(x as usize, y as usize) {
                        snapshot[idx * 4] = src.r;
                        snapshot[idx * 4 + 1] = src.g;
                        snapshot[idx * 4 + 2] = src.b;
                        snapshot[idx * 4 + 3] = src.a;
                    }
                }
            }
        }
        Some(Self {
            rect: clipped,
            snapshot,
            mask: SelectionMask::Bitmap(cropped),
            canvas,
        })
    }

    /// Captures the pixels of `rect` from `buf`, clipping the rect to the
    /// buffer's canvas.  Returns `None` when the clipped rect is empty.
    pub fn capture(buf: &PixelBuffer, rect: Rect2i) -> Option<Self> {
        Self::from_mask(buf, rect, SelectionMask::Rectangle)
    }

    /// Captures a mask-shaped selection: `rect` is the bounding box, `mask` a
    /// row-major `rect.area()` boolean map.  The mask is cropped to the canvas;
    /// an all-true mask collapses to the rectangular fast path.  Returns `None`
    /// when the clipped box is empty or no pixel is selected.
    pub fn capture_mask(buf: &PixelBuffer, rect: Rect2i, mask: Vec<bool>) -> Option<Self> {
        Self::from_mask(buf, rect, SelectionMask::Bitmap(mask))
    }

    /// The selection rect (canvas-clipped bounding box).
    pub fn rect(&self) -> Rect2i {
        self.rect
    }

    /// The captured RGBA8 pixels of the selection box (holes zeroed for masks).
    pub fn snapshot(&self) -> &[u8] {
        &self.snapshot
    }

    /// The selection's mask, the pairing of its bounding box with the set of
    /// selected cells.
    pub fn selection_mask(&self) -> &SelectionMask {
        &self.mask
    }

    /// The per-pixel mask, or `None` for the rectangular fast path.
    pub fn mask(&self) -> Option<&[bool]> {
        self.mask.cells()
    }

    /// True when the selection is a plain rectangle (mask fast path).
    pub fn is_rectangular(&self) -> bool {
        self.mask.is_rectangle()
    }

    /// Whether canvas pixel `(x, y)` belongs to the selection.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.mask.contains(self.rect, x, y)
    }

    /// Number of selected pixels.
    pub fn pixel_count(&self) -> usize {
        self.mask.selected_count(self.rect)
    }

    /// The selection's `(bbox, mask)` shape, materializing an all-true mask for
    /// the rectangular fast path.
    pub fn to_shape(&self) -> (Rect2i, Vec<bool>) {
        (self.rect, self.mask.to_vec(self.rect))
    }

    /// The destination rect after translating by `(dx, dy)`, clamped to the
    /// canvas.
    pub fn destination(&self, dx: i32, dy: i32) -> Rect2i {
        self.rect.translate(dx, dy).clamp_to(self.canvas)
    }

    /// Shape of this selection translated by `(dx, dy)` and clipped to the
    /// canvas: `(bbox, mask)`.  Returns `None` when the result is empty.
    pub fn translated_shape(&self, dx: i32, dy: i32) -> Option<(Rect2i, Vec<bool>)> {
        let clipped = self.rect.translate(dx, dy).clamp_to(self.canvas);
        if clipped.is_empty() {
            return None;
        }
        if self.is_rectangular() {
            return Some((clipped, vec![true; clipped.area() as usize]));
        }
        let mut mask = vec![false; clipped.area() as usize];
        let mut any = false;
        for y in clipped.y..clipped.bottom() {
            for x in clipped.x..clipped.right() {
                if self.contains(x - dx, y - dy) {
                    mask[index_of(clipped, x, y)?] = true;
                    any = true;
                }
            }
        }
        any.then_some((clipped, mask))
    }

    /// This selection's shape re-captured from `buf`, so the snapshot reflects
    /// the buffer's current pixels while the same pixels stay selected.  This is
    /// how the UI refreshes a selection after an operation changed the pixels
    /// underneath it (delete, cut).
    pub fn recaptured(&self, buf: &PixelBuffer) -> Option<Self> {
        Self::from_mask(buf, self.rect, self.mask.clone())
    }

    /// This selection translated by `(dx, dy)` and re-captured from `buf`.  A
    /// rectangular selection needs no mask, so a move costs no per-cell work.
    /// Returns `None` when the destination is empty or fully off-canvas.
    pub fn translated_to(&self, dx: i32, dy: i32, buf: &PixelBuffer) -> Option<Self> {
        let dest = self.destination(dx, dy);
        if dest.is_empty() {
            return None;
        }
        if self.is_rectangular() {
            return Self::capture(buf, dest);
        }
        let (bbox, mask) = self.translated_shape(dx, dy)?;
        Self::capture_mask(buf, bbox, mask)
    }

    /// Shape of the union of `self` and `other` over their combined box.  Two
    /// disjoint rectangles do NOT fill their combined box, so this always
    /// scans; unlike a drag, a union happens once per gesture.
    pub fn union_shape(&self, other: &Selection) -> Option<(Rect2i, Vec<bool>)> {
        let bbox = self.rect.union(other.rect).clamp_to(self.canvas);
        if bbox.is_empty() {
            return None;
        }
        let mut mask = vec![false; bbox.area() as usize];
        let mut any = false;
        for y in bbox.y..bbox.bottom() {
            for x in bbox.x..bbox.right() {
                if self.contains(x, y) || other.contains(x, y) {
                    mask[index_of(bbox, x, y)?] = true;
                    any = true;
                }
            }
        }
        any.then_some((bbox, mask))
    }

    /// Shape of the pixels common to `self` and `other`, bounded by their
    /// intersection.  Returns `None` when the boxes do not overlap or no pixel
    /// is selected by both.
    pub fn intersect_shape(&self, other: &Selection) -> Option<(Rect2i, Vec<bool>)> {
        let bbox = self.rect.intersection(other.rect).clamp_to(self.canvas);
        if bbox.is_empty() {
            return None;
        }
        if self.is_rectangular() && other.is_rectangular() {
            return Some((bbox, vec![true; bbox.area() as usize]));
        }
        let mut mask = vec![false; bbox.area() as usize];
        let mut any = false;
        for y in bbox.y..bbox.bottom() {
            for x in bbox.x..bbox.right() {
                if self.contains(x, y) && other.contains(x, y) {
                    mask[index_of(bbox, x, y)?] = true;
                    any = true;
                }
            }
        }
        any.then_some((bbox, mask))
    }

    /// Shape of `self` minus `other`, tightly bounded.
    pub fn subtract_shape(&self, other: &Selection) -> Option<(Rect2i, Vec<bool>)> {
        if !self.rect.intersects(other.rect) {
            return Some(self.to_shape());
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for y in self.rect.y..self.rect.bottom() {
            for x in self.rect.x..self.rect.right() {
                if self.contains(x, y) && !other.contains(x, y) {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        if max_x < min_x {
            return None;
        }
        let bbox = Rect2i::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1);
        let mut mask = vec![false; bbox.area() as usize];
        for y in bbox.y..bbox.bottom() {
            for x in bbox.x..bbox.right() {
                if self.contains(x, y) && !other.contains(x, y) {
                    mask[index_of(bbox, x, y)?] = true;
                }
            }
        }
        Some((bbox, mask))
    }

    /// The complement of this selection's shape over the whole canvas.  Never
    /// collapses to the rectangular fast path: the source is always excluded.
    pub fn inverted_shape(&self) -> (Rect2i, Vec<bool>) {
        let bbox = self.canvas;
        let mut mask = vec![true; bbox.area().max(0) as usize];
        for y in self.rect.y..self.rect.bottom() {
            for x in self.rect.x..self.rect.right() {
                if !self.contains(x, y) {
                    continue;
                }
                if let Some(idx) = index_of(bbox, x, y) {
                    mask[idx] = false;
                }
            }
        }
        (bbox, mask)
    }

    /// This selection's shape mirrored inside its own bounding box, so the same
    /// pixels stay selected after a flip.
    pub fn mirrored_shape(&self, horizontal: bool) -> (Rect2i, Vec<bool>) {
        let bbox = self.rect;
        let mut mask = vec![false; bbox.area().max(0) as usize];
        for y in bbox.y..bbox.bottom() {
            for x in bbox.x..bbox.right() {
                if !self.contains(x, y) {
                    continue;
                }
                let (tx, ty) = if horizontal {
                    (bbox.right() - 1 - (x - bbox.x), y)
                } else {
                    (x, bbox.bottom() - 1 - (y - bbox.y))
                };
                if let Some(idx) = index_of(bbox, tx, ty) {
                    mask[idx] = true;
                }
            }
        }
        (bbox, mask)
    }

    /// This selection's shape rotated 90° clockwise around its bounding box
    /// center, so the same pixels stay selected after a rotate.  The box swaps
    /// width and height and re-centers on the original center; each destination
    /// cell inverse-maps back to the source, exactly like
    /// [`Selection::mirrored_shape`].
    pub fn rotate_shape(&self) -> (Rect2i, Vec<bool>) {
        let bbox = self.rect;
        let rotated = Rect2i::new(
            bbox.x + (bbox.w - bbox.h) / 2,
            bbox.y + (bbox.h - bbox.w) / 2,
            bbox.h,
            bbox.w,
        );
        let mut mask = vec![false; rotated.area().max(0) as usize];
        for y in rotated.y..rotated.bottom() {
            for x in rotated.x..rotated.right() {
                let src_x = bbox.x + (y - rotated.y);
                let src_y = bbox.y + (bbox.h - 1 - (x - rotated.x));
                if !self.contains(src_x, src_y) {
                    continue;
                }
                if let Some(idx) = index_of(rotated, x, y) {
                    mask[idx] = true;
                }
            }
        }
        (rotated, mask)
    }

    /// Boundary edges of the selection as unit segments in canvas coordinates
    /// (clockwise per edge, y-down).  Each edge borders a selected pixel and a
    /// non-selected neighbour (or the canvas edge).  Order is row-major
    /// deterministic.  This is the geometry marching-ants rendering consumes.
    pub fn outline_segments(&self) -> Vec<((i32, i32), (i32, i32))> {
        let mut segments = Vec::new();
        for y in self.rect.y..self.rect.bottom() {
            for x in self.rect.x..self.rect.right() {
                if !self.contains(x, y) {
                    continue;
                }
                if !self.contains(x, y - 1) {
                    segments.push(((x, y), (x + 1, y)));
                }
                if !self.contains(x, y + 1) {
                    segments.push(((x, y + 1), (x + 1, y + 1)));
                }
                if !self.contains(x - 1, y) {
                    segments.push(((x, y), (x, y + 1)));
                }
                if !self.contains(x + 1, y) {
                    segments.push(((x + 1, y), (x + 1, y + 1)));
                }
            }
        }
        segments
    }
}

/// Commits a move of `selection` to `dest` as one cut+paste undo command.
///
/// The source is cleared to transparent and the snapshot is written at `dest`
/// (clamped to the canvas).  The command covers the union of source and
/// destination rects with before/after bytes (D45 reverse-delta).
///
/// A rectangular selection takes the O(1) fast path: the whole source rect is
/// cleared and the whole snapshot pasted.  A mask selection clears only its
/// selected source pixels (unselected pixels inside the bounding box are
/// rewritten with their own bytes, exactly like [`delete_selected_command`])
/// and writes the translated mask at the destination, so its holes stay holes.
///
/// Returns `None` when the move is a no-op: `dest` equals the source rect, or
/// the cut+paste leaves the union region byte-identical (e.g. a fully
/// transparent snapshot).
pub fn move_selection_to_command(
    selection: &Selection,
    dest: Rect2i,
    layer: LayerId,
    buf: &mut PixelBuffer,
) -> Option<ReverseDeltaCommand> {
    let dest = dest.clamp_to(selection.canvas);
    if dest == selection.rect {
        return None;
    }
    if selection.is_rectangular() {
        return move_rectangular_selection(selection, dest, layer, buf);
    }
    move_mask_selection(selection, dest, layer, buf)
}

/// The rectangular fast path: clear the whole source rect, paste the whole
/// snapshot at `dest`.  O(1) in the selection's area (no per-cell scan).
fn move_rectangular_selection(
    selection: &Selection,
    dest: Rect2i,
    layer: LayerId,
    buf: &mut PixelBuffer,
) -> Option<ReverseDeltaCommand> {
    let union = selection.rect.union(dest);
    let recorder = DeltaRecorder::begin(MOVE_UNDO_NAME, layer, buf, union)?;

    let clear = vec![0u8; selection.rect.area() as usize * 4];
    buf.blit_region(selection.rect, &clear);
    buf.blit_region(dest, &selection.snapshot);

    if recorder.is_empty_delta(buf) {
        return None;
    }
    Some(recorder.finish(buf))
}

/// The mask path: clear only the selected source pixels, then write the
/// translated mask at `dest`.  Both blits are clipped to the canvas, and the
/// single recorder spans the union of source and destination so one undo
/// restores every touched pixel.
fn move_mask_selection(
    selection: &Selection,
    dest: Rect2i,
    layer: LayerId,
    buf: &mut PixelBuffer,
) -> Option<ReverseDeltaCommand> {
    let source = selection.rect;
    let union = source.union(dest);
    let recorder = DeltaRecorder::begin(MOVE_UNDO_NAME, layer, buf, union)?;
    // Snapshot the union before mutating: a destination hole that overlaps a
    // cleared source pixel must keep its ORIGINAL bytes, not the cleared ones.
    let before = buf.export_region(union, None)?;

    let mut clear = vec![0u8; source.area().max(0) as usize * 4];
    for y in source.y..source.bottom() {
        for x in source.x..source.right() {
            let idx = index_of(source, x, y)?;
            if selection.contains(x, y) {
                continue;
            }
            if let Some(px) = buf.get_pixel(x as usize, y as usize) {
                clear[idx * 4] = px.r;
                clear[idx * 4 + 1] = px.g;
                clear[idx * 4 + 2] = px.b;
                clear[idx * 4 + 3] = px.a;
            }
        }
    }
    buf.blit_region(source, &clear);

    let dx = dest.x - source.x;
    let dy = dest.y - source.y;
    let (bbox, mask) = selection.translated_shape(dx, dy)?;
    let mut paste = vec![0u8; bbox.area().max(0) as usize * 4];
    for y in bbox.y..bbox.bottom() {
        for x in bbox.x..bbox.right() {
            let idx = index_of(bbox, x, y)?;
            if !mask[idx] {
                if let Some(union_idx) = index_of(union, x, y) {
                    paste[idx * 4..idx * 4 + 4]
                        .copy_from_slice(&before[union_idx * 4..union_idx * 4 + 4]);
                }
                continue;
            }
            let (sx, sy) = (x - dx, y - dy);
            if let Some(src_idx) = index_of(source, sx, sy) {
                paste[idx * 4..idx * 4 + 4]
                    .copy_from_slice(&selection.snapshot[src_idx * 4..src_idx * 4 + 4]);
            }
        }
    }
    buf.blit_region(bbox, &paste);

    if recorder.is_empty_delta(buf) {
        return None;
    }
    Some(recorder.finish(buf))
}

/// Clears the selected pixels as ONE undoable step, leaving the selection
/// itself in place so it can be moved, copied or deleted again.
///
/// Pixels inside the bounding box that the mask rejects are rewritten with
/// their own bytes, so the single blit only clears what is actually selected.
/// Returns `None` when nothing would change.
pub fn delete_selected_command(
    selection: &Selection,
    layer: LayerId,
    buf: &mut PixelBuffer,
) -> Option<ReverseDeltaCommand> {
    let bbox = selection.rect();
    let recorder = DeltaRecorder::begin(DELETE_UNDO_NAME, layer, buf, bbox)?;
    let mut clear = vec![0u8; bbox.area().max(0) as usize * 4];
    for y in bbox.y..bbox.bottom() {
        for x in bbox.x..bbox.right() {
            let idx = index_of(bbox, x, y)?;
            if selection.contains(x, y) {
                continue;
            }
            if let Some(px) = buf.get_pixel(x as usize, y as usize) {
                clear[idx * 4] = px.r;
                clear[idx * 4 + 1] = px.g;
                clear[idx * 4 + 2] = px.b;
                clear[idx * 4 + 3] = px.a;
            }
        }
    }
    buf.blit_region(bbox, &clear);
    if recorder.is_empty_delta(buf) {
        return None;
    }
    Some(recorder.finish(buf))
}

/// Mirrors the selection's bounding box in place as ONE undoable step.
///
/// `horizontal` mirrors left↔right, otherwise top↔bottom.  The whole box is
/// mirrored, so for a rectangular selection this is exactly "flip the selected
/// pixels"; pair it with [`Selection::mirrored_shape`] to keep the same pixels
/// selected.  Returns `None` when the box is empty or already symmetric.
pub fn flip_selected_command(
    selection: &Selection,
    horizontal: bool,
    layer: LayerId,
    buf: &mut PixelBuffer,
) -> Option<ReverseDeltaCommand> {
    let bbox = selection.rect();
    if bbox.is_empty() {
        return None;
    }
    let recorder = DeltaRecorder::begin(FLIP_UNDO_NAME, layer, buf, bbox)?;
    let source = buf.export_region(bbox, None)?;
    let (w, h) = (bbox.w as usize, bbox.h as usize);
    let mut mirrored = source.clone();
    for y in 0..h {
        for x in 0..w {
            let (tx, ty) = if horizontal {
                (w - 1 - x, y)
            } else {
                (x, h - 1 - y)
            };
            let from = (ty * w + tx) * 4;
            let to = (y * w + x) * 4;
            mirrored[to..to + 4].copy_from_slice(&source[from..from + 4]);
        }
    }
    buf.blit_region(bbox, &mirrored);
    if recorder.is_empty_delta(buf) {
        return None;
    }
    Some(recorder.finish(buf))
}

/// Rotates the selection's bounding box 90° clockwise around its center as ONE
/// undoable step.
///
/// The box swaps width/height and re-centers on the original center (see
/// [`Selection::rotate_shape`]); the snapshot is permuted with it, so every
/// selected pixel lands at its rotated position: source cell `(col, row)` in a
/// `w×h` box moves to dest cell `(h-1-row, col)` in the `h×w` dest box.  A
/// rectangular selection takes the fast path: the source rect is cleared and the
/// rotated snapshot pasted.  A mask selection clears only its selected source
/// pixels and builds the paste from the rotated snapshot (selected cells) and
/// the pre-mutation union bytes (unselected cells), so holes and untouched
/// background survive.  Returns `None` when the box is empty or the rotation
/// leaves the union byte-identical (e.g. a 1×1 selection).
pub fn rotate_selected_command(
    selection: &Selection,
    layer: LayerId,
    buf: &mut PixelBuffer,
) -> Option<ReverseDeltaCommand> {
    let (dest, mask) = selection.rotate_shape();
    let union = selection.rect.union(dest);
    let recorder = DeltaRecorder::begin(ROTATE_UNDO_NAME, layer, buf, union)?;
    if selection.is_rectangular() {
        rotate_rectangular_selection(selection, dest, buf);
    } else {
        rotate_mask_selection(selection, dest, &mask, union, buf)?;
    }
    if recorder.is_empty_delta(buf) {
        return None;
    }
    Some(recorder.finish(buf))
}

/// The rectangular fast path: clear the whole source rect, paste the rotated
/// snapshot at `dest`.  O(1) in the selection's area (no per-cell scan).
fn rotate_rectangular_selection(selection: &Selection, dest: Rect2i, buf: &mut PixelBuffer) {
    let source = selection.rect;
    let (w, h) = (source.w as usize, source.h as usize);
    let mut rotated = vec![0u8; dest.area().max(0) as usize * 4];
    for row in 0..h {
        for col in 0..w {
            let from = (row * w + col) * 4;
            let to = (col * h + (h - 1 - row)) * 4;
            rotated[to..to + 4].copy_from_slice(&selection.snapshot[from..from + 4]);
        }
    }
    let clear = vec![0u8; source.area().max(0) as usize * 4];
    buf.blit_region(source, &clear);
    buf.blit_region(dest, &rotated);
}

/// The mask path: clear only the selected source pixels, then write the
/// rotated mask at `dest`.  Selected paste cells read the rotated snapshot;
/// unselected paste cells read the pre-mutation union bytes, so a destination
/// hole over a cleared source pixel keeps its original bytes.  Both blits are
/// clipped to the canvas, and the single recorder spans the union of source
/// and destination so one undo restores every touched pixel.
fn rotate_mask_selection(
    selection: &Selection,
    dest: Rect2i,
    mask: &[bool],
    union: Rect2i,
    buf: &mut PixelBuffer,
) -> Option<()> {
    let source = selection.rect;
    // Snapshot the union before mutating: a destination hole that overlaps a
    // cleared source pixel must keep its ORIGINAL bytes, not the cleared ones.
    let before = buf.export_region(union, None)?;

    let mut clear = vec![0u8; source.area().max(0) as usize * 4];
    for y in source.y..source.bottom() {
        for x in source.x..source.right() {
            let idx = index_of(source, x, y)?;
            if selection.contains(x, y) {
                continue;
            }
            if let Some(px) = buf.get_pixel(x as usize, y as usize) {
                clear[idx * 4] = px.r;
                clear[idx * 4 + 1] = px.g;
                clear[idx * 4 + 2] = px.b;
                clear[idx * 4 + 3] = px.a;
            }
        }
    }
    buf.blit_region(source, &clear);

    let mut paste = vec![0u8; dest.area().max(0) as usize * 4];
    for y in dest.y..dest.bottom() {
        for x in dest.x..dest.right() {
            let idx = index_of(dest, x, y)?;
            if !mask[idx] {
                if let Some(union_idx) = index_of(union, x, y) {
                    paste[idx * 4..idx * 4 + 4]
                        .copy_from_slice(&before[union_idx * 4..union_idx * 4 + 4]);
                }
                continue;
            }
            // Inverse of the forward rotation: dest cell (dlx, dly) reads
            // source cell (col = dly, row = h-1-dlx).
            let sx = source.x + (y - dest.y);
            let sy = source.y + (source.h - 1 - (x - dest.x));
            if let Some(src_idx) = index_of(source, sx, sy) {
                paste[idx * 4..idx * 4 + 4]
                    .copy_from_slice(&selection.snapshot[src_idx * 4..src_idx * 4 + 4]);
            }
        }
    }
    buf.blit_region(dest, &paste);
    Some(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::color::Color;
    use crate::core::model::LayerStack;
    use crate::core::undo::{Command, CommandContext, UndoStack};

    fn ctx(layers: &mut LayerStack) -> CommandContext<'_> {
        CommandContext { layers }
    }

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
    fn capture_clips_rect_to_canvas() {
        let mut buf = PixelBuffer::new(4, 4);
        pattern_fill(&mut buf);
        let sel = Selection::capture(&buf, Rect2i::new(2, 2, 4, 4)).unwrap();
        assert_eq!(sel.rect(), Rect2i::new(2, 2, 2, 2));
        assert!(sel.is_rectangular());
        assert_eq!(
            sel.snapshot(),
            buf.export_region(Rect2i::new(2, 2, 2, 2), None).unwrap()
        );
    }

    #[test]
    fn capture_rejects_empty_rect() {
        let buf = PixelBuffer::new(4, 4);
        assert!(Selection::capture(&buf, Rect2i::ZERO).is_none());
        assert!(Selection::capture(&buf, Rect2i::new(5, 5, 2, 2)).is_none());
        assert!(Selection::capture(&buf, Rect2i::new(-2, -2, 2, 2)).is_none());
    }

    #[test]
    fn capture_snapshot_matches_buffer() {
        let mut buf = PixelBuffer::new(3, 3);
        pattern_fill(&mut buf);
        let sel = Selection::capture(&buf, Rect2i::new(1, 0, 2, 2)).unwrap();
        assert_eq!(
            sel.snapshot(),
            buf.export_region(Rect2i::new(1, 0, 2, 2), None).unwrap()
        );
    }

    #[test]
    fn contains_hit_test() {
        let buf = PixelBuffer::new(4, 4);
        let sel = Selection::capture(&buf, Rect2i::new(1, 1, 2, 2)).unwrap();
        assert!(sel.contains(1, 1));
        assert!(sel.contains(2, 2));
        assert!(!sel.contains(0, 0));
        assert!(!sel.contains(3, 3));
    }

    #[test]
    fn destination_clamps_to_canvas() {
        let buf = PixelBuffer::new(4, 4);
        let sel = Selection::capture(&buf, Rect2i::new(0, 0, 2, 2)).unwrap();
        assert_eq!(sel.destination(1, 1), Rect2i::new(1, 1, 2, 2));
        assert_eq!(sel.destination(2, 0), Rect2i::new(2, 0, 2, 2));
        // Partially off-canvas → clipped to the canvas.
        assert_eq!(sel.destination(3, 0), Rect2i::new(3, 0, 1, 2));
        // Fully off-canvas → empty rect (no-op move).
        assert!(sel.destination(10, 10).is_empty());
        assert!(sel.destination(-10, -10).is_empty());
    }

    #[test]
    fn capture_mask_round_trip_and_fast_path() {
        let mut buf = PixelBuffer::new(4, 4);
        pattern_fill(&mut buf);
        // Diagonal mask over a 2×2 box.
        let sel = Selection::capture_mask(
            &buf,
            Rect2i::new(1, 1, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        assert_eq!(sel.rect(), Rect2i::new(1, 1, 2, 2));
        assert!(!sel.is_rectangular());
        assert!(sel.contains(1, 1));
        assert!(!sel.contains(2, 1));
        assert!(!sel.contains(1, 2));
        assert!(sel.contains(2, 2));
        assert_eq!(sel.pixel_count(), 2);
        // Holes are zeroed in the snapshot.
        assert_eq!(&sel.snapshot()[4..8], &[0, 0, 0, 0]);

        // All-true mask collapses to the rectangular fast path.
        let rect_sel =
            Selection::capture_mask(&buf, Rect2i::new(0, 0, 2, 2), vec![true; 4]).unwrap();
        assert!(rect_sel.is_rectangular());

        // No selected pixel → None.
        assert!(Selection::capture_mask(&buf, Rect2i::new(0, 0, 2, 2), vec![false; 4]).is_none());
    }

    #[test]
    fn union_and_subtract_shapes() {
        let buf = PixelBuffer::new(6, 6);
        let a = Selection::capture(&buf, Rect2i::new(0, 0, 2, 2)).unwrap();
        let b = Selection::capture(&buf, Rect2i::new(4, 0, 2, 2)).unwrap();
        let (bbox, mask) = a.union_shape(&b).unwrap();
        assert_eq!(bbox, Rect2i::new(0, 0, 6, 2));
        let union = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert!(union.contains(0, 0));
        assert!(union.contains(4, 0));
        assert!(!union.contains(3, 0));

        let (bbox, mask) = a.subtract_shape(&b).unwrap();
        assert_eq!(bbox, Rect2i::new(0, 0, 2, 2));
        let sub = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert!(sub.contains(0, 0));
        assert!(!sub.contains(4, 0));

        // Subtracting everything leaves nothing.
        assert!(a.subtract_shape(&a).is_none());
    }

    #[test]
    fn outline_segments_traces_an_l_shape() {
        let buf = PixelBuffer::new(4, 4);
        // Three pixels: (0,0), (1,0), (0,1) — an L.
        let sel =
            Selection::capture_mask(&buf, Rect2i::new(0, 0, 2, 2), vec![true, true, true, false])
                .unwrap();
        let outline = sel.outline_segments();
        // 8 boundary unit edges: 3 top, 3 left, 1 right-of-(1,0), 1 bottom-of-(0,1).
        assert_eq!(outline.len(), 8, "outline must trace the mask boundary");
        assert!(outline.contains(&((1, 0), (2, 0))));
        assert!(outline.contains(&((1, 1), (2, 1))));
        assert!(outline.contains(&((1, 1), (1, 2))));
    }

    #[test]
    fn move_commit_round_trip() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            pattern_fill(buf);
        }
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 2)).unwrap();
        let dest = sel.destination(2, 0);

        let mut cmd =
            move_selection_to_command(&sel, dest, lid, &mut layers.active_layer_mut().buffer)
                .unwrap();
        assert_eq!(cmd.name(), "Move");

        // Source cleared, destination holds the snapshot.
        assert_eq!(
            layers.active_layer().buffer.get_pixel(0, 0),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 0),
            Some(Color::rgba(0, 0, 0, 255))
        );

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);

        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(0, 0),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 0),
            Some(Color::rgba(0, 0, 0, 255))
        );
    }

    #[test]
    fn move_noop_returns_none() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 2)).unwrap();
        assert!(move_selection_to_command(
            &sel,
            sel.rect(),
            lid,
            &mut layers.active_layer_mut().buffer
        )
        .is_none());
    }

    #[test]
    fn mask_selection_move_commits_the_translated_mask() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            for y in 0..6 {
                for x in 0..6 {
                    buf.set_pixel(x, y, Color::rgba(x as u8, y as u8, 0, 255));
                }
            }
        }
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        // A 2×2 diagonal mask at (1,1): only (1,1) and (2,2) are selected.
        let sel = Selection::capture_mask(
            &layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        let dest = sel.destination(2, 0);

        let mut cmd =
            move_selection_to_command(&sel, dest, lid, &mut layers.active_layer_mut().buffer)
                .unwrap();
        assert_eq!(cmd.name(), "Move");

        let buf = &layers.active_layer().buffer;
        // Selected source pixels are cleared.
        assert_eq!(buf.get_pixel(1, 1), Some(Color::TRANSPARENT));
        assert_eq!(buf.get_pixel(2, 2), Some(Color::TRANSPARENT));
        // Unselected pixels inside the source bbox are byte-identical.
        assert_eq!(buf.get_pixel(2, 1), Some(Color::rgba(2, 1, 0, 255)));
        assert_eq!(buf.get_pixel(1, 2), Some(Color::rgba(1, 2, 0, 255)));
        // The translated mask lands at (3,1) and (4,2); its holes stay holes.
        assert_eq!(buf.get_pixel(3, 1), Some(Color::rgba(1, 1, 0, 255)));
        assert_eq!(buf.get_pixel(4, 2), Some(Color::rgba(2, 2, 0, 255)));
        assert_eq!(
            buf.get_pixel(4, 1),
            Some(Color::rgba(4, 1, 0, 255)),
            "the destination hole must keep the underlying pixel"
        );
        assert_eq!(
            buf.get_pixel(3, 2),
            Some(Color::rgba(3, 2, 0, 255)),
            "the destination hole must keep the underlying pixel"
        );

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(3, 1),
            Some(Color::rgba(1, 1, 0, 255))
        );
    }

    #[test]
    fn mask_selection_move_is_one_undo_step_over_the_union() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel = Selection::capture_mask(
            &layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        let dest = sel.destination(2, 0);

        let mut stack = UndoStack::new();
        let cmd = move_selection_to_command(&sel, dest, lid, &mut layers.active_layer_mut().buffer)
            .unwrap();
        stack.push(Box::new(cmd));
        assert_eq!(stack.top_undo_name(), Some("Move"));
        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.as_bytes(),
            &original[..],
            "one undo must restore both the cleared source and the pasted mask"
        );
    }

    #[test]
    fn mask_selection_move_noop_returns_none() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let sel = Selection::capture_mask(
            &layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        assert!(move_selection_to_command(
            &sel,
            sel.rect(),
            lid,
            &mut layers.active_layer_mut().buffer
        )
        .is_none());
    }

    #[test]
    fn mask_selection_move_keeps_a_destination_hole_over_a_cleared_source_pixel() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            for y in 0..6 {
                for x in 0..6 {
                    buf.set_pixel(x, y, Color::rgba(x as u8, y as u8, 0, 255));
                }
            }
        }
        // Selected: (1,1), (2,1), (2,2).  Moving +1,+0 puts the destination
        // hole at (2,2) — a selected source pixel that the clear wipes.
        let sel = Selection::capture_mask(
            &layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 2),
            vec![true, true, false, true],
        )
        .unwrap();
        let dest = sel.destination(1, 0);

        move_selection_to_command(&sel, dest, lid, &mut layers.active_layer_mut().buffer).unwrap();

        let buf = &layers.active_layer().buffer;
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::rgba(2, 2, 0, 255)),
            "the destination hole must keep the ORIGINAL pixel, not the cleared one"
        );
        assert_eq!(buf.get_pixel(2, 1), Some(Color::rgba(1, 1, 0, 255)));
        assert_eq!(buf.get_pixel(3, 1), Some(Color::rgba(2, 1, 0, 255)));
        assert_eq!(buf.get_pixel(3, 2), Some(Color::rgba(2, 2, 0, 255)));
    }

    #[test]
    fn mask_selection_move_fully_off_canvas_returns_none() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let sel = Selection::capture_mask(
            &layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        assert!(move_selection_to_command(
            &sel,
            Rect2i::new(20, 20, 2, 2),
            lid,
            &mut layers.active_layer_mut().buffer
        )
        .is_none());
    }

    #[test]
    fn move_transparent_snapshot_returns_none() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 2)).unwrap();
        let dest = sel.destination(1, 0);
        assert!(
            move_selection_to_command(&sel, dest, lid, &mut layers.active_layer_mut().buffer)
                .is_none()
        );
    }

    #[test]
    fn move_overlapping_source_and_dest() {
        let mut layers = LayerStack::new(3, 3);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(1, 0, 0));
            buf.set_pixel(1, 0, Color::rgb(2, 0, 0));
            buf.set_pixel(0, 1, Color::rgb(3, 0, 0));
            buf.set_pixel(1, 1, Color::rgb(4, 0, 0));
        }
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 2)).unwrap();
        let dest = sel.destination(1, 1);

        let mut cmd =
            move_selection_to_command(&sel, dest, lid, &mut layers.active_layer_mut().buffer)
                .unwrap();

        // (1,1) now holds the old (0,0) pixel; (0,0) is cleared.
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::rgb(1, 0, 0))
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(0, 0),
            Some(Color::TRANSPARENT)
        );

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
    }

    #[test]
    fn move_via_undo_stack() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            pattern_fill(buf);
        }
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 2)).unwrap();
        let dest = sel.destination(0, 2);

        let mut stack = UndoStack::new();
        let cmd = move_selection_to_command(&sel, dest, lid, &mut layers.active_layer_mut().buffer)
            .unwrap();
        stack.push(Box::new(cmd));
        assert_eq!(stack.top_undo_name(), Some("Move"));

        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);

        assert!(stack.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(0, 2),
            Some(Color::rgba(0, 0, 0, 255))
        );
    }

    // -----------------------------------------------------------------------
    // Degenerate rects and canvas clamping (§21)
    // -----------------------------------------------------------------------

    #[test]
    fn one_by_one_selection_is_valid() {
        let buf = PixelBuffer::new(4, 4);
        let sel = Selection::capture(&buf, Rect2i::new(2, 2, 1, 1)).unwrap();
        assert_eq!(sel.rect(), Rect2i::new(2, 2, 1, 1));
        assert!(sel.is_rectangular());
        assert_eq!(sel.pixel_count(), 1);
        assert!(sel.contains(2, 2));
        assert!(!sel.contains(3, 2));
    }

    #[test]
    fn zero_and_negative_sized_rects_are_rejected() {
        let buf = PixelBuffer::new(4, 4);
        for rect in [
            Rect2i::new(1, 1, 0, 3),
            Rect2i::new(1, 1, 3, 0),
            Rect2i::new(1, 1, -2, 2),
            Rect2i::new(1, 1, 2, -2),
        ] {
            assert!(
                Selection::capture(&buf, rect).is_none(),
                "{rect:?} must not produce a selection"
            );
        }
    }

    #[test]
    fn capture_clamps_to_every_canvas_edge() {
        let mut buf = PixelBuffer::new(4, 4);
        pattern_fill(&mut buf);
        for (asked, expected) in [
            (Rect2i::new(-3, -3, 3, 3), Rect2i::new(0, 0, 0, 0)),
            (Rect2i::new(3, 0, 5, 2), Rect2i::new(3, 0, 1, 2)),
            (Rect2i::new(0, 3, 2, 5), Rect2i::new(0, 3, 2, 1)),
            (Rect2i::new(-5, -5, 2, 2), Rect2i::ZERO),
        ] {
            if expected.is_empty() {
                assert!(Selection::capture(&buf, asked).is_none(), "{asked:?}");
            } else {
                let sel = Selection::capture(&buf, asked).unwrap();
                assert_eq!(sel.rect(), expected, "{asked:?}");
            }
        }
    }

    #[test]
    fn an_empty_canvas_has_no_selection() {
        let buf = PixelBuffer::new(0, 0);
        assert!(Selection::capture(&buf, Rect2i::new(0, 0, 1, 1)).is_none());
    }

    #[test]
    fn from_mask_rejects_a_bitmap_whose_length_does_not_match_the_rect() {
        let buf = PixelBuffer::new(4, 4);
        assert!(Selection::capture_mask(&buf, Rect2i::new(0, 0, 2, 2), vec![true; 3]).is_none());
        assert!(Selection::capture_mask(&buf, Rect2i::new(0, 0, 2, 2), vec![true; 5]).is_none());
        assert!(Selection::capture_mask(&buf, Rect2i::new(0, 0, 2, 2), vec![true; 4]).is_some());
    }

    // -----------------------------------------------------------------------
    // Reverse / off-canvas drags (§2, §12, §21)
    // -----------------------------------------------------------------------

    #[test]
    fn a_negative_drag_translates_the_selection_the_other_way() {
        let mut buf = PixelBuffer::new(8, 8);
        pattern_fill(&mut buf);
        let sel = Selection::capture(&buf, Rect2i::new(4, 4, 2, 2)).unwrap();
        let moved = sel.translated_to(-3, -3, &buf).unwrap();
        assert_eq!(moved.rect(), Rect2i::new(1, 1, 2, 2));
        assert!(moved.is_rectangular());
    }

    #[test]
    fn a_fully_off_canvas_move_produces_nothing() {
        let buf = PixelBuffer::new(4, 4);
        let sel = Selection::capture(&buf, Rect2i::new(0, 0, 2, 2)).unwrap();
        assert!(sel.translated_to(10, 10, &buf).is_none());
        assert!(sel.translated_to(-10, -10, &buf).is_none());
        assert!(sel.translated_shape(10, 10).is_none());
    }

    #[test]
    fn a_partially_off_canvas_move_clamps_to_the_edge() {
        let mut buf = PixelBuffer::new(4, 4);
        pattern_fill(&mut buf);
        let sel = Selection::capture(&buf, Rect2i::new(0, 0, 2, 2)).unwrap();
        let moved = sel.translated_to(3, 0, &buf).unwrap();
        assert_eq!(moved.rect(), Rect2i::new(3, 0, 1, 2));
    }

    #[test]
    fn moving_a_mask_selection_keeps_its_holes() {
        let mut buf = PixelBuffer::new(6, 6);
        pattern_fill(&mut buf);
        let sel = Selection::capture_mask(
            &buf,
            Rect2i::new(1, 1, 2, 2),
            vec![true, false, false, true],
        )
        .unwrap();
        let moved = sel.translated_to(2, 0, &buf).unwrap();
        assert_eq!(moved.rect(), Rect2i::new(3, 1, 2, 2));
        assert!(!moved.is_rectangular());
        assert!(moved.contains(3, 1));
        assert!(!moved.contains(4, 1));
        assert!(!moved.contains(3, 2));
        assert!(moved.contains(4, 2));
    }

    #[test]
    fn a_rectangular_move_never_materializes_a_mask() {
        let mut buf = PixelBuffer::new(64, 64);
        pattern_fill(&mut buf);
        let sel = Selection::capture(&buf, Rect2i::new(0, 0, 32, 32)).unwrap();
        let moved = sel.translated_to(4, 4, &buf).unwrap();
        assert_eq!(
            moved.selection_mask(),
            &SelectionMask::Rectangle,
            "a translated rectangle must stay on the maskless fast path"
        );
    }

    // -----------------------------------------------------------------------
    // Add / Subtract / Intersect (§5, §6, §7, §21)
    // -----------------------------------------------------------------------

    #[test]
    fn union_of_overlapping_rectangles_fills_the_overlap_once() {
        let buf = PixelBuffer::new(6, 6);
        let a = Selection::capture(&buf, Rect2i::new(0, 0, 3, 2)).unwrap();
        let b = Selection::capture(&buf, Rect2i::new(2, 0, 3, 2)).unwrap();
        let (bbox, mask) = a.union_shape(&b).unwrap();
        assert_eq!(bbox, Rect2i::new(0, 0, 5, 2));
        let union = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert!(union.is_rectangular());
        assert_eq!(union.pixel_count(), 10);
    }

    #[test]
    fn union_of_disjoint_rectangles_leaves_the_gap_unselected() {
        let buf = PixelBuffer::new(6, 6);
        let a = Selection::capture(&buf, Rect2i::new(0, 0, 2, 2)).unwrap();
        let b = Selection::capture(&buf, Rect2i::new(4, 0, 2, 2)).unwrap();
        let (bbox, mask) = a.union_shape(&b).unwrap();
        let union = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert!(!union.is_rectangular(), "the gap must stay unselected");
        assert_eq!(union.pixel_count(), 8);
        assert!(!union.contains(2, 0));
        assert!(!union.contains(3, 0));
    }

    #[test]
    fn subtracting_a_disjoint_rect_is_the_identity() {
        let buf = PixelBuffer::new(6, 6);
        let a = Selection::capture(&buf, Rect2i::new(0, 0, 2, 2)).unwrap();
        let b = Selection::capture(&buf, Rect2i::new(4, 0, 2, 2)).unwrap();
        let (bbox, mask) = a.subtract_shape(&b).unwrap();
        let left = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert_eq!(left.rect(), a.rect());
        assert_eq!(left.pixel_count(), a.pixel_count());
    }

    #[test]
    fn subtract_can_punch_a_hole_inside_a_rectangle() {
        let buf = PixelBuffer::new(6, 6);
        let outer = Selection::capture(&buf, Rect2i::new(0, 0, 5, 5)).unwrap();
        let inner = Selection::capture(&buf, Rect2i::new(2, 2, 1, 1)).unwrap();
        let (bbox, mask) = outer.subtract_shape(&inner).unwrap();
        let ring = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert!(!ring.is_rectangular());
        assert!(!ring.contains(2, 2), "the subtracted cell is gone");
        assert!(ring.contains(0, 0));
        assert!(ring.contains(4, 4));
        assert_eq!(ring.pixel_count(), 24);
    }

    #[test]
    fn subtract_everything_leaves_nothing() {
        let buf = PixelBuffer::new(6, 6);
        let a = Selection::capture(&buf, Rect2i::new(0, 0, 3, 3)).unwrap();
        assert!(a.subtract_shape(&a).is_none());
        let same = Selection::capture_mask(&buf, Rect2i::new(0, 0, 3, 3), vec![true; 9]).unwrap();
        assert!(same.subtract_shape(&a).is_none());
    }

    #[test]
    fn intersect_of_disjoint_rects_is_none() {
        let buf = PixelBuffer::new(6, 6);
        let a = Selection::capture(&buf, Rect2i::new(0, 0, 2, 2)).unwrap();
        let b = Selection::capture(&buf, Rect2i::new(4, 0, 2, 2)).unwrap();
        assert!(a.intersect_shape(&b).is_none());
    }

    #[test]
    fn intersect_keeps_only_the_shared_box() {
        let buf = PixelBuffer::new(6, 6);
        let a = Selection::capture(&buf, Rect2i::new(0, 0, 4, 2)).unwrap();
        let b = Selection::capture(&buf, Rect2i::new(2, 0, 4, 2)).unwrap();
        let (bbox, mask) = a.intersect_shape(&b).unwrap();
        assert_eq!(bbox, Rect2i::new(2, 0, 2, 2));
        let shared = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert_eq!(shared.pixel_count(), 4);
    }

    #[test]
    fn intersect_of_masks_keeps_only_shared_pixels() {
        let buf = PixelBuffer::new(4, 4);
        let a = Selection::capture_mask(
            &buf,
            Rect2i::new(0, 0, 2, 2),
            vec![true, true, false, false],
        )
        .unwrap();
        let b = Selection::capture_mask(
            &buf,
            Rect2i::new(0, 0, 2, 2),
            vec![false, true, false, true],
        )
        .unwrap();
        let (bbox, mask) = a.intersect_shape(&b).unwrap();
        let shared = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert_eq!(shared.pixel_count(), 1);
        assert!(shared.contains(1, 0));
        assert!(!shared.contains(0, 0));
    }

    // -----------------------------------------------------------------------
    // Invert (§16)
    // -----------------------------------------------------------------------

    #[test]
    fn invert_covers_the_canvas_except_the_selection() {
        let mut buf = PixelBuffer::new(4, 4);
        pattern_fill(&mut buf);
        let sel = Selection::capture(&buf, Rect2i::new(1, 1, 2, 2)).unwrap();
        let (bbox, mask) = sel.inverted_shape();
        assert_eq!(bbox, Rect2i::new(0, 0, 4, 4));
        let inverted = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert_eq!(inverted.rect(), Rect2i::new(0, 0, 4, 4));
        assert!(!inverted.is_rectangular(), "the source stays excluded");
        assert!(!inverted.contains(1, 1));
        assert!(!inverted.contains(2, 2));
        assert!(inverted.contains(0, 0));
        assert!(inverted.contains(3, 3));
        assert_eq!(inverted.pixel_count(), 12);
    }

    #[test]
    fn inverting_a_full_canvas_selection_leaves_nothing_selected() {
        let buf = PixelBuffer::new(4, 4);
        let sel = Selection::capture(&buf, Rect2i::new(0, 0, 4, 4)).unwrap();
        let (bbox, mask) = sel.inverted_shape();
        assert!(Selection::capture_mask(&buf, bbox, mask).is_none());
    }

    #[test]
    fn inverting_twice_restores_the_selected_pixels() {
        let mut buf = PixelBuffer::new(6, 6);
        pattern_fill(&mut buf);
        let sel = Selection::capture_mask(&buf, Rect2i::new(1, 1, 3, 3), {
            let mut m = vec![false; 9];
            m[0] = true;
            m[4] = true;
            m
        })
        .unwrap();
        let (bbox, mask) = sel.inverted_shape();
        let once = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert_eq!(once.pixel_count(), 36 - sel.pixel_count());

        let (bbox, mask) = once.inverted_shape();
        let twice = Selection::capture_mask(&buf, bbox, mask).unwrap();
        assert_eq!(twice.pixel_count(), sel.pixel_count());
        for y in 0..6 {
            for x in 0..6 {
                assert_eq!(
                    twice.contains(x, y),
                    sel.contains(x, y),
                    "double invert must restore the selected set at ({x},{y})"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Delete + undo/redo (§8, §18, §25)
    // -----------------------------------------------------------------------

    #[test]
    fn delete_clears_only_the_selected_pixels() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(1, 1, Color::rgb(9, 9, 9));
            buf.set_pixel(2, 1, Color::rgb(9, 9, 9));
        }
        let sel = Selection::capture_mask(
            &layers.active_layer().buffer,
            Rect2i::new(1, 1, 2, 1),
            vec![true, false],
        )
        .unwrap();
        let mut cmd =
            delete_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).unwrap();
        assert_eq!(cmd.name(), "Delete");
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT),
            "the selected cell is cleared"
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 1),
            Some(Color::rgb(9, 9, 9)),
            "the unselected cell inside the bounding box survives"
        );
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::rgb(9, 9, 9))
        );
    }

    #[test]
    fn delete_of_a_rectangle_clears_every_cell_in_it() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            for y in 1..3 {
                for x in 1..3 {
                    buf.set_pixel(x, y, Color::rgb(7, 7, 7));
                }
            }
            buf.set_pixel(3, 3, Color::rgb(7, 7, 7));
        }
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(1, 1, 2, 2)).unwrap();
        let mut cmd =
            delete_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).unwrap();
        for y in 1..3 {
            for x in 1..3 {
                assert_eq!(
                    layers.active_layer().buffer.get_pixel(x, y),
                    Some(Color::TRANSPARENT)
                );
            }
        }
        assert_eq!(
            layers.active_layer().buffer.get_pixel(3, 3),
            Some(Color::rgb(7, 7, 7)),
            "a pixel outside the selection is untouched"
        );
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
    }

    #[test]
    fn delete_leaves_the_selection_usable_afterwards() {
        let mut layers = LayerStack::new(4, 4);
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(1, 1, 2, 2)).unwrap();
        let refreshed = sel
            .recaptured(&layers.active_layer().buffer)
            .expect("the shape still selects pixels after a delete");
        assert_eq!(refreshed.rect(), sel.rect());
        assert!(refreshed.is_rectangular());
        assert_eq!(refreshed.pixel_count(), sel.pixel_count());
    }

    #[test]
    fn deleting_an_already_transparent_selection_is_a_noop() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 2)).unwrap();
        assert!(
            delete_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).is_none(),
            "clearing an empty region must not push an undo step"
        );
    }

    #[test]
    fn delete_pushes_exactly_one_undo_step() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(1, 1, 3, 3)).unwrap();

        let mut stack = UndoStack::new();
        let cmd =
            delete_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).unwrap();
        stack.push(Box::new(cmd));
        assert_eq!(stack.top_undo_name(), Some("Delete"));

        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.as_bytes(),
            &original[..],
            "one undo restores every deleted pixel"
        );
        assert!(stack.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
    }

    #[test]
    fn delete_then_undo_then_delete_again_is_stable() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(2, 2, 2, 2)).unwrap();

        let mut cmd =
            delete_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).unwrap();
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
    }

    // -----------------------------------------------------------------------
    // Shape-preserving recapture
    // -----------------------------------------------------------------------

    #[test]
    fn recaptured_keeps_the_mask_but_refreshes_the_pixels() {
        let mut layers = LayerStack::new(4, 4);
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(1, 1, 1));
            buf.set_pixel(1, 1, Color::rgb(2, 2, 2));
        }
        let sel =
            Selection::capture_mask(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 2), {
                let mut m = vec![false; 4];
                m[0] = true;
                m[3] = true;
                m
            })
            .unwrap();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(9, 9, 9));
        }
        let after = sel.recaptured(&layers.active_layer().buffer).unwrap();
        assert_eq!(after.rect(), sel.rect());
        assert_eq!(after.pixel_count(), sel.pixel_count());
        assert_eq!(after.contains(0, 0), sel.contains(0, 0));
        assert_eq!(&after.snapshot()[0..4], &[9, 9, 9, 255]);
    }

    // -----------------------------------------------------------------------
    // Flip (Ctrl+F / Shift+F) + rotate (Alt+F)
    // -----------------------------------------------------------------------

    #[test]
    fn mirrored_shape_swaps_left_and_right() {
        let buf = PixelBuffer::new(4, 2);
        let sel = Selection::capture_mask(&buf, Rect2i::new(0, 0, 3, 1), vec![true, false, true])
            .unwrap();
        let (bbox, mask) = sel.mirrored_shape(true);
        assert_eq!(bbox, sel.rect());
        let at = |m: &[bool], i: usize| m[i];
        assert!(at(&mask, 0), "the left pixel moves to the right");
        assert!(at(&mask, 2));
        assert!(!at(&mask, 1), "the middle pixel stays unselected");
    }

    #[test]
    fn mirrored_shape_is_its_own_inverse() {
        let buf = PixelBuffer::new(4, 4);
        let original = vec![true, false, true, false, true, false, false, false, true];
        let sel = Selection::capture_mask(&buf, Rect2i::new(0, 0, 3, 3), original.clone()).unwrap();
        let (_, once) = sel.mirrored_shape(true);
        assert_ne!(once, original, "one mirror must differ from the source");
        let (_, twice) = Selection::capture_mask(&buf, sel.rect(), once)
            .unwrap()
            .mirrored_shape(true);
        assert_eq!(twice, original, "mirroring twice must restore the source");
    }

    // -----------------------------------------------------------------------
    // Rotate (90° CW)
    // -----------------------------------------------------------------------

    #[test]
    fn rotate_shape_square_returns_same_rect() {
        let buf = PixelBuffer::new(8, 8);
        let sel = Selection::capture(&buf, Rect2i::new(2, 3, 4, 4)).unwrap();
        let (bbox, mask) = sel.rotate_shape();
        assert_eq!(bbox, Rect2i::new(2, 3, 4, 4));
        assert_eq!(mask, vec![true; 16], "a square rotates onto itself");
    }

    #[test]
    fn rotate_shape_nonsquare_swaps_dims() {
        let buf = PixelBuffer::new(8, 8);
        let sel = Selection::capture(&buf, Rect2i::new(1, 2, 4, 2)).unwrap();
        let (bbox, mask) = sel.rotate_shape();
        assert_eq!(bbox, Rect2i::new(2, 1, 2, 4));
        assert_eq!(mask, vec![true; 8]);
    }

    #[test]
    fn rotate_shape_maps_mask_correctly() {
        let buf = PixelBuffer::new(4, 4);
        // Γ shape: full top row plus the bottom-left pixel.
        let sel = Selection::capture_mask(
            &buf,
            Rect2i::new(0, 0, 3, 2),
            vec![true, true, true, true, false, false],
        )
        .unwrap();
        let (bbox, mask) = sel.rotate_shape();
        assert_eq!(bbox, Rect2i::new(0, 0, 2, 3));
        // 90° CW: the top bar becomes the right column, the leg becomes the
        // top bar.
        assert_eq!(mask, vec![true, true, false, true, false, true]);
    }

    #[test]
    fn rotate_shape_four_times_is_identity() {
        let buf = PixelBuffer::new(8, 8);
        let original = vec![true, false, true, false, true, true, false, false, true];
        let sel = Selection::capture_mask(&buf, Rect2i::new(1, 1, 3, 3), original.clone()).unwrap();
        let (rect, mask) = sel.rotate_shape();
        let once = Selection::capture_mask(&buf, rect, mask).unwrap();
        let (rect, mask) = once.rotate_shape();
        let twice = Selection::capture_mask(&buf, rect, mask).unwrap();
        let (rect, mask) = twice.rotate_shape();
        let thrice = Selection::capture_mask(&buf, rect, mask).unwrap();
        let (rect, mask) = thrice.rotate_shape();
        assert_eq!(rect, sel.rect());
        assert_eq!(mask, original, "four quarter-turns must restore the source");
    }

    #[test]
    fn flip_horizontal_mirrors_the_selected_pixels() {
        let mut layers = LayerStack::new(4, 2);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(1, 0, 0));
            buf.set_pixel(3, 0, Color::rgb(2, 0, 0));
        }
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 4, 1)).unwrap();
        let mut cmd =
            flip_selected_command(&sel, true, lid, &mut layers.active_layer_mut().buffer).unwrap();
        assert_eq!(cmd.name(), "Flip");
        let buf = &layers.active_layer().buffer;
        assert_eq!(
            buf.get_pixel(3, 0),
            Some(Color::rgb(1, 0, 0)),
            "left moved right"
        );
        assert_eq!(
            buf.get_pixel(0, 0),
            Some(Color::rgb(2, 0, 0)),
            "right moved left"
        );
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(0, 0),
            Some(Color::rgb(1, 0, 0))
        );
    }

    #[test]
    fn flip_vertical_mirrors_top_to_bottom() {
        let mut layers = LayerStack::new(2, 3);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(1, 0, 0));
            buf.set_pixel(0, 2, Color::rgb(2, 0, 0));
        }
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 1, 3)).unwrap();
        let mut cmd =
            flip_selected_command(&sel, false, lid, &mut layers.active_layer_mut().buffer).unwrap();
        let buf = &layers.active_layer().buffer;
        assert_eq!(buf.get_pixel(0, 2), Some(Color::rgb(1, 0, 0)));
        assert_eq!(buf.get_pixel(0, 0), Some(Color::rgb(2, 0, 0)));
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(0, 0),
            Some(Color::rgb(1, 0, 0))
        );
    }

    #[test]
    fn flip_leaves_pixels_outside_the_selection_alone() {
        let mut layers = LayerStack::new(4, 1);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(7, 0, 0));
            buf.set_pixel(3, 0, Color::rgb(8, 0, 0));
        }
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 1)).unwrap();
        let _ = flip_selected_command(&sel, true, lid, &mut layers.active_layer_mut().buffer);
        assert_eq!(
            layers.active_layer().buffer.get_pixel(3, 0),
            Some(Color::rgb(8, 0, 0)),
            "a pixel past the selection's box must not move"
        );
    }

    #[test]
    fn flipping_a_symmetric_region_pushes_no_undo_step() {
        let mut layers = LayerStack::new(4, 1);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(3, 0, 0));
            buf.set_pixel(3, 0, Color::rgb(3, 0, 0));
        }
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 4, 1)).unwrap();
        assert!(
            flip_selected_command(&sel, true, lid, &mut layers.active_layer_mut().buffer).is_none(),
            "a mirror-symmetric region must not grow the undo stack"
        );
    }

    #[test]
    fn flip_pushes_exactly_one_undo_step() {
        let mut layers = LayerStack::new(4, 1);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.set_pixel(0, 0, Color::rgb(1, 0, 0));
            buf.set_pixel(3, 0, Color::rgb(2, 0, 0));
        }
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 4, 1)).unwrap();
        let mut stack = UndoStack::new();
        let cmd =
            flip_selected_command(&sel, true, lid, &mut layers.active_layer_mut().buffer).unwrap();
        stack.push(Box::new(cmd));
        assert_eq!(stack.top_undo_name(), Some("Flip"));
        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
        assert!(stack.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(3, 0),
            Some(Color::rgb(1, 0, 0))
        );
    }

    #[test]
    fn rotate_selected_command_rectangular_moves_pixels() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            // A 2×4 box of distinct pixels.
            let cells = [
                (2, 2, 1),
                (3, 2, 2),
                (2, 3, 3),
                (3, 3, 4),
                (2, 4, 5),
                (3, 4, 6),
                (2, 5, 7),
                (3, 5, 8),
            ];
            for (x, y, v) in cells {
                buf.set_pixel(x, y, Color::rgb(v, 0, 0));
            }
        }
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(2, 2, 2, 4)).unwrap();
        let mut cmd =
            rotate_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).unwrap();
        assert_eq!(cmd.name(), "Rotate");
        let buf = &layers.active_layer().buffer;
        // 90° CW: source (col, row) → dest (h-1-row, col); the 2×4 box at
        // (2, 2) becomes a 4×2 box at (1, 3).
        assert_eq!(
            buf.get_pixel(4, 3),
            Some(Color::rgb(1, 0, 0)),
            "source top-left moves to dest top-right"
        );
        assert_eq!(
            buf.get_pixel(1, 3),
            Some(Color::rgb(7, 0, 0)),
            "source bottom-left moves to dest top-left"
        );
        assert_eq!(
            buf.get_pixel(4, 4),
            Some(Color::rgb(2, 0, 0)),
            "source top-right moves to dest bottom-right"
        );
        assert_eq!(
            buf.get_pixel(1, 4),
            Some(Color::rgb(8, 0, 0)),
            "source bottom-right moves to dest bottom-left"
        );
        // Source cells the rotated box no longer covers are cleared.
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::rgba(0, 0, 0, 0)),
            "a source cell outside the dest box is cleared"
        );
        assert_eq!(
            buf.get_pixel(3, 5),
            Some(Color::rgba(0, 0, 0, 0)),
            "a source cell outside the dest box is cleared"
        );
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 2),
            Some(Color::rgb(1, 0, 0)),
            "undo restores the source pixels"
        );
    }

    #[test]
    fn rotate_selected_command_mask_preserves_unselected() {
        let mut layers = LayerStack::new(6, 6);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            for y in 0..6 {
                for x in 0..6 {
                    buf.set_pixel(x, y, Color::rgb(7, 8, 9));
                }
            }
            buf.set_pixel(1, 1, Color::rgb(1, 0, 0));
            buf.set_pixel(2, 1, Color::rgb(2, 0, 0));
            buf.set_pixel(3, 1, Color::rgb(3, 0, 0));
            buf.set_pixel(1, 2, Color::rgb(4, 0, 0));
        }
        // L shape: full top row plus the bottom-left pixel.
        let sel = Selection::capture_mask(
            &layers.active_layer().buffer,
            Rect2i::new(1, 1, 3, 2),
            vec![true, true, true, true, false, false],
        )
        .unwrap();
        let mut cmd =
            rotate_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).unwrap();
        assert_eq!(cmd.name(), "Rotate");
        let buf = &layers.active_layer().buffer;
        // The selected pixels rotate 90° CW into the 2×3 dest box at (1, 1).
        assert_eq!(
            buf.get_pixel(2, 1),
            Some(Color::rgb(1, 0, 0)),
            "(1,1) moves to (2,1)"
        );
        assert_eq!(
            buf.get_pixel(1, 1),
            Some(Color::rgb(4, 0, 0)),
            "(1,2) moves to (1,1)"
        );
        assert_eq!(
            buf.get_pixel(2, 2),
            Some(Color::rgb(2, 0, 0)),
            "(2,1) moves to (2,2)"
        );
        assert_eq!(
            buf.get_pixel(2, 3),
            Some(Color::rgb(3, 0, 0)),
            "(3,1) moves to (2,3)"
        );
        // Unselected background pixels are untouched.
        assert_eq!(
            buf.get_pixel(3, 2),
            Some(Color::rgb(7, 8, 9)),
            "a source hole keeps its background"
        );
        assert_eq!(
            buf.get_pixel(1, 3),
            Some(Color::rgb(7, 8, 9)),
            "a destination hole outside the source keeps its background"
        );
        assert_eq!(
            buf.get_pixel(5, 5),
            Some(Color::rgb(7, 8, 9)),
            "background far from the selection is unchanged"
        );
        // A destination hole over a cleared source cell restores the original
        // bytes (mirrors move_mask_selection).
        assert_eq!(
            buf.get_pixel(1, 2),
            Some(Color::rgb(4, 0, 0)),
            "a destination hole over a cleared source cell keeps its original bytes"
        );
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::rgb(1, 0, 0)),
            "undo restores the original pixels"
        );
    }

    #[test]
    fn rotate_selected_command_empty_delta_returns_none() {
        let mut layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        layers
            .active_layer_mut()
            .buffer
            .set_pixel(1, 1, Color::rgb(1, 2, 3));
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(1, 1, 1, 1)).unwrap();
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        assert!(
            rotate_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).is_none(),
            "a 1×1 selection rotates onto itself: nothing changes"
        );
        assert_eq!(layers.active_layer().buffer.as_bytes(), &original[..]);
    }

    #[test]
    fn rotate_selected_command_undo_restores_exact_bytes() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(2, 2, 2, 4)).unwrap();
        let mut cmd =
            rotate_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).unwrap();
        assert_eq!(cmd.name(), "Rotate");
        assert_ne!(
            layers.active_layer().buffer.as_bytes(),
            &original[..],
            "the rotation must change the buffer"
        );
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.as_bytes(),
            &original[..],
            "undo must restore the buffer byte-for-byte"
        );
    }

    #[test]
    fn rotate_off_canvas_is_a_safe_noop() {
        // A 2×4 selection at the left edge rotates to a dest at x = -1, so the
        // union leaves the canvas and no command is produced. The buffer is left
        // untouched (D79: an off-canvas rotation must never desync or lose data).
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        pattern_fill(&mut layers.active_layer_mut().buffer);
        let original = layers.active_layer().buffer.as_bytes().to_vec();
        let sel =
            Selection::capture(&layers.active_layer().buffer, Rect2i::new(0, 0, 2, 4)).unwrap();
        assert!(
            rotate_selected_command(&sel, lid, &mut layers.active_layer_mut().buffer).is_none(),
            "a rotation whose union leaves the canvas must be a no-op"
        );
        assert_eq!(
            layers.active_layer().buffer.as_bytes(),
            &original[..],
            "the buffer is unchanged"
        );
    }
}
