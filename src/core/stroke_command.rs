//! Stroke ↔ Undo glue — converts brush strokes into reversible undo commands.
//!
//! This module bridges [`brush`] and [`undo`] without polluting either with
//! cross-dependencies.  The intended call-site pattern:
//!
//! ```text
//! // 1. Capture before-state (once, before the first stamp)
//! let record = StrokeRecord::capture_region(&buf, canvas_rect)?;
//!
//! // 2. Run the stroke (start / continue_to / …)
//! stroke.start(&mut buf, x, y);
//! // …
//!
//! // 3. Build the undo command, bound to the layer the stroke painted on
//! let cmd = stroke_to_undo_command(&record, layer_id, &buf);
//!
//! // 4. Push it onto the undo stack
//! stack.push(Box::new(cmd));
//! ```

use crate::core::brush::{Stroke, StrokeRecord};
use crate::core::buffer::PixelBuffer;
use crate::core::math::Rect2i;
use crate::core::model::LayerId;
use crate::core::undo::ReverseDeltaCommand;

/// The static label stamped on every stroke-derived undo command.
const STROKE_UNDO_NAME: &str = "Stroke";

/// Turn a [`StrokeRecord`] (captured before the stroke touched the buffer)
/// into a [`ReverseDeltaCommand`] that knows both the before and after state.
///
/// The command is bound to `layer` — the layer the stroke painted on — so
/// undo/redo replay onto that layer's buffer regardless of which layer is
/// active at undo time.
///
/// Returns `None` when `record.region` is no longer valid for `buf` (e.g.
/// the buffer was resized between capture and finish — shouldn't happen in
/// normal usage, but we stay defensive).
pub fn stroke_to_undo_command(
    record: &StrokeRecord,
    layer: LayerId,
    buf: &PixelBuffer,
) -> Option<ReverseDeltaCommand> {
    buf.export_region(record.region, None)?;
    Some(ReverseDeltaCommand::from_recorder(
        STROKE_UNDO_NAME,
        layer,
        record.region,
        &record.before,
        buf,
    ))
}

/// High-level session that wraps capture → stroke → command in one RAII type.
///
/// Usage:
/// ```text
/// let mut session = StrokeSession::begin(buf, stroke, bounds, layer_id);
/// session.stroke_mut().continue_to(buf, x1, y1);
/// // …
/// if let Some(cmd) = session.finish(buf) {
///     stack.push(Box::new(cmd));
/// }
/// ```
pub struct StrokeSession {
    stroke: Stroke,
    record: StrokeRecord,
    layer: LayerId,
}

impl StrokeSession {
    /// Begin a new stroke session.
    ///
    /// `capture_region` is the rectangular area whose before-state should be
    /// recorded for undo.  Typically the full canvas rect or the visible
    /// viewport — callers may supply a generous estimate; the undo command
    /// will store the exact bytes regardless.
    ///
    /// `layer` is the [`LayerId`] the stroke will paint on; the resulting
    /// undo command is bound to it.
    pub fn begin(
        buf: &PixelBuffer,
        stroke: Stroke,
        capture_region: Rect2i,
        layer: LayerId,
    ) -> Option<Self> {
        let record = StrokeRecord::capture_region(buf, capture_region)?;
        Some(Self {
            stroke,
            record,
            layer,
        })
    }

    /// Borrow the inner [`Stroke`] for `start` / `continue_to` calls.
    pub fn stroke_mut(&mut self) -> &mut Stroke {
        &mut self.stroke
    }

    /// Borrow the inner [`Stroke`] immutably.
    pub fn stroke(&self) -> &Stroke {
        &self.stroke
    }

    /// The [`LayerId`] this session's stroke paints on.
    pub fn layer_id(&self) -> LayerId {
        self.layer
    }

    /// The bounding box of the stroke so far (in canvas coordinates).
    pub fn bounding_box(&self) -> Rect2i {
        self.stroke.bounding_box()
    }

    /// The before-state region captured at the beginning.
    pub fn capture_region(&self) -> Rect2i {
        self.record.region
    }

