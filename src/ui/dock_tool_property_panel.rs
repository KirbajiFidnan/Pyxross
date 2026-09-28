//! Tool Property dock panel: the active tool's name plus the per-tool property
//! area.
//!
//! The Draw tools (Pencil and Eraser) share the brush-size, brush-shape and
//! brush-scatter properties; the Fieldier shows its child picker plus the wand's
//! tolerance / contiguity / restrict-to-region. Every other tool keeps the
//! placeholder area.

use egui::vec2;

use crate::core::brush::{BrushShape, BrushSpec, DrawTool, ScatterShape};
use crate::core::transform::TransformAlgorithm;
use crate::input::{FieldierChild, Tool};
use crate::ui::dock_hosts::{ToolboxHost, ToolboxView};
use crate::ui::panel_dock::nest::NestView;
use crate::ui::panel_dock::{
    ButtonStyle, Component, NestContent, NestMode, PanelChrome, PanelId, PanelMetadata,
    PanelPlacement, PanelSpec,
};
use crate::ui::toolbar::{tool_label, ToolbarEvent};

/// The dock id for the Tool Property panel.
const TOOL_PROPERTY_PANEL_ID: u64 = 103;

/// Content height: the tool name plus the property area. The width follows the
/// panel so a narrow dock never overflows horizontally.
const TOOL_PROPERTY_CONTENT_HEIGHT: f32 = 160.0;

/// Placeholder shown where the active tool's editable properties will render.
const TOOL_PROPERTY_PLACEHOLDER: &str = "No editable properties for this tool yet.";

/// Label of the reset button that replaces the numeric input beside each
/// slider row; it emits the parameter's reset value.
const RESET_LABEL: &str = "Reset";

/// The tool name (top-left) followed by the per-tool property area.
struct ToolPropertyComponent {
    host: ToolboxHost,
}

impl Component for ToolPropertyComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        ui.set_min_size(vec2(ui.available_width(), TOOL_PROPERTY_CONTENT_HEIGHT));
        let view = self.host.view.borrow().clone();
        // A live transform takes over the panel: its rotation algorithm is the
        // only property, and the tool's own controls are suspended (mirroring
        // the transform-session input contract).
        if view.transform_active {
            ui.strong("Transform");
            self.transform_properties(ui, &view, chrome);
            return;
        }
        ui.strong(tool_label(view.tool));
        match view.tool {
            Tool::Pencil | Tool::Eraser => {
                self.draw_properties(ui, &view, chrome);
            }
            Tool::Fieldier => {
                self.fieldier_properties(ui, &view, chrome);
            }
            Tool::Fill | Tool::Eyedropper | Tool::Draw => {
                ui.weak(TOOL_PROPERTY_PLACEHOLDER);
            }
        }
    }
}

/// One property row: the parameter name on its own line, then the inputs
/// left-aligned below it with a small gap.
fn property_row(ui: &mut egui::Ui, label: &str, inputs: impl FnOnce(&mut egui::Ui)) {
    ui.label(label);
    ui.add_space(2.0);
    ui.horizontal_wrapped(inputs);
}

