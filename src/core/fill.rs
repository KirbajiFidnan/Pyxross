//! Fill tool core algorithms — contiguous flood fill and replace-all.
//!
//! D48: two fill modes — CONTIGUOUS (default) and REPLACE_ALL — with a shared
//! tolerance of 0 (exact byte equality against the seed pixel's color is the
//! match criterion).  One fill gesture produces exactly one undo step via
//! [`fill_command`].

use crate::core::buffer::PixelBuffer;
use crate::core::clip::PixelClip;
use crate::core::color::Color;
use crate::core::math::Rect2i;
use crate::core::model::LayerId;
use crate::core::undo::{DeltaRecorder, ReverseDeltaCommand};

/// The static label stamped on every fill-derived undo command.
const FILL_UNDO_NAME: &str = "Fill";

/// Contiguous 4-connectivity flood fill with tolerance 0.
///
/// Every pixel reachable from `(x, y)` through 4-connected neighbors whose
/// color exactly equals the seed pixel's color is replaced with `color`.
/// Diagonal neighbors are never filled.  The buffer is mutated in place and
/// its change tracking is updated per pixel.
///
/// Returns the minimal bounding rect of the pixels actually changed, or `None`
/// when the seed is out of bounds or already equals `color`.
pub fn fill_region(buffer: &mut PixelBuffer, x: i32, y: i32, color: Color) -> Option<Rect2i> {
    fill_region_clipped(buffer, x, y, color, None)
}

fn fill_region_clipped(
    buffer: &mut PixelBuffer,
    x: i32,
    y: i32,
    color: Color,
    clip: Option<&PixelClip>,
) -> Option<Rect2i> {
    let w = buffer.width() as i32;
    let h = buffer.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h || !pixel_allowed(clip, x, y) {
        return None;
    }
    let seed = buffer.get_pixel(x as usize, y as usize)?;
    if seed == color {
        return None;
    }

    // Iterative scanline fill.  The buffer itself is the visited marker: once
    // a pixel is written with `color` (which != seed), it can never match the
    // seed again, so it is never re-visited.  No recursion, so arbitrarily
    // large canvases cannot overflow the stack.  O(n) in the region size.
    let mut stack = vec![(x, y)];
    let mut min_x = x;
    let mut min_y = y;
    let mut max_x = x;
    let mut max_y = y;
    while let Some((sx, sy)) = stack.pop() {
        // Walk left to the first matching pixel of this row's span.
        let mut cx = sx;
        while cx > 0
            && pixel_allowed(clip, cx - 1, sy)
            && buffer.get_pixel((cx - 1) as usize, sy as usize) == Some(seed)
        {
            cx -= 1;
        }
        let span_start = cx;
        // Walk right, filling every matching pixel of the span.
        while cx < w
            && pixel_allowed(clip, cx, sy)
            && buffer.get_pixel(cx as usize, sy as usize) == Some(seed)
        {
            buffer.set_pixel(cx as usize, sy as usize, color);
            min_x = min_x.min(cx);
            max_x = max_x.max(cx);
            cx += 1;
        }
        let span_end = cx;
        // Scan the rows above and below for spans overlapping this one.
        for ny in [sy - 1, sy + 1] {
            if ny < 0 || ny >= h {
                continue;
            }
            let mut nx = span_start;
            while nx < span_end {
                if pixel_allowed(clip, nx, ny)
                    && buffer.get_pixel(nx as usize, ny as usize) == Some(seed)
                {
                    stack.push((nx, ny));
                    min_y = min_y.min(ny);
                    max_y = max_y.max(ny);
                    // Skip the rest of this run in the adjacent row.
                    while nx < span_end
                        && pixel_allowed(clip, nx, ny)
                        && buffer.get_pixel(nx as usize, ny as usize) == Some(seed)
                    {
                        nx += 1;
                    }
                } else {
                    nx += 1;
                }
            }
        }
    }
    Some(Rect2i::new(
        min_x,
        min_y,
        max_x - min_x + 1,
        max_y - min_y + 1,
    ))
}

