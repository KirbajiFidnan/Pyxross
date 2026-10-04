//! Toolbar + color/palette panel. R2/R5 milestones.
//!
//! A stateless view over the current tool, color and brush size: renders the
//! tool buttons, a preset color palette plus the current color, and the
//! brush-size slider. The widget never mutates app state — every gesture is
//! emitted as a [`ToolbarEvent`] for the App shell to apply (D62: deliberate
//! switches only; every event here is deliberate).

use crate::core::brush::{BrushShape, BrushSpec, ScatterShape};
use crate::core::color::Color;
use crate::core::palette::Palette;
use crate::core::transform::TransformAlgorithm;
use crate::input::{FieldierChild, Tool};
use crate::ui::theme::ThemeColors;

/// A user gesture on the toolbar, emitted by [`ToolbarWidget::ui`].
///
/// The App shell owns the tool/color/brush state and applies these events; the
/// widget itself is a pure view and never mutates the inputs it is given.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolbarEvent {
    /// The user clicked a tool button (a deliberate switch, D62).
    ToolSelected(Tool),
    /// The user clicked a color swatch.
    ColorChanged(Color),
    /// The user picked the secondary color (right-click on a palette swatch).
    SecondaryColorChanged(Color),
    /// The user moved the brush-size slider (1..=64).
    BrushSizeChanged(u8),
    /// The user picked a brush footprint shape (Square / Round).
    BrushShapeChanged(BrushShape),
    /// The user moved the brush-scatter control (0..=32 px radius, 0 = off).
    BrushScatterChanged(u8),
    /// The user picked the brush-scatter spread shape.
    BrushScatterShapeChanged(ScatterShape),
    /// The user moved the brush-tail taper control (-100..=100 percent, 0 = off).
    BrushTailChanged(i8),
    /// The user picked a Fieldier child (Rectangle / Wand / Lasso) in the Tool
    /// Property panel.
    FieldierChildChanged(FieldierChild),
    /// The user moved the wand-tolerance control in the Tool Property panel.
    WandToleranceChanged(u8),
    /// The user toggled the wand's restrict-to-region checkbox.
    WandRestrictToRegionChanged(bool),
    /// The user moved the Fill tool's tolerance control in the Tool Property panel.
    FillToleranceChanged(u8),
    /// The user toggled the Fill tool's contiguous checkbox.
    FillContiguousChanged(bool),
    /// The user toggled the Fill tool's restrict-to-region checkbox.
    FillRestrictToRegionChanged(bool),
    /// The user picked the free-angle rotation algorithm (RotSprite / CleanEdge
    /// / Rotxel) in the Tool Property panel while a transform is live.
    TransformAlgorithmChanged(TransformAlgorithm),
    /// The user typed a rotation into the Tile tool's rotation input. The value
    /// arrives ALREADY snapped to a multiple of 90 and normalized into
    /// `0..360` by the panel, so the App stores `(value / 90) % 4` quarter
    /// turns. Activates the sticky transform.
    TileRotationChanged(i32),
    /// The user toggled the Tile tool's horizontal-flip checkbox. Activates
    /// the sticky transform.
    TileFlipXChanged(bool),
    /// The user toggled the Tile tool's vertical-flip checkbox. Activates
    /// the sticky transform.
    TileFlipYChanged(bool),
    PaletteSelected(usize),
    /// The user swapped the primary and secondary colors (swap button / X key).
    SwapColors,
}

/// All tools in toolbox display order.
pub(crate) const TOOLS: [Tool; 6] = [
    Tool::Pencil,
    Tool::Eraser,
    Tool::Fill,
    Tool::Eyedropper,
    Tool::Fieldier,
    Tool::Tile,
];

/// Display label for a tool.
pub(crate) fn tool_label(tool: Tool) -> &'static str {
    match tool {
        Tool::Pencil => "Pencil",
        Tool::Eraser => "Eraser",
        Tool::Fill => "Fill",
        Tool::Eyedropper => "Eyedropper",
        Tool::Fieldier => "Fieldier",
        Tool::Tile => "Tile",
        Tool::Draw => "Draw",
    }
}

