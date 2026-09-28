//! Timeline + playback panel. U13 (R3 wave, feature F3).
//!
//! A pure view over the animation sequence and its playback controller:
//! renders one chip per frame (drag handle, delay editor, remove button), an
//! add-frame button, a loop toggle and the transport buttons. The panel never
//! mutates the sequence or the controller — every gesture is emitted as a
//! [`TimelineEvent`] for the App shell to apply (D59/D62: panels are pure
//! views; the App owns state and applies events as actions).

use crate::core::model::sequence::AnimationSequence;
use crate::ui::theme::ThemeColors;

/// A user gesture on the timeline panel, emitted by [`TimelinePanel::ui`].
///
/// The App shell owns the [`AnimationSequence`] and the playback controller
/// and applies these events; the panel itself is a pure view and never
/// mutates the inputs it is given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TimelineEvent {
    /// The user clicked a frame chip (the App decides whether to jump).
    SelectFrame(usize),
    /// The user changed a frame's delay (ms).
    SetDuration { index: usize, ms: u32 },
    /// The user dragged a chip onto another chip's slot.
    Reorder { from: usize, to: usize },
    /// The user pressed the "+" button.
    AddFrame,
    /// The user pressed a chip's remove button.
    RemoveFrame(usize),
    /// The user toggled the loop checkbox.
    ToggleLoop,
    /// The user pressed the play button.
    Play,
    /// The user pressed the stop button.
    Stop,
    /// The user pressed the restart button.
    Restart,
}

/// Vertical gap between the frame-chip row and the transport row.
const SECTION_SPACING: f32 = 4.0;

/// Timeline panel widget — a pure view over the animation sequence.
///
/// Holds no persistent state today; it is a struct (rather than a free
/// function) so drag-reorder state can be added later without changing the
/// API shape.
pub struct TimelinePanel {}

impl TimelinePanel {
    /// New panel.
    pub fn new() -> Self {
        Self {}
    }

    /// Render the panel for one frame.
    ///
    /// * `sequence` — a snapshot borrow of the frame list (never mutated
    ///   here).
    /// * `current` — the current frame index (from the controller).
    /// * `playing` — whether playback is running (enables the stop button).
    /// * `looping` — the loop flag (from the controller).
    /// * `events` — appended with every user gesture this frame.
    ///
    /// Renders one chip per frame in play order. The current chip is tinted
    /// with the selection background. Each chip is a drop target for reorder;
    /// only the "#N" label is the drag handle (so the delay editor and remove
    /// button stay clickable). The second row holds the loop toggle and the
    /// transport buttons.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        sequence: &AnimationSequence,
        current: usize,
        playing: bool,
        looping: bool,
        events: &mut Vec<TimelineEvent>,
    ) {
        // Row 1: one chip per frame, then the add button.
        ui.horizontal_wrapped(|ui| {
            for index in 0..sequence.len() {
                self.chip_ui(ui, theme, sequence, index, current, events);
            }
            if ui.button("+").on_hover_text("Add frame").clicked() {
                events.push(TimelineEvent::AddFrame);
            }
        });

        ui.add_space(SECTION_SPACING);

        // Row 2: loop toggle + transport.
        ui.horizontal(|ui| {
            let mut loop_on = looping;
            if ui.checkbox(&mut loop_on, "Loop").changed() {
                events.push(TimelineEvent::ToggleLoop);
            }
            ui.separator();
            if ui.button("⏮").on_hover_text("Restart").clicked() {
                events.push(TimelineEvent::Restart);
            }
            if ui.button("▶").on_hover_text("Play").clicked() {
                events.push(TimelineEvent::Play);
            }
            let stop = ui
                .add_enabled(playing, egui::Button::new("⏸"))
                .on_hover_text("Stop");
            if stop.clicked() {
                events.push(TimelineEvent::Stop);
            }
        });
    }

    /// One frame chip: drag handle, delay editor and remove button. The whole
    /// chip is the drop target for reorder; the "#N" label is the drag source.
    fn chip_ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        sequence: &AnimationSequence,
        index: usize,
        current: usize,
        events: &mut Vec<TimelineEvent>,
    ) {
        let is_current = index == current;
        let chip_bg = if is_current {
            theme.selection_bg_fill32()
        } else {
            egui::Color32::TRANSPARENT
        };
        let (_, dropped) = ui.dnd_drop_zone::<usize, _>(egui::Frame::NONE, |ui| {
            egui::Frame::NONE
                .fill(chip_bg)
                .inner_margin(egui::Margin::symmetric(4, 2))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Drag handle: the "#N" label. A plain click selects
                        // the frame; a drag reorders it. The button senses both
                        // click and drag so the hit-testing does not swallow the
                        // click (a drag-only overlay would).
                        let label = format!("#{}", index + 1);
                        let response = ui.add(
                            egui::Button::new(label)
                                .sense(egui::Sense::click_and_drag())
                                .selected(is_current),
                        );
                        if response.clicked() {
                            events.push(TimelineEvent::SelectFrame(index));
                        }
                        if response.drag_started() {
                            response.dnd_set_drag_payload(index);
                        }

                        // Delay editor (ms).
                        let mut delay = sequence.frame(index).map(|f| f.delay_ms()).unwrap_or(100);
                        let drag_value = ui.add(
                            egui::DragValue::new(&mut delay)
                                .range(0..=60_000)
                                .speed(1.0)
                                .suffix(" ms"),
                        );
                        if drag_value.changed() {
                            events.push(TimelineEvent::SetDuration { index, ms: delay });
                        }

                        // Remove button.
                        if ui.button("✖").on_hover_text("Remove frame").clicked() {
                            events.push(TimelineEvent::RemoveFrame(index));
                        }
                    });
                });
        });
        if let Some(from) = dropped {
            events.push(TimelineEvent::Reorder {
                from: *from,
                to: index,
            });
        }
    }
}