/// Replace mode (D48): every pixel in the canvas whose color exactly equals
/// the seed pixel's color is replaced with `color`, including pixels in
/// regions disconnected from the seed.
///
/// Returns the minimal bounding rect of the changed pixels, or `None` when the
/// seed is out of bounds or already equals `color`.
pub fn fill_replace_all(buffer: &mut PixelBuffer, x: i32, y: i32, color: Color) -> Option<Rect2i> {
    fill_replace_all_clipped(buffer, x, y, color, None)
}

fn fill_replace_all_clipped(
    buffer: &mut PixelBuffer,
    x: i32,
    y: i32,
    color: Color,
    clip: Option<&PixelClip>,
) -> Option<Rect2i> {
    let w = buffer.width() as i32;
    let h = buffer.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h || !pixel_allowed(clip, x, y) {
        return None;
    }
    let seed = buffer.get_pixel(x as usize, y as usize)?;
    if seed == color {
        return None;
    }
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = -1;
    let mut max_y = -1;
    for py in 0..h {
        for px in 0..w {
            if pixel_allowed(clip, px, py)
                && buffer.get_pixel(px as usize, py as usize) == Some(seed)
            {
                buffer.set_pixel(px as usize, py as usize, color);
                min_x = min_x.min(px);
                min_y = min_y.min(py);
                max_x = max_x.max(px);
                max_y = max_y.max(py);
            }
        }
    }
    // The seed itself always matches, so at least one pixel was changed.
    Some(Rect2i::new(
        min_x,
        min_y,
        max_x - min_x + 1,
        max_y - min_y + 1,
    ))
}

/// Apply a fill (contiguous or replace-all per `replace_all`) and record it as
/// a single undo step (D48: one undo step per fill gesture).
///
/// The changed-region bbox is computed up front (dry run, no mutation) so the
/// [`DeltaRecorder`] can snapshot the exact region before the mutation.  The
/// resulting command is bound to `layer` — the layer the fill painted on.
/// Returns `None` when nothing changes (out-of-bounds seed or seed color
/// already equals `color`).
pub fn fill_command(
    layer: LayerId,
    buffer: &mut PixelBuffer,
    x: i32,
    y: i32,
    color: Color,
    replace_all: bool,
) -> Option<ReverseDeltaCommand> {
    fill_command_clipped(layer, buffer, x, y, color, replace_all, None)
}

/// Applies a clipped fill and records it as a single undo step.
pub fn fill_command_clipped(
    layer: LayerId,
    buffer: &mut PixelBuffer,
    x: i32,
    y: i32,
    color: Color,
    replace_all: bool,
    clip: Option<&PixelClip>,
) -> Option<ReverseDeltaCommand> {
    let bbox = if replace_all {
        replace_all_bbox(buffer, x, y, color, clip)?
    } else {
        contiguous_bbox(buffer, x, y, color, clip)?
    };
    let recorder = DeltaRecorder::begin(FILL_UNDO_NAME, layer, buffer, bbox)?;
    let applied = if replace_all {
        fill_replace_all_clipped(buffer, x, y, color, clip)
    } else {
        fill_region_clipped(buffer, x, y, color, clip)
    };
    debug_assert_eq!(applied, Some(bbox));
    Some(recorder.finish(buffer))
}