/// Fixed preset palette: black, white, red, green, blue, yellow, magenta, cyan.
const PRESETS: [Color; 8] = [
    Color::BLACK,
    Color::WHITE,
    Color::rgb(255, 0, 0),
    Color::rgb(0, 255, 0),
    Color::rgb(0, 0, 255),
    Color::rgb(255, 255, 0),
    Color::rgb(255, 0, 255),
    Color::rgb(0, 255, 255),
];

/// Brush-size slider bounds. The minimum is the 1 px pencil
/// ([`BrushSpec::PENCIL_1PX`]); the maximum is [`BrushSpec::MAX_SIZE`]. The App
/// sanitizes the emitted value into a [`BrushSpec`] (see
/// [`BrushSpec::sanitize`]).
const BRUSH_SIZE_MIN: u8 = BrushSpec::PENCIL_1PX.size;
const BRUSH_SIZE_MAX: u8 = BrushSpec::MAX_SIZE;

/// Side length of one color swatch, in points.
const SWATCH_SIZE: f32 = 24.0;

/// Vertical gap between toolbar sections (toolbox → palette → brush).
const SECTION_SPACING: f32 = 4.0;

/// Toolbar widget — a stateless view over tool/color/brush size.
///
/// Holds no persistent state today; it is a struct (rather than a free
/// function) so future drag state can be added without changing the API shape.
pub struct ToolbarWidget {}

impl ToolbarWidget {
    /// New toolbar.
    pub fn new() -> Self {
        Self {}
    }

    /// Paint the toolbar for one frame.
    ///
    /// `tool`/`color`/`brush_size` are the CURRENT app state (read-only inputs
    /// for highlighting); interactions append events to `events` for the App
    /// shell to apply. Never mutates the inputs directly — the App applies
    /// events (D62 semantics: deliberate switches only; this widget's events
    /// are all deliberate).
    ///
    /// Paints vertically: the tool column, then the color/palette row, then the
    /// brush-size slider. The App shell is expected to place this inside a
    /// `SidePanel`; the widget does not create the panel itself.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        tool: Tool,
        color: Color,
        brush_size: u8,
        events: &mut Vec<ToolbarEvent>,
    ) {
        self.ui_with_palettes(ui, theme, tool, color, brush_size, &[], events);
    }

    pub fn ui_with_palettes(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        tool: Tool,
        color: Color,
        brush_size: u8,
        palettes: &[Palette],
        events: &mut Vec<ToolbarEvent>,
    ) {
        // 1. Toolbox: one selectable label per tool; egui's selected styling
        //    highlights the active tool.
        for candidate in TOOLS {
            let is_active = candidate == tool;
            let response = ui
                .selectable_label(is_active, tool_label(candidate))
                .on_hover_text(tool_label(candidate));
            if response.clicked() {
                events.push(ToolbarEvent::ToolSelected(candidate));
            }
        }

        ui.add_space(SECTION_SPACING);

        // 2. Color/palette: the fixed presets plus the current color as a
        //    contiguous swatch. A swatch is outlined when it matches the
        //    current color.
        ui.horizontal_wrapped(|ui| {
            if palettes.is_empty() {
                for preset in PRESETS {
                    self.swatch(ui, theme, preset, color, events);
                }
            }
            for (palette_index, palette) in palettes.iter().enumerate() {
                if ui.selectable_label(false, &palette.name).clicked() {
                    events.push(ToolbarEvent::PaletteSelected(palette_index));
                }
                for entry in &palette.colors {
                    self.swatch(ui, theme, Color::from(*entry), color, events);
                }
            }
            self.swatch(ui, theme, color, color, events);
        });

        ui.add_space(SECTION_SPACING);

        // 3. Brush size: a local copy of the app value, clamped like
        //    `BrushSpec::sanitize` clamps UI input. Emit only when the slider
        //    actually moved this frame and the value differs from the input, so
        //    a no-op frame never emits.
        let mut size = brush_size.clamp(BRUSH_SIZE_MIN, BRUSH_SIZE_MAX);
        let slider =
            ui.add(egui::Slider::new(&mut size, BRUSH_SIZE_MIN..=BRUSH_SIZE_MAX).text("brush"));
        if slider.changed() && size != brush_size {
            events.push(ToolbarEvent::BrushSizeChanged(size));
        }
    }

    /// One color swatch: a clickable filled square, outlined when it is the
    /// current color. Clicking emits [`ToolbarEvent::ColorChanged`].
    fn swatch(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        swatch_color: Color,
        current: Color,
        events: &mut Vec<ToolbarEvent>,
    ) {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(SWATCH_SIZE, SWATCH_SIZE), egui::Sense::click());
        ui.painter()
            .rect_filled(rect, 4.0, color_to_color32(swatch_color));
        if swatch_color == current {
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.0, theme.selection_stroke_color32()),
                egui::StrokeKind::Inside,
            );
        }
        let response = response.on_hover_text(format!(
            "#{:02X}{:02X}{:02X}",
            swatch_color.r, swatch_color.g, swatch_color.b
        ));
        if response.clicked() {
            events.push(ToolbarEvent::ColorChanged(swatch_color));
        }
    }
}

