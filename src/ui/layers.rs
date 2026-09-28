//! Layers panel. R2 milestone.
//!
//! A read-only view over [`crate::core::model::LayerStack`]: renders one row
//! per layer (bottom-to-top order as stored) with name, visibility toggle,
//! blend-mode combo, opacity slider and a delete button. Row reordering is
//! drag-and-drop (the row's drop target is resolved by [`drop_placement`]).
//! The panel never mutates the stack — every gesture is emitted as a
//! [`LayerPanelEvent`] for the App shell to apply as exactly one undoable
//! command (D59). The panel is decoupled from `App` by design.

use crate::core::model::{BlendMode, Layer, LayerId, LayerStack};
use crate::ui::dock_hosts::LayerView;
use crate::ui::theme::ThemeColors;

/// A user gesture on the layer panel, emitted by [`LayerPanel::ui`].
///
/// The App shell owns the [`LayerStack`] and applies these events; the panel
/// itself is a pure view and never mutates the stack.
#[derive(Clone, PartialEq, Debug)]
pub enum LayerPanelEvent {
    /// Clicked a layer row (the App decides activation).
    Select(LayerId),
    /// Pressed "add layer".
    Add,
    /// Pressed delete for `id`; the App removes the whole subtree.
    Remove(LayerId),
    /// Toggled the eye of `id`.
    ToggleVisible(LayerId),
    /// Committed a new name for `id`.
    Rename(LayerId, String),
    /// Toggled the floating settings popup for `id`.
    SettingsToggle(LayerId),
    /// Toggled expansion of the group row `id`.
    ToggleExpanded(LayerId),
    /// Dropped `dragged` onto `target` (ids only; the App resolves the placement).
    ReorderDropped { dragged: LayerId, target: LayerId },
    /// Pressed "merge down" for the active layer.
    MergeDown,
    /// Pressed "new group" (wraps the active layer).
    CreateGroup,
    /// Pressed "ungroup" for the group `id`.
    Ungroup(LayerId),
    /// Opacity changed for `id` (0..=1).
    SetOpacity(LayerId, f32),
    /// Blend mode changed for `id`.
    SetBlend(LayerId, BlendMode),
}

/// Where a drop should place the dragged layer relative to the drop target.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DropPlacement {
    /// Directly above the target (dragged came from below the target).
    Above,
    /// Directly below the target (dragged came from above the target).
    Below,
    /// Into the target group, appended as its last child.
    IntoGroup,
}

/// Resolves the drop of `dragged` onto `target` from the current flattened view.
/// `LayerView.rows` is DFS pre-order, TOP layer first (row 0 is the topmost layer).
pub fn drop_placement(
    dragged: LayerId,
    target: LayerId,
    view: &LayerView,
) -> Option<DropPlacement> {
    if dragged == target {
        return None;
    }
    let dragged_idx = view.rows.iter().position(|row| row.id == dragged)?;
    let target_idx = view.rows.iter().position(|row| row.id == target)?;
    let target_row = &view.rows[target_idx];

    if target_row.is_group {
        // The target group's subtree is every following row with a greater
        // depth, up to (exclusive) the first row whose depth is <= the group's.
        let inside_subtree = view.rows[target_idx + 1..]
            .iter()
            .take_while(|row| row.depth > target_row.depth)
            .any(|row| row.id == dragged);
        if !inside_subtree {
            return Some(DropPlacement::IntoGroup);
        }
    }

    if dragged_idx > target_idx {
        Some(DropPlacement::Above)
    } else {
        Some(DropPlacement::Below)
    }
}

/// Display label for a blend mode (D58: normal/multiply/screen/add).
fn blend_label(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "Normal",
        BlendMode::Multiply => "Multiply",
        BlendMode::Screen => "Screen",
        BlendMode::Add => "Add",
    }
}

/// All blend modes in display order.
const BLEND_MODES: [BlendMode; 4] = [
    BlendMode::Normal,
    BlendMode::Multiply,
    BlendMode::Screen,
    BlendMode::Add,
];

/// Vertical gap between the "Add layer" button and the layer list.
const SECTION_SPACING: f32 = 4.0;

/// Layer panel widget — a read-only view over the layer stack.
///
/// Holds no persistent state today; it is a struct (rather than a free
/// function) so drag-reorder state can be added later without changing the
/// API shape.
pub struct LayerPanel {}

impl LayerPanel {
    /// New panel.
    pub fn new() -> Self {
        Self {}
    }