/// Dry-run bbox of a contiguous fill: the minimal rect covering every pixel
/// the fill would change, without mutating the buffer.
fn contiguous_bbox(
    buffer: &PixelBuffer,
    x: i32,
    y: i32,
    color: Color,
    clip: Option<&PixelClip>,
) -> Option<Rect2i> {
    let w = buffer.width() as i32;
    let h = buffer.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h || !pixel_allowed(clip, x, y) {
        return None;
    }
    let seed = buffer.get_pixel(x as usize, y as usize)?;
    if seed == color {
        return None;
    }
    // Iterative 4-neighbor BFS with an explicit visited set (no mutation).
    let mut visited = vec![false; (w as usize) * (h as usize)];
    let mut stack = vec![(x, y)];
    visited[(y as usize) * (w as usize) + (x as usize)] = true;
    let mut min_x = x;
    let mut min_y = y;
    let mut max_x = x;
    let mut max_y = y;
    while let Some((cx, cy)) = stack.pop() {
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= w || ny >= h || !pixel_allowed(clip, nx, ny) {
                continue;
            }
            let idx = (ny as usize) * (w as usize) + (nx as usize);
            if visited[idx] {
                continue;
            }
            if buffer.get_pixel(nx as usize, ny as usize) != Some(seed) {
                continue;
            }
            visited[idx] = true;
            stack.push((nx, ny));
            min_x = min_x.min(nx);
            min_y = min_y.min(ny);
            max_x = max_x.max(nx);
            max_y = max_y.max(ny);
        }
    }
    Some(Rect2i::new(
        min_x,
        min_y,
        max_x - min_x + 1,
        max_y - min_y + 1,
    ))
}

/// Dry-run bbox of a replace-all fill: the minimal rect covering every pixel
/// the fill would change, without mutating the buffer.
fn replace_all_bbox(
    buffer: &PixelBuffer,
    x: i32,
    y: i32,
    color: Color,
    clip: Option<&PixelClip>,
) -> Option<Rect2i> {
    let w = buffer.width() as i32;
    let h = buffer.height() as i32;
    if x < 0 || y < 0 || x >= w || y >= h || !pixel_allowed(clip, x, y) {
        return None;
    }
    let seed = buffer.get_pixel(x as usize, y as usize)?;
    if seed == color {
        return None;
    }
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = -1;
    let mut max_y = -1;
    for py in 0..h {
        for px in 0..w {
            if pixel_allowed(clip, px, py)
                && buffer.get_pixel(px as usize, py as usize) == Some(seed)
            {
                min_x = min_x.min(px);
                min_y = min_y.min(py);
                max_x = max_x.max(px);
                max_y = max_y.max(py);
            }
        }
    }
    Some(Rect2i::new(
        min_x,
        min_y,
        max_x - min_x + 1,
        max_y - min_y + 1,
    ))
}