impl ToolPropertyComponent {
    /// The Draw tools' shared properties: brush size, brush shape (Square /
    /// Circle toggle), stepped tail, stamp scatter (each a
    /// `[numeric input][slider][Reset]` row), and the scatter spread shape
    /// (Square / Circle / Diamond selector).
    fn draw_properties(&self, ui: &mut egui::Ui, view: &ToolboxView, chrome: &PanelChrome) {
        let events = &self.host.events;
        let mut size = view
            .draw
            .size
            .clamp(BrushSpec::MIN_SIZE, BrushSpec::MAX_SIZE);
        property_row(ui, "Size", |ui| {
            let input = ui.add(
                egui::DragValue::new(&mut size).range(BrushSpec::MIN_SIZE..=BrushSpec::MAX_SIZE),
            );
            let slider = ui.add(
                egui::Slider::new(&mut size, BrushSpec::MIN_SIZE..=BrushSpec::MAX_SIZE)
                    .show_value(false),
            );
            let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
            if (input.changed() || slider.changed()) && size != view.draw.size {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::BrushSizeChanged(size));
            }
            if reset.clicked() {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::BrushSizeChanged(BrushSpec::MIN_SIZE));
            }
        });
        property_row(ui, "Shape", |ui| {
            let square = chrome.button(
                ui,
                "Square",
                ButtonStyle::toggled(view.draw.shape == BrushShape::Square),
            );
            let circle = chrome.button(
                ui,
                "Circle",
                ButtonStyle::toggled(view.draw.shape == BrushShape::Round),
            );
            if square.clicked() {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::BrushShapeChanged(BrushShape::Square));
            }
            if circle.clicked() {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::BrushShapeChanged(BrushShape::Round));
            }
        });
        property_row(ui, "Tail", |ui| {
            let mut tail = view
                .draw
                .tail
                .clamp(-DrawTool::MAX_TAIL, DrawTool::MAX_TAIL);
            let input = ui.add(
                egui::DragValue::new(&mut tail).range(-DrawTool::MAX_TAIL..=DrawTool::MAX_TAIL),
            );
            let mut position = tail_slider_from_value(tail);
            let slider = ui.add(egui::Slider::new(&mut position, -1.0..=1.0).show_value(false));
            let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
            if slider.changed() {
                tail = tail_value_from_slider(position);
            }
            if (input.changed() || slider.changed()) && tail != view.draw.tail {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::BrushTailChanged(tail));
            }
            if reset.clicked() {
                events.borrow_mut().push(ToolbarEvent::BrushTailChanged(0));
            }
        });
        property_row(ui, "Scatter", |ui| {
            let mut scatter = view.draw.scatter.min(DrawTool::MAX_SCATTER);
            let input = ui.add(egui::DragValue::new(&mut scatter).range(0..=DrawTool::MAX_SCATTER));
            let slider = ui
                .add(egui::Slider::new(&mut scatter, 0..=DrawTool::MAX_SCATTER).show_value(false));
            let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
            if (input.changed() || slider.changed()) && scatter != view.draw.scatter {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::BrushScatterChanged(scatter));
            }
            if reset.clicked() {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::BrushScatterChanged(0));
            }
        });
        property_row(ui, "Spread", |ui| {
            let shapes = [
                (ScatterShape::Square, "Square"),
                (ScatterShape::Circle, "Circle"),
                (ScatterShape::Diamond, "Diamond"),
            ];
            for (shape, label) in shapes {
                if chrome
                    .button(
                        ui,
                        label,
                        ButtonStyle::toggled(view.draw.scatter_shape == shape),
                    )
                    .clicked()
                {
                    events
                        .borrow_mut()
                        .push(ToolbarEvent::BrushScatterShapeChanged(shape));
                }
            }
        });
    }

    /// The Fieldier's properties: the child picker (Rectangle / Wand / Lasso)
    /// and, while the Wand child is active, the read-only contiguity checkbox,
    /// the wand tolerance (`[numeric input][slider][Reset]`) and the
    /// restrict-to-region checkbox.
    fn fieldier_properties(&self, ui: &mut egui::Ui, view: &ToolboxView, chrome: &PanelChrome) {
        let events = &self.host.events;
        property_row(ui, "Child", |ui| {
            let children = [
                (FieldierChild::Rectangle, "Rectangle"),
                (FieldierChild::Wand, "Wand"),
                (FieldierChild::Lasso, "Lasso"),
            ];
            for (child, label) in children {
                if chrome
                    .button(ui, label, ButtonStyle::toggled(view.child == child))
                    .clicked()
                {
                    events
                        .borrow_mut()
                        .push(ToolbarEvent::FieldierChildChanged(child));
                }
            }
        });
        if view.child != FieldierChild::Wand {
            return;
        }
        // Display-only: the effective contiguity already folds in Alt, and the
        // checkbox must never emit — the temp Alt override is a gesture-engine
        // concern, not a stored setting.
        let mut effective = view.wand_contiguous_effective;
        ui.add_enabled(false, egui::Checkbox::new(&mut effective, "Contiguous"));
        property_row(ui, "Tolerance", |ui| {
            let mut tolerance = view.wand.tolerance;
            let input = ui.add(egui::DragValue::new(&mut tolerance).range(0..=u8::MAX));
            let slider = ui.add(egui::Slider::new(&mut tolerance, 0..=u8::MAX).show_value(false));
            let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
            if (input.changed() || slider.changed()) && tolerance != view.wand.tolerance {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::WandToleranceChanged(tolerance));
            }
            if reset.clicked() {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::WandToleranceChanged(0));
            }
        });
        let mut restrict = view.wand.restrict_to_region;
        if ui.checkbox(&mut restrict, "Restrict to region").changed() {
            events
                .borrow_mut()
                .push(ToolbarEvent::WandRestrictToRegionChanged(restrict));
        }
    }

    /// The live transform's properties: the free-angle rotation algorithm
    /// selector, mirroring the Fieldier "Child" row. Only meaningful while
    /// `view.transform_active` (the caller branches on it).
    fn transform_properties(&self, ui: &mut egui::Ui, view: &ToolboxView, chrome: &PanelChrome) {
        let events = &self.host.events;
        property_row(ui, "Algorithm", |ui| {
            let algorithms = [
                (TransformAlgorithm::RotSprite, "RotSprite"),
                (TransformAlgorithm::CleanEdge, "CleanEdge"),
                (TransformAlgorithm::Rotxel, "Rotxel"),
            ];
            for (algorithm, label) in algorithms {
                if chrome
                    .button(
                        ui,
                        label,
                        ButtonStyle::toggled(view.transform_algorithm == algorithm),
                    )
                    .clicked()
                {
                    events
                        .borrow_mut()
                        .push(ToolbarEvent::TransformAlgorithmChanged(algorithm));
                }
            }
        });
    }
}