    /// Returns the tight RGBA8 before/current delta for changed stroke pixels.
    ///
    /// The stroke's stamp bbox conservatively includes the tapered brush
    /// footprint and scatter radius; intersecting it with the captured region
    /// bounds work to the region the session recorded. Only that candidate
    /// rectangle is exported, never the full canvas.
    pub fn preview_delta(&self, buf: &PixelBuffer) -> Option<(Rect2i, Vec<u8>, Vec<u8>)> {
        let capture = self.record.region;
        if capture.is_empty()
            || capture.x < 0
            || capture.y < 0
            || capture.right() > buf.width().min(i32::MAX as usize) as i32
            || capture.bottom() > buf.height().min(i32::MAX as usize) as i32
        {
            return None;
        }
        let capture_len = (capture.w as usize)
            .checked_mul(capture.h as usize)?
            .checked_mul(4)?;
        if self.record.before.len() != capture_len {
            return None;
        }
        let candidate = self.stroke.bounding_box().intersection(capture);
        if candidate.is_empty() {
            return None;
        }
        let current = buf.export_region(candidate, None)?;
        let capture_stride = capture.w as usize * 4;
        let candidate_stride = candidate.w as usize * 4;
        let mut changed_bounds: Option<(i32, i32, i32, i32)> = None;
        for y in 0..candidate.h as usize {
            for x in 0..candidate.w as usize {
                let capture_offset = ((candidate.y - capture.y) as usize + y) * capture_stride
                    + (candidate.x - capture.x) as usize * 4
                    + x * 4;
                let current_offset = y * candidate_stride + x * 4;
                if self.record.before[capture_offset..capture_offset + 4]
                    != current[current_offset..current_offset + 4]
                {
                    let px = candidate.x + x as i32;
                    let py = candidate.y + y as i32;
                    changed_bounds = Some(match changed_bounds {
                        Some((x0, y0, x1, y1)) => {
                            (x0.min(px), y0.min(py), x1.max(px + 1), y1.max(py + 1))
                        }
                        None => (px, py, px + 1, py + 1),
                    });
                }
            }
        }
        let (x0, y0, x1, y1) = changed_bounds?;
        let region = Rect2i::new(x0, y0, x1 - x0, y1 - y0);
        let mut before = Vec::with_capacity(region.area() as usize * 4);
        let mut after = Vec::with_capacity(region.area() as usize * 4);
        for y in y0..y1 {
            let source_y = (y - candidate.y) as usize;
            let source_x = (x0 - candidate.x) as usize;
            let candidate_offset = source_y * candidate_stride + source_x * 4;
            let capture_offset = (y - capture.y) as usize * capture_stride
                + (x0 - capture.x) as usize * 4;
            let row_bytes = region.w as usize * 4;
            before.extend_from_slice(
                &self.record.before[capture_offset..capture_offset + row_bytes],
            );
            after.extend_from_slice(&current[candidate_offset..candidate_offset + row_bytes]);
        }
        if before.len() != region.area() as usize * 4 || after.len() != before.len() {
            return None;
        }
        Some((region, before, after))
    }

    /// Finalize the stroke and produce an undo command.
    ///
    /// The command stores the full before/after delta for the captured region,
    /// bound to the session's layer, ready to push onto an
    /// [`UndoStack`](crate::core::undo::UndoStack).
    pub fn finish(self, buf: &PixelBuffer) -> Option<ReverseDeltaCommand> {
        stroke_to_undo_command(&self.record, self.layer, buf)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::brush::{BrushShape, BrushSpec, DrawMode};
    use crate::core::color::Color;
    use crate::core::model::LayerStack;
    use crate::core::tilemap::TilePalette;
    use crate::core::undo::{Command, CommandContext};

    /// Build a `CommandContext` borrowing the given layer stack.
    fn ctx(layers: &mut LayerStack) -> CommandContext<'_> {
        // Test-only: the palette is leaked so the returned context outlives the
        // `&mut ctx(...)` temporary (no tile-edit command flows through these
        // tests).
        let palette = Box::leak(Box::new(TilePalette::new()));
        CommandContext {
            layers,
            palette: &mut *palette,
        }
    }