impl Default for TimelinePanel {
    fn default() -> Self {
        Self::new()
    }
}

/// Clamp a reorder destination into the last valid slot (`len - 1`).
///
/// The timeline emits the chip index under the pointer as the destination;
/// the App clamps it defensively before reordering (a drop past the end of
/// the strip must land on the last frame).
pub(crate) fn clamp_reorder_target(to: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        to.min(len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math::Rect2i;
    use crate::core::model::frame::Frame;
    use crate::core::model::region::Region;
    use crate::ui::theme::Theme;

    /// A two-frame sequence with distinct delays.
    fn sample_sequence() -> AnimationSequence {
        let mut seq = AnimationSequence::new("Animation 1");
        seq.push(Frame::new(
            Region::new(Rect2i::new(0, 0, 16, 16), "Frame 1"),
            100,
        ));
        seq.push(Frame::new(
            Region::new(Rect2i::new(16, 0, 16, 16), "Frame 2"),
            200,
        ));
        seq
    }

    /// Run one headless frame of the panel in a full-screen central panel.
    ///
    /// Same harness style as `src/ui/layers.rs` tests: a manual
    /// `egui::Context` driven by `run_ui` with synthetic events.
    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        panel: &mut TimelinePanel,
        sequence: &AnimationSequence,
        current: usize,
        playing: bool,
        looping: bool,
        out: &mut Vec<TimelineEvent>,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(800.0, 600.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = Theme::default_dark().colors;
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    panel.ui(ui, &theme, sequence, current, playing, looping, out);
                });
        });
        output.textures_delta.clear();
        output
    }

    /// All text rendered in the last frame, with the top-left position of each.
    fn rendered_texts(output: &egui::FullOutput) -> Vec<(String, egui::Pos2)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(text) => out.push((text.galley.text().to_string(), text.pos)),
                egui::Shape::Vec(shapes) => {
                    for s in shapes {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
    }

    /// Top-left positions of every rendered text equal to `text`, in paint
    /// order.
    fn text_positions(output: &egui::FullOutput, text: &str) -> Vec<egui::Pos2> {
        rendered_texts(output)
            .into_iter()
            .filter(|(t, _)| t == text)
            .map(|(_, pos)| pos)
            .collect()
    }

    /// All filled rects rendered in the last frame, with their fill color.
    fn rect_fills(output: &egui::FullOutput) -> Vec<(egui::Rect, egui::Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Rect(r) => out.push((r.rect, r.fill)),
                egui::Shape::Vec(shapes) => {
                    for s in shapes {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut rects);
        }
        rects
    }

    /// Simulate a full click (move, press, release) at `pos`.
    fn click(
        ctx: &egui::Context,
        pos: egui::Pos2,
        panel: &mut TimelinePanel,
        sequence: &AnimationSequence,
        current: usize,
        playing: bool,
        looping: bool,
        events: &mut Vec<TimelineEvent>,
    ) {
        run_frame(
            ctx,
            vec![egui::Event::PointerMoved(pos)],
            panel,
            sequence,
            current,
            playing,
            looping,
            events,
        );
        run_frame(
            ctx,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            panel,
            sequence,
            current,
            playing,
            looping,
            events,
        );
        run_frame(
            ctx,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            panel,
            sequence,
            current,
            playing,
            looping,
            events,
        );
    }

    /// Click the first widget whose rendered text is `text`, clicking just
    /// inside the text's top-left corner (which is inside the widget).
    fn click_text(
        ctx: &egui::Context,
        output: &egui::FullOutput,
        text: &str,
        panel: &mut TimelinePanel,
        sequence: &AnimationSequence,
        current: usize,
        playing: bool,
        looping: bool,
        events: &mut Vec<TimelineEvent>,
    ) {
        let pos = text_positions(output, text)
            .into_iter()
            .next()
            .expect("text should be rendered");
        click(
            ctx,
            pos + egui::vec2(4.0, 8.0),
            panel,
            sequence,
            current,
            playing,
            looping,
            events,
        );
    }

    #[test]
    fn plain_frame_emits_no_events() {
        let ctx = egui::Context::default();
        let mut panel = TimelinePanel::new();
        let seq = sample_sequence();
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);

        for text in ["#1", "#2", "+", "Loop", "⏮", "▶", "⏸"] {
            assert!(
                !text_positions(&output, text).is_empty(),
                "missing widget text {text}"
            );
        }
        assert!(events.is_empty(), "a plain frame emits no events");
    }

    #[test]
    fn clicking_chip_emits_select_frame() {
        let ctx = egui::Context::default();
        let mut panel = TimelinePanel::new();
        let seq = sample_sequence();
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);

        click_text(
            &ctx,
            &output,
            "#1",
            &mut panel,
            &seq,
            0,
            false,
            true,
            &mut events,
        );
        assert_eq!(events, vec![TimelineEvent::SelectFrame(0)]);
    }

    #[test]
    fn clicking_add_emits_add_frame() {
        let ctx = egui::Context::default();
        let mut panel = TimelinePanel::new();
        let seq = sample_sequence();
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);

        click_text(
            &ctx,
            &output,
            "+",
            &mut panel,
            &seq,
            0,
            false,
            true,
            &mut events,
        );
        assert_eq!(events, vec![TimelineEvent::AddFrame]);
    }

    #[test]
    fn clicking_remove_emits_remove_frame() {
        let ctx = egui::Context::default();
        let mut panel = TimelinePanel::new();
        let seq = sample_sequence();
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);

        // One remove button per frame; click the first frame's.
        let positions = text_positions(&output, "✖");
        assert_eq!(positions.len(), 2, "one remove button per frame");
        click(
            &ctx,
            positions[0] + egui::vec2(4.0, 8.0),
            &mut panel,
            &seq,
            0,
            false,
            true,
            &mut events,
        );
        assert_eq!(events, vec![TimelineEvent::RemoveFrame(0)]);
    }

    #[test]
    fn transport_buttons_emit_play_stop_restart() {
        let ctx = egui::Context::default();
        let mut panel = TimelinePanel::new();
        let seq = sample_sequence();
        let mut events = Vec::new();

        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);
        click_text(
            &ctx,
            &output,
            "▶",
            &mut panel,
            &seq,
            0,
            false,
            true,
            &mut events,
        );
        assert_eq!(events, vec![TimelineEvent::Play]);
        events.clear();

        // Stop is only enabled while playing.
        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, true, true, &mut events);
        click_text(
            &ctx,
            &output,
            "⏸",
            &mut panel,
            &seq,
            0,
            true,
            true,
            &mut events,
        );
        assert_eq!(events, vec![TimelineEvent::Stop]);
        events.clear();

        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);
        click_text(
            &ctx,
            &output,
            "⏮",
            &mut panel,
            &seq,
            0,
            false,
            true,
            &mut events,
        );
        assert_eq!(events, vec![TimelineEvent::Restart]);
    }

    #[test]
    fn clicking_loop_emits_toggle_loop() {
        let ctx = egui::Context::default();
        let mut panel = TimelinePanel::new();
        let seq = sample_sequence();
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);

        click_text(
            &ctx,
            &output,
            "Loop",
            &mut panel,
            &seq,
            0,
            false,
            true,
            &mut events,
        );
        assert_eq!(events, vec![TimelineEvent::ToggleLoop]);
    }

    #[test]
    fn current_chip_is_visually_selected() {
        let ctx = egui::Context::default();
        let mut panel = TimelinePanel::new();
        let seq = sample_sequence();
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &seq, 0, false, true, &mut events);

        // The current chip's label is painted with the selection background.
        let selection_fill = Theme::default_dark().colors.selection_bg_fill32();
        let covered_by_selection = |label: &str| {
            let pos = text_positions(&output, label)[0] + egui::vec2(4.0, 8.0);
            rect_fills(&output)
                .iter()
                .any(|(rect, fill)| *fill == selection_fill && rect.contains(pos))
        };
        assert!(
            covered_by_selection("#1"),
            "current chip should be painted with the selection fill"
        );
        assert!(
            !covered_by_selection("#2"),
            "inactive chip should not use the selection fill"
        );
    }

    #[test]
    fn clamp_reorder_target_clamps() {
        assert_eq!(clamp_reorder_target(0, 3), 0);
        assert_eq!(clamp_reorder_target(2, 3), 2);
        assert_eq!(clamp_reorder_target(99, 3), 2);
        assert_eq!(clamp_reorder_target(0, 0), 0);
        assert_eq!(clamp_reorder_target(5, 1), 0);
    }
}