/// The tail value a slider position `t ∈ [-1, 1]` drives: `0` at the exact
/// centre, otherwise the magnitude grows from `1` at either end to `100` on
/// approaching the centre (`t = ±0.5 → ±50`, `t = ±0.75 → ±25`).
fn tail_value_from_slider(position: f32) -> i8 {
    if position == 0.0 {
        return 0;
    }
    let max = f32::from(DrawTool::MAX_TAIL);
    let magnitude = ((1.0 - position.abs()) * max).round().clamp(1.0, max) as i8;
    if position < 0.0 {
        -magnitude
    } else {
        magnitude
    }
}

/// The slider position for a stored tail value. The thumb is parked at the
/// centre of the value's bucket, so re-deriving the value from it returns the
/// stored value and the thumb stays consistent when the value changes
/// elsewhere (scroll, shortcut, undo).
fn tail_slider_from_value(value: i8) -> f32 {
    let value = value.clamp(-DrawTool::MAX_TAIL, DrawTool::MAX_TAIL);
    if value == 0 {
        return 0.0;
    }
    let max = f32::from(DrawTool::MAX_TAIL);
    let magnitude = f32::from(value.unsigned_abs());
    // A magnitude of 100 rounds from |t| < 0.005 (centred on 0.0025); every
    // smaller magnitude's bucket is centred on |t| = 1 - magnitude/100.
    let normalized = if magnitude >= max {
        0.9975
    } else {
        magnitude / max
    };
    let position = 1.0 - normalized;
    if value < 0 {
        -position
    } else {
        position
    }
}

/// Static nest: the current tool's name at the top-left, then the per-tool
/// property area.
pub(crate) fn build_tool_property_nest(host: &ToolboxHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ToolPropertyComponent { host: host.clone() });
    nest
}

/// Dock spec for the Tool Property panel (dock id `PanelId::new(103)`).
pub fn tool_property_panel_spec(host: &ToolboxHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(TOOL_PROPERTY_PANEL_ID),
        metadata: PanelMetadata::new("Tool Property", false, vec2(160.0, 120.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(40.0, 80.0), vec2(200.0, 200.0)),
        content: Box::new(build_tool_property_nest(host)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_slider_maps_extremes_to_one_and_centre_to_one_hundred() {
        assert_eq!(tail_value_from_slider(1.0), 1, "the extreme right end is 1");
        assert_eq!(
            tail_value_from_slider(-1.0),
            -1,
            "the extreme left end is -1"
        );
        assert_eq!(tail_value_from_slider(0.0), 0, "the exact centre is 0");
        assert_eq!(tail_value_from_slider(0.5), 50);
        assert_eq!(tail_value_from_slider(-0.5), -50);
        assert_eq!(tail_value_from_slider(-0.75), -25);
        assert_eq!(tail_value_from_slider(0.75), 25);
        assert_eq!(
            tail_value_from_slider(0.001),
            100,
            "approaching the centre the magnitude reaches the 100 cap"
        );
        assert_eq!(tail_value_from_slider(-0.001), -100);
    }

    #[test]
    fn tail_slider_round_trips_with_the_stored_value() {
        for value in -DrawTool::MAX_TAIL..=DrawTool::MAX_TAIL {
            let position = tail_slider_from_value(value);
            assert_eq!(
                tail_value_from_slider(position),
                value,
                "stored {value} must map to a thumb position that maps back to it"
            );
        }
        for position in [-1.0f32, -0.75, -0.5, -0.25, 0.25, 0.5, 0.75, 1.0] {
            let value = tail_value_from_slider(position);
            assert_eq!(
                tail_value_from_slider(tail_slider_from_value(value)),
                value,
                "position {position} must be stable through the value round trip"
            );
        }
        assert_eq!(tail_slider_from_value(0), 0.0);
        assert!(tail_slider_from_value(1) > 0.9);
        assert!(tail_slider_from_value(-1) < -0.9);
        assert!(
            tail_slider_from_value(DrawTool::MAX_TAIL) > 0.0
                && tail_slider_from_value(DrawTool::MAX_TAIL) < 0.005,
            "the maximum magnitude parks the thumb just off the centre"
        );
    }
}