    #[test]
    fn stroke_to_undo_command_round_trips() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.fill(Color::rgb(30, 60, 90));
            buf.set_pixel(4, 4, Color::rgb(1, 1, 1));
        }

        // Capture before-state for the whole canvas.
        let region = Rect2i::new(0, 0, 8, 8);
        let record = StrokeRecord::capture_region(&layers.active_layer().buffer, region).unwrap();
        let before_bytes = record.before.clone();

        // Mutate: draw a 3px pencil line.
        let spec = BrushSpec::new(1, BrushShape::Square);
        let mut stroke = Stroke::new(spec, DrawMode::Pen, Color::WHITE);
        {
            let buf = &mut layers.active_layer_mut().buffer;
            stroke.start(buf, 2, 2);
            stroke.continue_to(buf, 5, 2);
        }

        // Build undo command.
        let mut cmd = stroke_to_undo_command(&record, lid, &layers.active_layer().buffer).unwrap();
        assert_eq!(cmd.name(), "Stroke");

        // Undo must restore the pre-stroke bytes exactly.
        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers
                .active_layer()
                .buffer
                .export_region(region, None)
                .unwrap(),
            before_bytes
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(4, 4),
            Some(Color::rgb(1, 1, 1))
        );

        // Redo must re-apply the post-stroke bytes.
        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 2),
            Some(Color::WHITE)
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(5, 2),
            Some(Color::WHITE)
        );
    }

    #[test]
    fn stroke_to_undo_command_returns_none_for_zero_region() {
        let layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let record = StrokeRecord {
            region: Rect2i::new(0, 0, 0, 0),
            before: vec![],
        };
        assert!(stroke_to_undo_command(&record, lid, &layers.active_layer().buffer).is_none());
    }

    #[test]
    fn stroke_to_undo_command_returns_none_for_negative_region() {
        let layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let record = StrokeRecord {
            region: Rect2i::new(-2, -2, 4, 4),
            before: vec![],
        };
        assert!(stroke_to_undo_command(&record, lid, &layers.active_layer().buffer).is_none());
    }

    #[test]
    fn stroke_session_happy_path() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.fill(Color::rgb(30, 60, 90));
        }
        let original = layers
            .active_layer()
            .buffer
            .export_region(Rect2i::new(0, 0, 8, 8), None)
            .unwrap();
        let spec = BrushSpec::new(3, BrushShape::Round);
        let stroke = Stroke::new(spec, DrawMode::Pen, Color::WHITE);
        let canvas = Rect2i::new(0, 0, 8, 8);

        let mut session =
            StrokeSession::begin(&layers.active_layer().buffer, stroke, canvas, lid).unwrap();
        assert_eq!(session.capture_region(), canvas);
        assert_eq!(session.layer_id(), lid);

        {
            let buf = &mut layers.active_layer_mut().buffer;
            session.stroke_mut().start(buf, 3, 3);
            session.stroke_mut().continue_to(buf, 5, 3);
        }
        assert!(!session.bounding_box().is_empty());

        let mut cmd = session.finish(&layers.active_layer().buffer).unwrap();

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers
                .active_layer()
                .buffer
                .export_region(canvas, None)
                .unwrap(),
            original
        );
    }

    #[test]
    fn stroke_session_returns_none_when_region_out_of_bounds() {
        let layers = LayerStack::new(4, 4);
        let lid = layers.active_layer_id();
        let stroke = Stroke::new(BrushSpec::PENCIL_1PX, DrawMode::Pen, Color::WHITE);
        let oob_region = Rect2i::new(10, 10, 4, 4);
        assert!(
            StrokeSession::begin(&layers.active_layer().buffer, stroke, oob_region, lid).is_none()
        );
    }

    #[test]
    fn preview_delta_is_none_for_noop_and_tight_for_single_pixel() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        let canvas = Rect2i::new(0, 0, 8, 8);
        let stroke = Stroke::new(BrushSpec::PENCIL_1PX, DrawMode::Pen, Color::WHITE);
        let mut session =
            StrokeSession::begin(&layers.active_layer().buffer, stroke, canvas, lid).unwrap();
        assert!(session.preview_delta(&layers.active_layer().buffer).is_none());
        session
            .stroke_mut()
            .start(&mut layers.active_layer_mut().buffer, 4, 5);
        assert_eq!(
            session.preview_delta(&layers.active_layer().buffer),
            Some((Rect2i::new(4, 5, 1, 1), vec![0, 0, 0, 0], vec![255, 255, 255, 255]))
        );
    }

    #[test]
    fn preview_delta_tightly_covers_scatter_and_tail_on_large_canvas() {
        let mut layers = LayerStack::new(512, 512);
        let lid = layers.active_layer_id();
        let canvas = Rect2i::new(0, 0, 512, 512);
        let stroke = Stroke::new(
            BrushSpec::new(1, BrushShape::Square),
            DrawMode::Pen,
            Color::rgb(30, 90, 240),
        )
        .with_scatter(7)
        .with_jitter_seed(1234)
        .with_tail(2);
        let mut session =
            StrokeSession::begin(&layers.active_layer().buffer, stroke, canvas, lid).unwrap();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            session.stroke_mut().start(buf, 100, 100);
            session.stroke_mut().continue_to(buf, 130, 100);
        }

        let (region, before, after) = session
            .preview_delta(&layers.active_layer().buffer)
            .expect("scatter/tail stroke changed pixels");
        let mut expected_bounds: Option<(i32, i32, i32, i32)> = None;
        let current = layers.active_layer().buffer.as_bytes();
        for y in 0..512usize {
            for x in 0..512usize {
                let offset = (y * 512 + x) * 4;
                if session.record.before[offset..offset + 4] != current[offset..offset + 4] {
                    expected_bounds = Some(match expected_bounds {
                        Some((x0, y0, x1, y1)) => (
                            x0.min(x as i32),
                            y0.min(y as i32),
                            x1.max(x as i32 + 1),
                            y1.max(y as i32 + 1),
                        ),
                        None => (x as i32, y as i32, x as i32 + 1, y as i32 + 1),
                    });
                }
            }
        }
        let (x0, y0, x1, y1) = expected_bounds.unwrap();
        assert_eq!(region, Rect2i::new(x0, y0, x1 - x0, y1 - y0));
        assert_eq!(before.len(), region.area() as usize * 4);
        assert_eq!(after.len(), region.area() as usize * 4);
        assert!(region.area() < canvas.area() / 4, "preview must stay sparse");
        for y in 0..region.h as usize {
            for x in 0..region.w as usize {
                let output_offset = (y * region.w as usize + x) * 4;
                let canvas_offset = ((region.y as usize + y) * 512 + region.x as usize + x) * 4;
                assert_eq!(&before[output_offset..output_offset + 4], &session.record.before[canvas_offset..canvas_offset + 4]);
                assert_eq!(&after[output_offset..output_offset + 4], &current[canvas_offset..canvas_offset + 4]);
            }
        }
    }

    #[test]
    fn stroke_session_erase_undo_round_trip() {
        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.fill(Color::rgb(30, 60, 90));
        }
        let original = layers
            .active_layer()
            .buffer
            .export_region(Rect2i::new(0, 0, 8, 8), None)
            .unwrap();
        let spec = BrushSpec::new(2, BrushShape::Square);
        let stroke = Stroke::new(spec, DrawMode::Eraser, Color::BLACK);
        let canvas = Rect2i::new(0, 0, 8, 8);

        let mut session =
            StrokeSession::begin(&layers.active_layer().buffer, stroke, canvas, lid).unwrap();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            session.stroke_mut().start(buf, 2, 2);
        }

        let mut cmd = session.finish(&layers.active_layer().buffer).unwrap();

        assert!(cmd.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers
                .active_layer()
                .buffer
                .export_region(canvas, None)
                .unwrap(),
            original
        );

        assert!(cmd.redo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::TRANSPARENT)
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(2, 2),
            Some(Color::TRANSPARENT)
        );
    }

    #[test]
    fn undo_stack_integration() {
        use crate::core::undo::UndoStack;

        let mut layers = LayerStack::new(8, 8);
        let lid = layers.active_layer_id();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            buf.fill(Color::rgb(30, 60, 90));
        }
        let mut stack = UndoStack::new();

        // Stroke 1: white pencil
        let spec = BrushSpec::new(1, BrushShape::Square);
        let stroke = Stroke::new(spec, DrawMode::Pen, Color::WHITE);
        let canvas = Rect2i::new(0, 0, 8, 8);
        let mut s1 =
            StrokeSession::begin(&layers.active_layer().buffer, stroke, canvas, lid).unwrap();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            s1.stroke_mut().start(buf, 1, 1);
        }
        stack.push(Box::new(s1.finish(&layers.active_layer().buffer).unwrap()));

        // Stroke 2: red pencil
        let stroke2 = Stroke::new(spec, DrawMode::Pen, Color::rgb(255, 0, 0));
        let mut s2 =
            StrokeSession::begin(&layers.active_layer().buffer, stroke2, canvas, lid).unwrap();
        {
            let buf = &mut layers.active_layer_mut().buffer;
            s2.stroke_mut().start(buf, 5, 5);
        }
        stack.push(Box::new(s2.finish(&layers.active_layer().buffer).unwrap()));

        assert_eq!(stack.undo_len(), 2);
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::WHITE)
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(5, 5),
            Some(Color::rgb(255, 0, 0))
        );

        // Undo stroke 2
        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(5, 5),
            Some(Color::rgb(30, 60, 90))
        );
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::WHITE)
        );

        // Undo stroke 1
        assert!(stack.undo(&mut ctx(&mut layers)));
        assert_eq!(
            layers.active_layer().buffer.get_pixel(1, 1),
            Some(Color::rgb(30, 60, 90))
        );
    }
}