impl ToolbarWidget {
    /// Returns the fixed preset palette as straight-alpha RGBA quads.
    ///
    /// Used by the persistence layer to snapshot the palette into a
    /// [`Document`](crate::core::document::Document).
    pub fn palette() -> Vec<[u8; 4]> {
        PRESETS.iter().map(|c| [c.r, c.g, c.b, c.a]).collect()
    }
}

impl Default for ToolbarWidget {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a straight-alpha [`Color`] to an egui color.
fn color_to_color32(color: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;

    /// Run one headless frame of the toolbar in a full-screen central panel.
    ///
    /// Same harness style as `src/ui/layers.rs` tests: a manual
    /// `egui::Context` driven by `run_ui` with synthetic events.
    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        widget: &mut ToolbarWidget,
        tool: Tool,
        color: Color,
        brush_size: u8,
        out: &mut Vec<ToolbarEvent>,
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
                    widget.ui(ui, &theme, tool, color, brush_size, out);
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

    /// All stroked rects rendered in the last frame, with their stroke color.
    fn rect_strokes(output: &egui::FullOutput) -> Vec<(egui::Rect, egui::Color32)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(egui::Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Rect(r) => {
                    if r.stroke.width > 0.0 {
                        out.push((r.rect, r.stroke.color));
                    }
                }
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
        widget: &mut ToolbarWidget,
        tool: Tool,
        color: Color,
        brush_size: u8,
        events: &mut Vec<ToolbarEvent>,
    ) {
        run_frame(
            ctx,
            vec![egui::Event::PointerMoved(pos)],
            widget,
            tool,
            color,
            brush_size,
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
            widget,
            tool,
            color,
            brush_size,
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
            widget,
            tool,
            color,
            brush_size,
            events,
        );
    }

    /// Click the first widget whose rendered text is `text`, clicking just
    /// inside the text's top-left corner (which is inside the widget).
    fn click_text(
        ctx: &egui::Context,
        output: &egui::FullOutput,
        text: &str,
        widget: &mut ToolbarWidget,
        tool: Tool,
        color: Color,
        brush_size: u8,
        events: &mut Vec<ToolbarEvent>,
    ) {
        let pos = text_positions(output, text)
            .into_iter()
            .next()
            .expect("text should be rendered");
        click(
            ctx,
            pos + egui::vec2(4.0, 8.0),
            widget,
            tool,
            color,
            brush_size,
            events,
        );
    }

    /// The slider rail: the only short, wide rect painted with the inactive
    /// widget fill (the tool buttons and value box are taller).
    fn slider_rail(ctx: &egui::Context, output: &egui::FullOutput) -> egui::Rect {
        let inactive_fill = ctx
            .style_of(egui::Theme::Dark)
            .visuals
            .widgets
            .inactive
            .bg_fill;
        rect_fills(output)
            .into_iter()
            .filter(|(rect, fill)| *fill == inactive_fill && rect.height() <= 10.0)
            .max_by(|a, b| a.0.width().total_cmp(&b.0.width()))
            .map(|(rect, _)| rect)
            .expect("slider rail should be painted")
    }

    /// Press at the left end of the rail and drag to the right end across a few
    /// frames, then release.
    fn drag_slider(
        ctx: &egui::Context,
        rail: egui::Rect,
        widget: &mut ToolbarWidget,
        tool: Tool,
        color: Color,
        brush_size: u8,
        events: &mut Vec<ToolbarEvent>,
    ) {
        let start = egui::pos2(rail.left() + 1.0, rail.center().y);
        let end = egui::pos2(rail.right() - 1.0, rail.center().y);
        let mid = egui::pos2((start.x + end.x) / 2.0, start.y);
        let press = |pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        let release = |pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        };
        run_frame(
            ctx,
            vec![egui::Event::PointerMoved(start)],
            widget,
            tool,
            color,
            brush_size,
            events,
        );
        run_frame(
            ctx,
            vec![press(start)],
            widget,
            tool,
            color,
            brush_size,
            events,
        );
        run_frame(
            ctx,
            vec![egui::Event::PointerMoved(mid)],
            widget,
            tool,
            color,
            brush_size,
            events,
        );
        run_frame(
            ctx,
            vec![egui::Event::PointerMoved(end)],
            widget,
            tool,
            color,
            brush_size,
            events,
        );
        run_frame(
            ctx,
            vec![release(end)],
            widget,
            tool,
            color,
            brush_size,
            events,
        );
    }