    /// Render the panel for one frame.
    ///
    /// * `layers` — a snapshot borrow of the stack (never mutated here).
    /// * `events` — appended with every user gesture this frame.
    ///
    /// Renders one row per layer in bottom-to-top order as stored (the bottom
    /// layer's row is the first, i.e. topmost, row). The active row is tinted
    /// with the selection background. The natural row width is ~330 px; the
    /// App shell should give the panel at least that much.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        layers: &LayerStack,
        events: &mut Vec<LayerPanelEvent>,
    ) {
        if ui.button("+ Add layer").clicked() {
            events.push(LayerPanelEvent::Add);
        }
        ui.add_space(SECTION_SPACING);

        let active_id = layers.active_layer_id();
        let selection_fill = theme.selection_bg_fill32();

        for layer in layers.iter() {
            let is_active = layer.id == active_id;
            let row_bg = if is_active {
                selection_fill
            } else {
                egui::Color32::TRANSPARENT
            };
            egui::Frame::NONE
                .fill(row_bg)
                .inner_margin(egui::Margin::symmetric(4, 2))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    self.row_ui(ui, layer, is_active, events);
                });
        }
    }

    /// One layer row: eye toggle, name, blend combo, opacity slider and a
    /// delete button.
    fn row_ui(
        &mut self,
        ui: &mut egui::Ui,
        layer: &Layer,
        is_active: bool,
        events: &mut Vec<LayerPanelEvent>,
    ) {
        let id = layer.id;
        ui.horizontal(|ui| {
            // Visibility toggle (eye): "selected" while the layer is visible.
            if ui.selectable_label(layer.visible, "👁").clicked() {
                events.push(LayerPanelEvent::ToggleVisible(id));
            }

            // Name — clicking always emits Select; the App decides what to do
            // when the row is already active.
            if ui.selectable_label(is_active, &layer.name).clicked() {
                events.push(LayerPanelEvent::Select(id));
            }

            // Blend-mode combo.
            let mut blend = layer.blend;
            egui::ComboBox::from_id_salt(("blend", id.as_u64()))
                .selected_text(blend_label(layer.blend))
                .width(80.0)
                .show_ui(ui, |ui| {
                    for mode in BLEND_MODES {
                        if ui
                            .selectable_value(&mut blend, mode, blend_label(mode))
                            .changed()
                        {
                            events.push(LayerPanelEvent::SetBlend(id, mode));
                        }
                    }
                });

            // Opacity slider. Emits on `changed()` (every frame the value
            // moves, i.e. live during a drag); the App coalesces the stream
            // into one undo step per gesture.
            let mut opacity = layer.opacity;
            let slider = ui.add(
                egui::Slider::new(&mut opacity, 0.0..=1.0)
                    .text("opacity")
                    .fixed_decimals(2),
            );
            if slider.changed() {
                events.push(LayerPanelEvent::SetOpacity(id, opacity));
            }

            if ui.button("🗑").on_hover_text("Delete layer").clicked() {
                events.push(LayerPanelEvent::Remove(id));
            }
        });
    }
}