fn pixel_allowed(clip: Option<&PixelClip>, x: i32, y: i32) -> bool {
    match clip {
        None => true,
        Some(clip) => clip.contains(x, y),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::clip::PixelClip;
    use crate::core::model::LayerStack;
    use crate::core::select::Selection;
    use crate::core::undo::{Command, CommandContext, UndoStack};

    const WALL: Color = Color::rgb(1, 1, 1);
    const REGION: Color = Color::rgb(2, 2, 2);
    const NEW: Color = Color::rgb(3, 3, 3);

    /// Build a `CommandContext` borrowing the given layer stack.
    fn ctx(layers: &mut LayerStack) -> CommandContext<'_> {
        CommandContext { layers }
    }

    /// 5x5 buffer: a 3x3 `REGION` block at (1,1)..(4,4) surrounded by `WALL`.
    fn walled_region_buffer() -> PixelBuffer {
        let mut buf = PixelBuffer::new(5, 5);
        buf.fill(WALL);
        for y in 1..4 {
            for x in 1..4 {
                buf.set_pixel(x, y, REGION);
            }
        }
        buf
    }

    #[test]
    fn contiguous_fill_does_not_cross_an_unselected_mask_gap() {
        let mut layers = LayerStack::new(5, 1);
        let layer = layers.active_layer_id();
        let buffer = &mut layers.active_layer_mut().buffer;
        buffer.fill(REGION);
        let selection = Selection::capture_mask(
            buffer,
            Rect2i::new(0, 0, 5, 1),
            vec![true, false, true, true, true],
        )
        .unwrap();
        let clip = PixelClip::from_selection(&selection);

        let command = fill_command_clipped(layer, buffer, 0, 0, NEW, false, Some(&clip)).unwrap();

        assert_eq!(command.name(), "Fill");
        assert_eq!(buffer.get_pixel(0, 0), Some(NEW));
        for x in 1..5 {
            assert_eq!(buffer.get_pixel(x, 0), Some(REGION));
        }
    }

    #[test]
    fn clipped_fill_seed_outside_the_clip_is_a_noop() {
        let mut layers = LayerStack::new(5, 1);
        let layer = layers.active_layer_id();
        let buffer = &mut layers.active_layer_mut().buffer;
        buffer.fill(REGION);
        let selection = Selection::capture_mask(
            buffer,
            Rect2i::new(0, 0, 5, 1),
            vec![false, true, true, true, true],
        )
        .unwrap();
        let clip = PixelClip::from_selection(&selection);
        let before = buffer.as_bytes().to_vec();
        let epoch = buffer.change_epoch();

        let command = fill_command_clipped(layer, buffer, 0, 0, NEW, false, Some(&clip));

        assert!(command.is_none());
        assert_eq!(buffer.as_bytes(), before.as_slice());
        assert_eq!(buffer.change_epoch(), epoch);
    }

    #[test]
    fn replace_all_under_a_clip_replaces_only_selected_pixels() {
        let mut layers = LayerStack::new(7, 1);
        let layer = layers.active_layer_id();
        let buffer = &mut layers.active_layer_mut().buffer;
        buffer.fill(REGION);
        let selection =
            Selection::capture_mask(buffer, Rect2i::new(0, 0, 3, 1), vec![true, false, true])
                .unwrap();
        let clip = PixelClip::from_selection(&selection);

        let command = fill_command_clipped(layer, buffer, 0, 0, NEW, true, Some(&clip)).unwrap();

        assert_eq!(command.name(), "Fill");
        assert_eq!(buffer.get_pixel(0, 0), Some(NEW));
        assert_eq!(buffer.get_pixel(1, 0), Some(REGION));
        assert_eq!(buffer.get_pixel(2, 0), Some(NEW));
        for x in 3..7 {
            assert_eq!(buffer.get_pixel(x, 0), Some(REGION));
        }
    }

    #[test]
    fn clipped_fill_undo_restores_pixels_and_keeps_a_tight_delta() {
        let mut layers = LayerStack::new(5, 1);
        let layer = layers.active_layer_id();
        let (mut command, before) = {
            let buffer = &mut layers.active_layer_mut().buffer;
            buffer.fill(REGION);
            let selection = Selection::capture_mask(
                buffer,
                Rect2i::new(0, 0, 5, 1),
                vec![true, false, false, false, false],
            )
            .unwrap();
            let clip = PixelClip::from_selection(&selection);
            let before = buffer.as_bytes().to_vec();
            let command =
                fill_command_clipped(layer, buffer, 0, 0, NEW, false, Some(&clip)).unwrap();
            assert_eq!(buffer.get_pixel(0, 0), Some(NEW));
            (command, before)
        };
        {
            let buffer = &mut layers.active_layer_mut().buffer;
            buffer.set_pixel(1, 0, Color::WHITE);
        }
        assert!(command.undo(&mut ctx(&mut layers)));
        let buffer = &layers.active_layer().buffer;
        assert_eq!(buffer.get_pixel(0, 0), Some(REGION));
        assert_eq!(buffer.get_pixel(1, 0), Some(Color::WHITE));
        assert_eq!(buffer.get_pixel(2, 0), Some(REGION));
        assert_eq!(buffer.get_pixel(3, 0), Some(REGION));
        assert_eq!(buffer.get_pixel(4, 0), Some(REGION));
        assert_eq!(&buffer.as_bytes()[8..12], &before[8..12]);
    }

    #[test]
    fn fill_region_replaces_exact_block_and_leaves_walls() {
        let mut buf = walled_region_buffer();
        let bbox = fill_region(&mut buf, 1, 1, NEW).unwrap();
        assert_eq!(bbox, Rect2i::new(1, 1, 3, 3));
        for y in 1..4 {
            for x in 1..4 {
                assert_eq!(buf.get_pixel(x, y), Some(NEW));
            }
        }
        // Walls untouched.
        for y in 0..5 {
            for x in 0..5 {
                if !(1..4).contains(&x) || !(1..4).contains(&y) {
                    assert_eq!(buf.get_pixel(x, y), Some(WALL));
                }
            }
        }
    }

    #[test]
    fn fill_region_does_not_fill_diagonally() {
        // 3x3: seed at (1,1); a matching pixel exists only diagonally at (0,0).
        let mut buf = PixelBuffer::new(3, 3);
        buf.fill(WALL);
        buf.set_pixel(1, 1, REGION);
        buf.set_pixel(0, 0, REGION);
        let bbox = fill_region(&mut buf, 1, 1, NEW).unwrap();
        assert_eq!(bbox, Rect2i::new(1, 1, 1, 1));
        assert_eq!(buf.get_pixel(1, 1), Some(NEW));
        assert_eq!(buf.get_pixel(0, 0), Some(REGION)); // diagonal only → untouched
    }

    #[test]
    fn fill_region_tolerance_zero_exact_byte_match() {
        let mut buf = PixelBuffer::new(3, 3);
        buf.fill(WALL);
        buf.set_pixel(1, 1, Color::rgb(10, 10, 10));
        // 4-connected neighbor differing by 1 in one channel.
        buf.set_pixel(0, 1, Color::rgb(11, 10, 10));
        let bbox = fill_region(&mut buf, 1, 1, NEW).unwrap();
        assert_eq!(bbox, Rect2i::new(1, 1, 1, 1));
        assert_eq!(buf.get_pixel(1, 1), Some(NEW));
        assert_eq!(buf.get_pixel(0, 1), Some(Color::rgb(11, 10, 10)));
    }

    #[test]
    fn fill_region_seed_already_target_returns_none() {
        let mut buf = walled_region_buffer();
        let epoch = buf.change_epoch();
        assert_eq!(fill_region(&mut buf, 1, 1, REGION), None);
        assert_eq!(buf.change_epoch(), epoch);
        assert_eq!(buf.get_pixel(1, 1), Some(REGION));
    }

    #[test]
    fn fill_region_out_of_bounds_seed_returns_none() {
        let mut buf = walled_region_buffer();
        let epoch = buf.change_epoch();
        for (x, y) in [(-1, 0), (0, -1), (5, 0), (0, 5), (100, 100), (-100, -100)] {
            assert_eq!(fill_region(&mut buf, x, y, NEW), None);
        }
        assert_eq!(buf.change_epoch(), epoch);
    }

    #[test]
    fn fill_region_bbox_is_minimal_for_irregular_shape() {
        // L-shape: (2,2),(3,2),(4,2),(2,3),(2,4) on a 7x7 walled canvas.
        let mut buf = PixelBuffer::new(7, 7);
        buf.fill(WALL);
        for (x, y) in [(2, 2), (3, 2), (4, 2), (2, 3), (2, 4)] {
            buf.set_pixel(x, y, REGION);
        }
        let bbox = fill_region(&mut buf, 3, 2, NEW).unwrap();
        assert_eq!(bbox, Rect2i::new(2, 2, 3, 3));
        for (x, y) in [(2, 2), (3, 2), (4, 2), (2, 3), (2, 4)] {
            assert_eq!(buf.get_pixel(x, y), Some(NEW));
        }
        // Bbox corners that are NOT part of the shape stay wall.
        for (x, y) in [(3, 3), (4, 3), (3, 4), (4, 4)] {
            assert_eq!(buf.get_pixel(x, y), Some(WALL));
        }
    }

    #[test]
    fn fill_replace_all_reaches_disconnected_regions() {
        // Two REGION blobs separated by a WALL gap.
        let mut buf = PixelBuffer::new(7, 3);
        buf.fill(WALL);
        buf.set_pixel(1, 1, REGION);
        buf.set_pixel(5, 1, REGION);
        let bbox = fill_replace_all(&mut buf, 1, 1, NEW).unwrap();
        assert_eq!(bbox, Rect2i::new(1, 1, 5, 1));
        assert_eq!(buf.get_pixel(1, 1), Some(NEW));
        assert_eq!(buf.get_pixel(5, 1), Some(NEW));
        // Gap and surroundings untouched.
        for x in 2..5 {
            assert_eq!(buf.get_pixel(x, 1), Some(WALL));
        }
        assert_eq!(buf.get_pixel(0, 1), Some(WALL));
        assert_eq!(buf.get_pixel(6, 1), Some(WALL));
    }

    #[test]
    fn fill_replace_all_seed_already_target_returns_none() {
        let mut buf = walled_region_buffer();
        let epoch = buf.change_epoch();
        assert_eq!(fill_replace_all(&mut buf, 1, 1, REGION), None);
        assert_eq!(buf.change_epoch(), epoch);
        assert_eq!(buf.get_pixel(1, 1), Some(REGION));
    }

    #[test]
    fn fill_command_records_single_undo_step() {
        let mut layers = LayerStack::new(5, 5);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.fill(WALL);
            for y in 1..4 {
                for x in 1..4 {
                    buf.set_pixel(x, y, REGION);
                }
            }
        }
        let original = layers
            .active_layer()
            .buffer
            .export_region(Rect2i::new(0, 0, 5, 5), None)
            .unwrap();

        let cmd =
            fill_command(lid, &mut layers.active_layer_mut().buffer, 1, 1, NEW, false).unwrap();
        assert_eq!(cmd.name(), "Fill");
        assert_eq!(layers.active_layer().buffer.get_pixel(1, 1), Some(NEW));

        let mut stack = UndoStack::new();
        stack.push(Box::new(cmd));
        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers
                .active_layer()
                .buffer
                .export_region(Rect2i::new(0, 0, 5, 5), None)
                .unwrap(),
            original
        );
        assert!(stack.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.get_pixel(1, 1), Some(NEW));

        // Nothing changed → None (seed color now equals the target).
        assert!(
            fill_command(lid, &mut layers.active_layer_mut().buffer, 1, 1, NEW, false).is_none()
        );
        // Out-of-bounds seed → None.
        assert!(fill_command(
            lid,
            &mut layers.active_layer_mut().buffer,
            -1,
            0,
            NEW,
            false
        )
        .is_none());
    }

    #[test]
    fn fill_command_replace_all_records_single_undo_step() {
        let mut layers = LayerStack::new(7, 3);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.fill(WALL);
            buf.set_pixel(1, 1, REGION);
            buf.set_pixel(5, 1, REGION);
        }
        let original = layers
            .active_layer()
            .buffer
            .export_region(Rect2i::new(0, 0, 7, 3), None)
            .unwrap();

        let cmd =
            fill_command(lid, &mut layers.active_layer_mut().buffer, 1, 1, NEW, true).unwrap();
        assert_eq!(cmd.name(), "Fill");
        assert_eq!(layers.active_layer().buffer.get_pixel(5, 1), Some(NEW));

        let mut stack = UndoStack::new();
        stack.push(Box::new(cmd));
        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers
                .active_layer()
                .buffer
                .export_region(Rect2i::new(0, 0, 7, 3), None)
                .unwrap(),
            original
        );
        assert!(stack.redo(&mut ctx(&mut layers)));
        assert_eq!(layers.active_layer().buffer.get_pixel(1, 1), Some(NEW));
        assert_eq!(layers.active_layer().buffer.get_pixel(5, 1), Some(NEW));
    }

    #[test]
    fn fill_region_canvas_edge_and_full_canvas() {
        // Seed at the canvas corner (0,0); the whole canvas is one region.
        let mut buf = PixelBuffer::new(4, 4);
        buf.fill(REGION);
        let bbox = fill_region(&mut buf, 0, 0, NEW).unwrap();
        assert_eq!(bbox, Rect2i::new(0, 0, 4, 4));
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(buf.get_pixel(x, y), Some(NEW));
            }
        }
    }

    #[test]
    /// Given a contiguous fill crosses a mask gap, when it runs from a selected seed, then it stops at the gap.
    fn fill_command_clipped_contiguous_stops_at_the_mask_boundary() {
        let mut layers = LayerStack::new(5, 1);
        let layer = layers.active_layer_id();
        let buffer = &mut layers.active_layer_mut().buffer;
        buffer.fill(REGION);
        let selection = Selection::capture_mask(
            buffer,
            Rect2i::new(0, 0, 5, 1),
            vec![true, false, true, true, true],
        )
        .unwrap();
        let clip = PixelClip::from_selection(&selection);

        let command = fill_command_clipped(layer, buffer, 0, 0, NEW, false, Some(&clip)).unwrap();

        assert_eq!(command.name(), "Fill");
        assert_eq!(buffer.get_pixel(0, 0), Some(NEW));
        for x in 1..5 {
            assert_eq!(buffer.get_pixel(x, 0), Some(REGION));
        }
    }

    #[test]
    /// Given a replace-all fill under a mask, when it runs, then every selected pixel of the seed color changes.
    fn fill_command_clipped_replace_all_respects_the_mask() {
        let mut layers = LayerStack::new(7, 1);
        let layer = layers.active_layer_id();
        let buffer = &mut layers.active_layer_mut().buffer;
        buffer.fill(REGION);
        let selection =
            Selection::capture_mask(buffer, Rect2i::new(0, 0, 3, 1), vec![true, false, true])
                .unwrap();
        let clip = PixelClip::from_selection(&selection);

        let command = fill_command_clipped(layer, buffer, 0, 0, NEW, true, Some(&clip)).unwrap();

        assert_eq!(command.name(), "Fill");
        assert_eq!(buffer.get_pixel(0, 0), Some(NEW));
        assert_eq!(buffer.get_pixel(1, 0), Some(REGION));
        assert_eq!(buffer.get_pixel(2, 0), Some(NEW));
        for x in 3..7 {
            assert_eq!(buffer.get_pixel(x, 0), Some(REGION));
        }
    }

    #[test]
    /// Given the fill seed is outside the mask, when the fill is requested, then it is a no-op.
    fn fill_command_clipped_seed_outside_the_mask_is_a_noop() {
        let mut layers = LayerStack::new(5, 1);
        let layer = layers.active_layer_id();
        let buffer = &mut layers.active_layer_mut().buffer;
        buffer.fill(REGION);
        let selection = Selection::capture_mask(
            buffer,
            Rect2i::new(0, 0, 5, 1),
            vec![false, true, true, true, true],
        )
        .unwrap();
        let clip = PixelClip::from_selection(&selection);
        let before = buffer.as_bytes().to_vec();
        let epoch = buffer.change_epoch();

        let command = fill_command_clipped(layer, buffer, 0, 0, NEW, false, Some(&clip));

        assert!(command.is_none());
        assert_eq!(buffer.as_bytes(), before.as_slice());
        assert_eq!(buffer.change_epoch(), epoch);
    }

    #[test]
    /// Given a clipped fill command, when it is undone, then the pre-fill pixels are restored.
    fn fill_command_clipped_undo_restores_the_pre_fill_pixels() {
        let mut layers = LayerStack::new(5, 1);
        let layer = layers.active_layer_id();
        let (mut command, before) = {
            let buffer = &mut layers.active_layer_mut().buffer;
            buffer.fill(REGION);
            let selection = Selection::capture_mask(
                buffer,
                Rect2i::new(0, 0, 5, 1),
                vec![true, false, true, true, true],
            )
            .unwrap();
            let clip = PixelClip::from_selection(&selection);
            let before = buffer.as_bytes().to_vec();
            let command =
                fill_command_clipped(layer, buffer, 0, 0, NEW, false, Some(&clip)).unwrap();
            assert_eq!(buffer.get_pixel(0, 0), Some(NEW));
            assert_eq!(buffer.get_pixel(1, 0), Some(REGION));
            (command, before)
        };

        assert!(command.undo(&mut ctx(&mut layers)));
        let buffer = &layers.active_layer().buffer;
        assert_eq!(buffer.as_bytes(), before.as_slice());
    }
}