    #[test]
    fn default_state_renders_all_tool_labels() {
        let ctx = egui::Context::default();
        let mut widget = ToolbarWidget::new();
        let mut events = Vec::new();
        let output = run_frame(
            &ctx,
            vec![],
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        for label in ["Pencil", "Eraser", "Fill", "Eyedropper", "Fieldier", "Tile"] {
            assert!(
                !text_positions(&output, label).is_empty(),
                "missing tool label {label}"
            );
        }
        assert!(events.is_empty(), "a plain frame emits no events");
    }

    #[test]
    fn active_tool_label_is_visually_selected() {
        let ctx = egui::Context::default();
        let mut widget = ToolbarWidget::new();
        let mut events = Vec::new();
        let output = run_frame(
            &ctx,
            vec![],
            &mut widget,
            Tool::Eraser,
            Color::BLACK,
            1,
            &mut events,
        );

        // The active tool's label is painted with egui's built-in selectable
        // styling (the widget uses `selectable_label`, not the theme tokens).
        let selection_fill = ctx.style_of(egui::Theme::Dark).visuals.selection.bg_fill;
        let covered_by_selection = |label: &str| {
            let pos = text_positions(&output, label)[0] + egui::vec2(4.0, 8.0);
            rect_fills(&output)
                .iter()
                .any(|(rect, fill)| *fill == selection_fill && rect.contains(pos))
        };
        assert!(
            covered_by_selection("Eraser"),
            "active tool should be painted with the selection fill"
        );
        assert!(
            !covered_by_selection("Pencil"),
            "inactive tool should not use the selection fill"
        );
    }

    #[test]
    fn clicking_fill_label_emits_tool_selected() {
        let ctx = egui::Context::default();
        let mut widget = ToolbarWidget::new();
        let mut events = Vec::new();
        let output = run_frame(
            &ctx,
            vec![],
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        click_text(
            &ctx,
            &output,
            "Fill",
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );
        assert_eq!(events, vec![ToolbarEvent::ToolSelected(Tool::Fill)]);
    }

    #[test]
    fn clicking_swatch_emits_color_changed() {
        let ctx = egui::Context::default();
        let mut widget = ToolbarWidget::new();
        let mut events = Vec::new();
        let output = run_frame(
            &ctx,
            vec![],
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        // The red preset is unique (the current color is black).
        let red = Color::rgb(255, 0, 0);
        let red32 = egui::Color32::from_rgb(255, 0, 0);
        let rect = rect_fills(&output)
            .into_iter()
            .find(|(_, fill)| *fill == red32)
            .map(|(rect, _)| rect)
            .expect("red swatch should be painted");
        click(
            &ctx,
            rect.center(),
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );
        assert_eq!(events, vec![ToolbarEvent::ColorChanged(red)]);
    }

    #[test]
    fn current_color_swatch_is_highlighted() {
        let ctx = egui::Context::default();
        let mut widget = ToolbarWidget::new();
        let mut events = Vec::new();
        // A color that is not one of the presets, so its swatch is unique.
        let current = Color::rgb(123, 45, 67);
        let output = run_frame(
            &ctx,
            vec![],
            &mut widget,
            Tool::Pencil,
            current,
            1,
            &mut events,
        );

        let current32 = egui::Color32::from_rgb(123, 45, 67);
        let swatch = rect_fills(&output)
            .into_iter()
            .find(|(_, fill)| *fill == current32)
            .map(|(rect, _)| rect)
            .expect("current-color swatch should be painted");

        let selection_stroke = Theme::default_dark().colors.selection_stroke_color32();
        assert!(
            rect_strokes(&output)
                .iter()
                .any(|(rect, color)| *color == selection_stroke && *rect == swatch),
            "current-color swatch should be outlined with the selection stroke"
        );
    }

    #[test]
    fn brush_slider_drag_emits_brush_size_changed() {
        let ctx = egui::Context::default();
        let mut widget = ToolbarWidget::new();
        let mut events = Vec::new();
        let output = run_frame(
            &ctx,
            vec![],
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        let rail = slider_rail(&ctx, &output);
        drag_slider(
            &ctx,
            rail,
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        assert!(
            events
                .iter()
                .any(|e| matches!(e, ToolbarEvent::BrushSizeChanged(v) if *v != 1)),
            "dragging the slider should emit a changed brush size, got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ToolbarEvent::BrushSizeChanged(64))),
            "dragging to the right end should reach the max, got {events:?}"
        );
    }

    #[test]
    fn toolbox_lists_only_the_real_tools() {
        assert_eq!(TOOLS.len(), 6);
        assert!(
            !TOOLS.contains(&Tool::Draw),
            "the hidden Draw tool must never be selectable from the toolbox"
        );
        for tool in TOOLS {
            assert_ne!(tool, Tool::Draw);
        }
        assert!(TOOLS.contains(&Tool::Fieldier));
        assert!(TOOLS.contains(&Tool::Tile));
        assert_eq!(
            TOOLS.len(),
            6,
            "rectangle/wand/lasso are Fieldier children, not separate toolbox entries"
        );
    }

    #[test]
    fn swatch_and_brush_leave_tool_unchanged() {
        let ctx = egui::Context::default();
        let mut widget = ToolbarWidget::new();
        let mut events = Vec::new();
        let output = run_frame(
            &ctx,
            vec![],
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        // Click a swatch.
        let red32 = egui::Color32::from_rgb(255, 0, 0);
        let swatch = rect_fills(&output)
            .into_iter()
            .find(|(_, fill)| *fill == red32)
            .map(|(rect, _)| rect)
            .expect("red swatch should be painted");
        click(
            &ctx,
            swatch.center(),
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        // Drag the brush-size slider.
        let rail = slider_rail(&ctx, &output);
        drag_slider(
            &ctx,
            rail,
            &mut widget,
            Tool::Pencil,
            Color::BLACK,
            1,
            &mut events,
        );

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ToolbarEvent::ToolSelected(_))),
            "swatch/slider gestures must not select a tool, got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ToolbarEvent::ColorChanged(_))),
            "the swatch click should still emit ColorChanged"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ToolbarEvent::BrushSizeChanged(_))),
            "the slider drag should still emit BrushSizeChanged"
        );
    }
}