impl Default for LayerPanel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::LayerStack;
    use crate::ui::theme::Theme;

    /// Run one headless frame of the panel in a full-screen central panel.
    ///
    /// Same harness style as `src/ui/canvas.rs` tests: a manual
    /// `egui::Context` driven by `run_ui` with synthetic events.
    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        panel: &mut LayerPanel,
        layers: &LayerStack,
        out: &mut Vec<LayerPanelEvent>,
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
                    panel.ui(ui, &theme, layers, out);
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
    /// order (rows paint bottom-to-top, widgets left-to-right).
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
        panel: &mut LayerPanel,
        layers: &LayerStack,
        events: &mut Vec<LayerPanelEvent>,
    ) {
        run_frame(
            ctx,
            vec![egui::Event::PointerMoved(pos)],
            panel,
            layers,
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
            layers,
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
            layers,
            events,
        );
    }

    /// Click the first widget whose rendered text is `text`, clicking just
    /// inside the text's top-left corner (which is inside the widget).
    fn click_text(
        ctx: &egui::Context,
        output: &egui::FullOutput,
        text: &str,
        panel: &mut LayerPanel,
        layers: &LayerStack,
        events: &mut Vec<LayerPanelEvent>,
    ) {
        let pos = text_positions(output, text)
            .into_iter()
            .next()
            .expect("text should be rendered");
        click(ctx, pos + egui::vec2(4.0, 8.0), panel, layers, events);
    }

    #[test]
    fn default_stack_renders_one_row() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let layers = LayerStack::new(64, 64);
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        let texts = rendered_texts(&output);
        assert!(texts.iter().any(|(t, _)| t == "Layer 1"), "layer name");
        assert!(texts.iter().any(|(t, _)| t == "+ Add layer"), "add button");
        assert!(texts.iter().any(|(t, _)| t == "Normal"), "blend combo");
        assert!(texts.iter().any(|(t, _)| t == "opacity"), "opacity label");
        assert!(events.is_empty(), "a plain frame emits no events");
    }

    #[test]
    fn three_layer_stack_renders_rows_bottom_to_top() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let mut layers = LayerStack::new(64, 64);
        layers.add_layer("Mid");
        layers.add_layer("Top");
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        for name in ["Layer 1", "Mid", "Top"] {
            assert!(
                !text_positions(&output, name).is_empty(),
                "missing row for {name}"
            );
        }
        // Bottom-to-top order as stored: the bottom layer's row is rendered
        // first (top of the panel), so its name has the smallest y.
        let y = |name: &str| text_positions(&output, name)[0].y;
        assert!(y("Layer 1") < y("Mid"), "bottom row should be above Mid");
        assert!(y("Mid") < y("Top"), "Mid row should be above Top");
    }

    #[test]
    fn clicking_inactive_row_emits_select() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let mut layers = LayerStack::new(64, 64);
        let top = layers.add_layer("Top");
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        click_text(&ctx, &output, "Top", &mut panel, &layers, &mut events);
        assert_eq!(events, vec![LayerPanelEvent::Select(top)]);
    }

    #[test]
    fn clicking_active_row_still_emits_select() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let layers = LayerStack::new(64, 64);
        let active = layers.active_layer_id();
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        click_text(&ctx, &output, "Layer 1", &mut panel, &layers, &mut events);
        assert_eq!(events, vec![LayerPanelEvent::Select(active)]);
    }

    #[test]
    fn add_button_emits_add() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let layers = LayerStack::new(64, 64);
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        click_text(
            &ctx,
            &output,
            "+ Add layer",
            &mut panel,
            &layers,
            &mut events,
        );
        assert_eq!(events, vec![LayerPanelEvent::Add]);
    }

    #[test]
    fn delete_button_emits_remove() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let mut layers = LayerStack::new(64, 64);
        let top = layers.add_layer("Top");
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        // Two delete buttons; click the top layer's (the second in paint order).
        let positions = text_positions(&output, "🗑");
        assert_eq!(positions.len(), 2, "one delete button per layer");
        click(
            &ctx,
            positions[1] + egui::vec2(4.0, 8.0),
            &mut panel,
            &layers,
            &mut events,
        );
        assert_eq!(events, vec![LayerPanelEvent::Remove(top)]);
    }

    #[test]
    fn visibility_toggle_emits_toggle_visible() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let mut layers = LayerStack::new(64, 64);
        let top = layers.add_layer("Top");
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        // Click the top layer's eye (the second in paint order).
        let positions = text_positions(&output, "👁");
        assert_eq!(positions.len(), 2, "one eye per layer");
        click(
            &ctx,
            positions[1] + egui::vec2(4.0, 8.0),
            &mut panel,
            &layers,
            &mut events,
        );
        assert_eq!(events, vec![LayerPanelEvent::ToggleVisible(top)]);
    }

    #[test]
    fn blend_combo_selecting_multiply_emits_set_blend() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let layers = LayerStack::new(64, 64);
        let id = layers.active_layer_id();
        let mut events = Vec::new();

        // Layout frame: the combo button shows "Normal".
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);
        // Open the blend combo.
        click_text(&ctx, &output, "Normal", &mut panel, &layers, &mut events);
        // Layout frame with the popup open; find "Multiply".
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);
        assert!(
            !text_positions(&output, "Multiply").is_empty(),
            "popup should list Multiply"
        );
        // Click "Multiply".
        click_text(&ctx, &output, "Multiply", &mut panel, &layers, &mut events);
        assert_eq!(
            events,
            vec![LayerPanelEvent::SetBlend(id, BlendMode::Multiply)]
        );
    }

    #[test]
    fn active_row_is_visually_selected() {
        let ctx = egui::Context::default();
        let mut panel = LayerPanel::new();
        let mut layers = LayerStack::new(64, 64);
        layers.add_layer("Top");
        let mut events = Vec::new();
        let output = run_frame(&ctx, vec![], &mut panel, &layers, &mut events);

        // The active row's name is painted with the selection background.
        let selection_fill = Theme::default_dark().colors.selection_bg_fill32();
        let covered_by_selection = |name: &str| {
            let pos = text_positions(&output, name)[0] + egui::vec2(4.0, 8.0);
            rect_fills(&output)
                .iter()
                .any(|(rect, fill)| *fill == selection_fill && rect.contains(pos))
        };
        assert!(
            covered_by_selection("Layer 1"),
            "active row name should be painted with the selection fill"
        );
        assert!(
            !covered_by_selection("Top"),
            "inactive row name should not use the selection fill"
        );
    }

    // NOTE: the opacity slider has no exact-value test. The headless harness
    // cannot reliably locate the slider rail to drag it to a precise value
    // (the rail is a plain rounded rect sharing the generic inactive widget
    // fill, indistinguishable from other chrome), and the slider's value text
    // position does not map linearly to the rail. The emit-on-`changed()`
    // mechanism is identical to the blend combo's, which is covered above.

    // --- drop_placement ---

    use crate::ui::dock_hosts::LayerRow;

    /// A leaf row at `depth` (groups opt in via `is_group`).
    fn row(id: u64, name: &str, is_group: bool, depth: usize) -> LayerRow {
        LayerRow {
            id: LayerId::new(id),
            name: name.to_string(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            is_group,
            depth,
            has_children: false,
            expanded: true,
        }
    }

    fn view(rows: Vec<LayerRow>) -> LayerView {
        LayerView {
            rows,
            active: None,
            merge_enabled: false,
            delete_enabled: false,
        }
    }

    #[test]
    fn drop_placement_above_when_dragged_is_below_the_target() {
        // Rows are DFS pre-order, topmost row first: A, B, C.
        let view = view(vec![
            row(1, "A", false, 0),
            row(2, "B", false, 0),
            row(3, "C", false, 0),
        ]);
        assert_eq!(
            drop_placement(LayerId::new(3), LayerId::new(2), &view),
            Some(DropPlacement::Above),
            "C is visually below B, so it drops directly above B"
        );
    }

    #[test]
    fn drop_placement_below_when_dragged_is_above_the_target() {
        let view = view(vec![
            row(1, "A", false, 0),
            row(2, "B", false, 0),
            row(3, "C", false, 0),
        ]);
        assert_eq!(
            drop_placement(LayerId::new(1), LayerId::new(2), &view),
            Some(DropPlacement::Below),
            "A is visually above B, so it drops directly below B"
        );
    }

    #[test]
    fn drop_placement_into_group_for_a_group_target() {
        // G (group) with children A, B, then a root leaf C.
        let view = view(vec![
            row(1, "G", true, 0),
            row(2, "A", false, 1),
            row(3, "B", false, 1),
            row(4, "C", false, 0),
        ]);
        assert_eq!(
            drop_placement(LayerId::new(4), LayerId::new(1), &view),
            Some(DropPlacement::IntoGroup),
            "C is outside G's subtree, so it drops into G"
        );
    }

    #[test]
    fn drop_placement_returns_none_for_self_or_unknown_ids() {
        let view = view(vec![row(1, "A", false, 0), row(2, "B", false, 0)]);
        assert_eq!(
            drop_placement(LayerId::new(1), LayerId::new(1), &view),
            None
        );
        assert_eq!(
            drop_placement(LayerId::new(1), LayerId::new(99), &view),
            None,
            "unknown target"
        );
        assert_eq!(
            drop_placement(LayerId::new(99), LayerId::new(1), &view),
            None,
            "unknown dragged"
        );
    }

    #[test]
    fn drop_placement_inside_a_group_uses_sibling_rules() {
        // G (group) with children A, B, then a root leaf C.
        let view = view(vec![
            row(1, "G", true, 0),
            row(2, "A", false, 1),
            row(3, "B", false, 1),
            row(4, "C", false, 0),
        ]);
        // A is already inside G's subtree: sibling rules apply, and A sits
        // below G in the flattened view, so it drops directly above G.
        assert_eq!(
            drop_placement(LayerId::new(2), LayerId::new(1), &view),
            Some(DropPlacement::Above)
        );
        // Sibling-to-sibling inside the group: B is below A ⇒ Above.
        assert_eq!(
            drop_placement(LayerId::new(3), LayerId::new(2), &view),
            Some(DropPlacement::Above)
        );
        // A is above B ⇒ Below.
        assert_eq!(
            drop_placement(LayerId::new(2), LayerId::new(3), &view),
            Some(DropPlacement::Below)
        );
    }

    #[test]
    fn event_carries_rename_payload() {
        let event = LayerPanelEvent::Rename(LayerId::new(7), "Sketch".to_string());
        let cloned = event.clone();
        assert_eq!(cloned, event);
        match cloned {
            LayerPanelEvent::Rename(id, name) => {
                assert_eq!(id, LayerId::new(7));
                assert_eq!(name, "Sketch");
            }
            other => panic!("clone should stay a Rename event, got {other:?}"),
        }
    }
}
