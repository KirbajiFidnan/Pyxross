//! Tool Property dock panel: the active tool's name plus the per-tool property
//! area.
//!
//! The Draw tools (Pencil and Eraser) share the brush-size, brush-shape and
//! brush-scatter properties; the Fieldier shows its child picker plus the wand's
//! tolerance / contiguity / restrict-to-region; the Fill tool shows its own
//! tolerance / contiguity / restrict-to-region. Every other tool keeps the
//! placeholder area.

use egui::vec2;

use crate::core::brush::{
    BrushPattern, BrushPatternKind, BrushShape, BrushSpec, DrawTool, ScatterShape,
};
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
pub const TOOL_PROPERTY_PANEL_ID: u64 = 103;

/// Minimum content height of the Tool Property nest.
const TOOL_PROPERTY_CONTENT_HEIGHT: f32 = 80.0;

/// Placeholder shown where the active tool's editable properties will render.
const TOOL_PROPERTY_PLACEHOLDER: &str = "No editable properties for this tool yet.";

/// Label of the reset button that replaces the numeric input beside each
/// slider row; it emits the parameter's reset value.
const RESET_LABEL: &str = "Reset";

/// Bounds of the Tile tool's rotation input. The stored value is always a
/// normalized 90 multiple, but the box accepts up to three full turns each way
/// so scrolling (or typing) can pass through a full turn before normalizing.
const ROTATION_INPUT_MIN: i32 = -1080;
const ROTATION_INPUT_MAX: i32 = 1080;

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
        if view.text_transform_active {
            ui.strong("Text Transform");
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(4.0);
            self.text_properties(ui, &view, chrome);
            return;
        }
        if view.transform_active {
            ui.strong("Transform");
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(4.0);
            self.transform_properties(ui, &view, chrome);
            return;
        }
        ui.strong(tool_label(view.tool));
        ui.add_space(2.0);
        ui.separator();
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("tool_property_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| match view.tool {
                Tool::Pencil | Tool::Eraser => {
                    self.draw_properties(ui, &view, chrome);
                }
                Tool::Fieldier => {
                    self.fieldier_properties(ui, &view, chrome);
                }
                Tool::Fill => {
                    self.fill_properties(ui, &view, chrome);
                }
                Tool::Tile => {
                    self.tile_properties(ui, &view);
                }
                Tool::Text => {
                    self.text_properties(ui, &view, chrome);
                }
                Tool::Eyedropper | Tool::Draw | Tool::AnimationPlacer => {
                    ui.weak(TOOL_PROPERTY_PLACEHOLDER);
                }
            });
    }
}

/// One property row: the parameter name on its own line, then the inputs
/// aligned below it with consistent vertical rhythm.
fn property_row(ui: &mut egui::Ui, label: &str, inputs: impl FnOnce(&mut egui::Ui)) {
    ui.weak(label);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        inputs(ui);
    });
    ui.add_space(6.0);
}

/// Custom filled-track slider with crisp pixel-editor styling:
/// dark recessed track, accent fill from origin (or center when `bipolar`),
/// and a sharp 8x14px thumb.
pub(crate) fn pro_slider(
    ui: &mut egui::Ui,
    id_salt: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    bipolar: bool,
) -> egui::Response {
    let min = *range.start();
    let max = *range.end();
    let span = (max - min).max(f32::EPSILON);
    let width = ui.available_width().max(40.0);
    let height = 18.0;
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, height), egui::Sense::click_and_drag());

    let track_inset = 5.0;
    let track_left = rect.left() + track_inset;
    let track_right = (rect.right() - track_inset).max(track_left + 1.0);
    let track_w = track_right - track_left;

    if response.dragged() || response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let t = ((pos.x - track_left) / track_w).clamp(0.0, 1.0);
            let new_val = (min + t * span).clamp(min, max);
            if (new_val - *value).abs() > f32::EPSILON {
                *value = new_val;
                response.mark_changed();
            }
        }
    }

    let t = ((*value - min) / span).clamp(0.0, 1.0);
    let handle_x = track_left + t * track_w;
    let track_rect = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.center().y),
        vec2(track_w, 4.0),
    );

    let visuals = ui.visuals();
    let painter = ui.painter();
    painter.rect_filled(track_rect, 1.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        track_rect,
        1.0,
        visuals.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    let fill_color = visuals.selection.stroke.color;
    let fill_rect = if bipolar {
        let zero_t = ((0.0 - min) / span).clamp(0.0, 1.0);
        let zero_x = track_left + zero_t * track_w;
        egui::Rect::from_min_max(
            egui::pos2(zero_x.min(handle_x), track_rect.top()),
            egui::pos2(zero_x.max(handle_x), track_rect.bottom()),
        )
    } else {
        egui::Rect::from_min_max(
            track_rect.min,
            egui::pos2(handle_x, track_rect.bottom()),
        )
    };
    if fill_rect.width() > 0.5 {
        painter.rect_filled(fill_rect, 1.0, fill_color);
    }

    let handle_rect = egui::Rect::from_center_size(
        egui::pos2(handle_x, rect.center().y),
        vec2(8.0, 14.0),
    );
    let active = response.hovered() || response.dragged();
    let handle_fill = if active {
        visuals.widgets.hovered.bg_fill
    } else {
        visuals.widgets.inactive.bg_fill
    };
    let handle_stroke = if active {
        visuals.selection.stroke
    } else {
        visuals.widgets.active.fg_stroke
    };
    painter.rect_filled(handle_rect, 1.5, handle_fill);
    painter.rect_stroke(handle_rect, 1.5, handle_stroke, egui::StrokeKind::Inside);

    let _ = id_salt;
    response
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
        property_row(ui, "Size (Shift+Scroll)", |ui| {
            let input = ui.add_sized(
                vec2(42.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut size).range(BrushSpec::MIN_SIZE..=BrushSpec::MAX_SIZE),
            );
            let mut size_f = size as f32;
            let slider_w = (ui.available_width() - 50.0).max(40.0);
            let slider = ui
                .allocate_ui(vec2(slider_w, 18.0), |ui| {
                    pro_slider(
                        ui,
                        "brush-size",
                        &mut size_f,
                        (BrushSpec::MIN_SIZE as f32)..=(BrushSpec::MAX_SIZE as f32),
                        false,
                    )
                })
                .inner;
            if slider.changed() {
                size = size_f.round() as u8;
            }
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
        property_row(ui, "Shape (Ctrl+Scroll)", |ui| {
            let current = match view.draw.shape {
                BrushShape::Square => "Square",
                BrushShape::Round => "Circle",
            };
            egui::ComboBox::from_id_salt("tool_prop_brush_shape")
                .width(ui.available_width().clamp(96.0, 180.0))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (shape, label) in [
                        (BrushShape::Square, "Square"),
                        (BrushShape::Round, "Circle"),
                    ] {
                        let selected = view.draw.shape == shape;
                        if chrome.combo_item(ui, selected, label).clicked() && !selected {
                            events
                                .borrow_mut()
                                .push(ToolbarEvent::BrushShapeChanged(shape));
                        }
                    }
                });
        });
        property_row(ui, "Tail", |ui| {
            let mut tail = view
                .draw
                .tail
                .clamp(-DrawTool::MAX_TAIL, DrawTool::MAX_TAIL);
            let input = ui.add_sized(
                vec2(42.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut tail).range(-DrawTool::MAX_TAIL..=DrawTool::MAX_TAIL),
            );
            let mut position = tail_slider_from_value(tail);
            let slider_w = (ui.available_width() - 50.0).max(40.0);
            let slider = ui
                .allocate_ui(vec2(slider_w, 18.0), |ui| {
                    pro_slider(ui, "brush-tail", &mut position, -1.0..=1.0, true)
                })
                .inner;
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
        property_row(ui, "Scatter (Alt+Scroll)", |ui| {
            let mut scatter = view.draw.scatter.min(DrawTool::MAX_SCATTER);
            let input = ui.add_sized(
                vec2(42.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut scatter).range(0..=DrawTool::MAX_SCATTER),
            );
            let mut scatter_f = scatter as f32;
            let slider_w = (ui.available_width() - 50.0).max(40.0);
            let slider = ui
                .allocate_ui(vec2(slider_w, 18.0), |ui| {
                    pro_slider(
                        ui,
                        "brush-scatter",
                        &mut scatter_f,
                        0.0..=(DrawTool::MAX_SCATTER as f32),
                        false,
                    )
                })
                .inner;
            if slider.changed() {
                scatter = scatter_f.round() as u8;
            }
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
            let current = shapes
                .iter()
                .find(|(s, _)| *s == view.draw.scatter_shape)
                .map(|(_, l)| *l)
                .unwrap_or("Square");
            egui::ComboBox::from_id_salt("tool_prop_scatter_shape")
                .width(ui.available_width().clamp(96.0, 180.0))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (shape, label) in shapes {
                        let selected = view.draw.scatter_shape == shape;
                        if chrome.combo_item(ui, selected, label).clicked() && !selected {
                            events
                                .borrow_mut()
                                .push(ToolbarEvent::BrushScatterShapeChanged(shape));
                        }
                    }
                });
        });
        let pattern = view.draw.pattern;
        property_row(ui, "Pattern", |ui| {
            let kinds = [
                (BrushPatternKind::Normal, "Normal"),
                (BrushPatternKind::BayerDither, "Bayer Dither"),
                (BrushPatternKind::Line, "Line"),
            ];
            let current = kinds
                .iter()
                .find(|(k, _)| *k == pattern.kind)
                .map(|(_, l)| *l)
                .unwrap_or("Normal");
            egui::ComboBox::from_id_salt("tool_prop_brush_pattern")
                .width(ui.available_width().clamp(96.0, 180.0))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (kind, label) in kinds {
                        let selected = pattern.kind == kind;
                        if chrome.combo_item(ui, selected, label).clicked() && !selected {
                            events
                                .borrow_mut()
                                .push(ToolbarEvent::BrushPatternKindChanged(kind));
                        }
                    }
                });
        });
        match pattern.kind {
            BrushPatternKind::Normal => {}
            BrushPatternKind::BayerDither => {
                property_row(ui, "Density", |ui| {
                    let mut density = pattern.dither_density.clamp(
                        BrushPattern::MIN_DITHER_DENSITY,
                        BrushPattern::MAX_DITHER_DENSITY,
                    );
                    let input = ui.add_sized(
                        vec2(42.0, ui.spacing().interact_size.y),
                        egui::DragValue::new(&mut density)
                            .range(
                                BrushPattern::MIN_DITHER_DENSITY..=BrushPattern::MAX_DITHER_DENSITY,
                            )
                            .suffix("%"),
                    );
                    let mut density_f = density as f32;
                    let slider_w = (ui.available_width() - 50.0).max(40.0);
                    let slider = ui
                        .allocate_ui(vec2(slider_w, 18.0), |ui| {
                            pro_slider(
                                ui,
                                "brush-pattern-density",
                                &mut density_f,
                                (BrushPattern::MIN_DITHER_DENSITY as f32)
                                    ..=(BrushPattern::MAX_DITHER_DENSITY as f32),
                                false,
                            )
                        })
                        .inner;
                    if slider.changed() {
                        density = density_f.round() as u8;
                    }
                    let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
                    if (input.changed() || slider.changed()) && density != pattern.dither_density {
                        events
                            .borrow_mut()
                            .push(ToolbarEvent::BrushPatternDitherDensityChanged(density));
                    }
                    if reset.clicked() {
                        events.borrow_mut().push(
                            ToolbarEvent::BrushPatternDitherDensityChanged(
                                BrushPattern::DEFAULT_DITHER_DENSITY,
                            ),
                        );
                    }
                });
            }
            BrushPatternKind::Line => {
                property_row(ui, "Thickness", |ui| {
                    let mut thickness = pattern.line_thickness.clamp(
                        BrushPattern::MIN_LINE_THICKNESS,
                        BrushPattern::MAX_LINE_THICKNESS,
                    );
                    let input = ui.add_sized(
                        vec2(42.0, ui.spacing().interact_size.y),
                        egui::DragValue::new(&mut thickness).range(
                            BrushPattern::MIN_LINE_THICKNESS..=BrushPattern::MAX_LINE_THICKNESS,
                        ),
                    );
                    let mut thickness_f = thickness as f32;
                    let slider_w = (ui.available_width() - 50.0).max(40.0);
                    let slider = ui
                        .allocate_ui(vec2(slider_w, 18.0), |ui| {
                            pro_slider(
                                ui,
                                "brush-pattern-thickness",
                                &mut thickness_f,
                                (BrushPattern::MIN_LINE_THICKNESS as f32)
                                    ..=(BrushPattern::MAX_LINE_THICKNESS as f32),
                                false,
                            )
                        })
                        .inner;
                    if slider.changed() {
                        thickness = thickness_f.round() as u8;
                    }
                    let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
                    if (input.changed() || slider.changed())
                        && thickness != pattern.line_thickness
                    {
                        events
                            .borrow_mut()
                            .push(ToolbarEvent::BrushPatternLineThicknessChanged(thickness));
                    }
                    if reset.clicked() {
                        events.borrow_mut().push(
                            ToolbarEvent::BrushPatternLineThicknessChanged(
                                BrushPattern::DEFAULT_LINE_THICKNESS,
                            ),
                        );
                    }
                });
                property_row(ui, "Angle", |ui| {
                    let mut angle = pattern.line_angle.clamp(
                        BrushPattern::MIN_LINE_ANGLE,
                        BrushPattern::MAX_LINE_ANGLE,
                    );
                    let input = ui.add_sized(
                        vec2(42.0, ui.spacing().interact_size.y),
                        egui::DragValue::new(&mut angle)
                            .range(BrushPattern::MIN_LINE_ANGLE..=BrushPattern::MAX_LINE_ANGLE)
                            .suffix("°"),
                    );
                    let mut angle_f = angle as f32;
                    let slider_w = (ui.available_width() - 50.0).max(40.0);
                    let slider = ui
                        .allocate_ui(vec2(slider_w, 18.0), |ui| {
                            pro_slider(
                                ui,
                                "brush-pattern-angle",
                                &mut angle_f,
                                (BrushPattern::MIN_LINE_ANGLE as f32)
                                    ..=(BrushPattern::MAX_LINE_ANGLE as f32),
                                false,
                            )
                        })
                        .inner;
                    if slider.changed() {
                        angle = angle_f.round() as i16;
                    }
                    let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
                    if (input.changed() || slider.changed()) && angle != pattern.line_angle {
                        events
                            .borrow_mut()
                            .push(ToolbarEvent::BrushPatternLineAngleChanged(angle));
                    }
                    if reset.clicked() {
                        events.borrow_mut().push(
                            ToolbarEvent::BrushPatternLineAngleChanged(
                                BrushPattern::DEFAULT_LINE_ANGLE,
                            ),
                        );
                    }
                });
                property_row(ui, "Spacing", |ui| {
                    let mut spacing = pattern.line_spacing.clamp(
                        BrushPattern::MIN_LINE_SPACING,
                        BrushPattern::MAX_LINE_SPACING,
                    );
                    let input = ui.add_sized(
                        vec2(42.0, ui.spacing().interact_size.y),
                        egui::DragValue::new(&mut spacing).range(
                            BrushPattern::MIN_LINE_SPACING..=BrushPattern::MAX_LINE_SPACING,
                        ),
                    );
                    let mut spacing_f = spacing as f32;
                    let slider_w = (ui.available_width() - 50.0).max(40.0);
                    let slider = ui
                        .allocate_ui(vec2(slider_w, 18.0), |ui| {
                            pro_slider(
                                ui,
                                "brush-pattern-spacing",
                                &mut spacing_f,
                                (BrushPattern::MIN_LINE_SPACING as f32)
                                    ..=(BrushPattern::MAX_LINE_SPACING as f32),
                                false,
                            )
                        })
                        .inner;
                    if slider.changed() {
                        spacing = spacing_f.round() as u8;
                    }
                    let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
                    if (input.changed() || slider.changed()) && spacing != pattern.line_spacing {
                        events
                            .borrow_mut()
                            .push(ToolbarEvent::BrushPatternLineSpacingChanged(spacing));
                    }
                    if reset.clicked() {
                        events.borrow_mut().push(
                            ToolbarEvent::BrushPatternLineSpacingChanged(
                                BrushPattern::DEFAULT_LINE_SPACING,
                            ),
                        );
                    }
                });
            }
        }
    }

    /// The Fieldier's properties: the child picker (Rectangle / Wand / Lasso)
    /// and, while the Wand child is active, the read-only contiguity checkbox,
    /// the wand tolerance (`[numeric input][slider][Reset]`) and the
    /// restrict-to-region checkbox.
    fn fieldier_properties(&self, ui: &mut egui::Ui, view: &ToolboxView, chrome: &PanelChrome) {
        let events = &self.host.events;
        property_row(ui, "Child", |ui| {
            let children = [
                (
                    FieldierChild::Rectangle,
                    format!("Rectangle ({})", view.shortcuts.rectangle),
                ),
                (
                    FieldierChild::Wand,
                    format!("Wand ({})", view.shortcuts.wand),
                ),
                (
                    FieldierChild::Lasso,
                    format!("Lasso ({})", view.shortcuts.lasso),
                ),
                (
                    FieldierChild::BrushSelector,
                    format!("Brush Selector ({})", view.shortcuts.brush_selector),
                ),
            ];
            let current = children
                .iter()
                .find(|(c, _)| *c == view.child)
                .map(|(_, l)| l.as_str())
                .unwrap_or("Rectangle");
            egui::ComboBox::from_id_salt("tool_prop_fieldier_child")
                .width(ui.available_width().clamp(110.0, 180.0))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (child, label) in &children {
                        let selected = view.child == *child;
                        if chrome.combo_item(ui, selected, label).clicked() && !selected {
                            events
                                .borrow_mut()
                                .push(ToolbarEvent::FieldierChildChanged(*child));
                        }
                    }
                });
        });
        if view.child == FieldierChild::BrushSelector {
            let mut size = view
                .brush_select
                .size
                .clamp(BrushSpec::MIN_SIZE, BrushSpec::MAX_SIZE);
            property_row(ui, "Size (Shift+Scroll)", |ui| {
                let input = ui.add_sized(
                    vec2(42.0, ui.spacing().interact_size.y),
                    egui::DragValue::new(&mut size)
                        .range(BrushSpec::MIN_SIZE..=BrushSpec::MAX_SIZE),
                );
                let mut size_f = size as f32;
                let slider_w = (ui.available_width() - 50.0).max(40.0);
                let slider = ui
                    .allocate_ui(vec2(slider_w, 18.0), |ui| {
                        pro_slider(
                            ui,
                            "brush-select-size",
                            &mut size_f,
                            (BrushSpec::MIN_SIZE as f32)..=(BrushSpec::MAX_SIZE as f32),
                            false,
                        )
                    })
                    .inner;
                if slider.changed() {
                    size = size_f.round() as u8;
                }
                let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
                if (input.changed() || slider.changed()) && size != view.brush_select.size {
                    events
                        .borrow_mut()
                        .push(ToolbarEvent::BrushSelectSizeChanged(size));
                }
                if reset.clicked() {
                    events
                        .borrow_mut()
                        .push(ToolbarEvent::BrushSelectSizeChanged(BrushSpec::MIN_SIZE));
                }
            });
            property_row(ui, "Shape (Ctrl+Scroll)", |ui| {
                let current = match view.brush_select.shape {
                    BrushShape::Square => "Square",
                    BrushShape::Round => "Circle",
                };
                egui::ComboBox::from_id_salt("tool_prop_brush_select_shape")
                    .width(ui.available_width().clamp(96.0, 180.0))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (shape, label) in [
                            (BrushShape::Square, "Square"),
                            (BrushShape::Round, "Circle"),
                        ] {
                            let selected = view.brush_select.shape == shape;
                            if chrome.combo_item(ui, selected, label).clicked() && !selected {
                                events
                                    .borrow_mut()
                                    .push(ToolbarEvent::BrushSelectShapeChanged(shape));
                            }
                        }
                    });
            });
            return;
        }
        if view.child != FieldierChild::Wand {
            return;
        }
        // Displays the live effective contiguity (stored XOR Alt). Clicking
        // toggles the stored `wand.contiguous` setting so the user can flip
        // the default while Alt temporarily inverts whatever state is active.
        let mut effective = view.wand_contiguous_effective;
        if ui.checkbox(&mut effective, "Contiguous (Alt)").changed() {
            events
                .borrow_mut()
                .push(ToolbarEvent::WandContiguousChanged(!view.wand.contiguous));
        }
        property_row(ui, "Tolerance", |ui| {
            let mut tolerance = view.wand.tolerance;
            let input = ui.add_sized(
                vec2(42.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut tolerance).range(0..=u8::MAX),
            );
            let mut tol_f = tolerance as f32;
            let slider_w = (ui.available_width() - 50.0).max(40.0);
            let slider = ui
                .allocate_ui(vec2(slider_w, 18.0), |ui| {
                    pro_slider(ui, "wand-tol", &mut tol_f, 0.0..=(u8::MAX as f32), false)
                })
                .inner;
            if slider.changed() {
                tolerance = tol_f.round() as u8;
            }
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

    /// The Fill (bucket) tool's properties: the per-channel tolerance
    /// (`[numeric input][slider][Reset]`), the contiguous checkbox and the
    /// restrict-to-region checkbox. All three write the Fill tool's own stored
    /// [`FillSettings`](crate::input::FillSettings).
    fn fill_properties(&self, ui: &mut egui::Ui, view: &ToolboxView, chrome: &PanelChrome) {
        let events = &self.host.events;
        property_row(ui, "Tolerance", |ui| {
            let mut tolerance = view.fill.tolerance;
            let input = ui.add_sized(
                vec2(42.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut tolerance).range(0..=u8::MAX),
            );
            let mut tol_f = tolerance as f32;
            let slider_w = (ui.available_width() - 50.0).max(40.0);
            let slider = ui
                .allocate_ui(vec2(slider_w, 18.0), |ui| {
                    pro_slider(ui, "fill-tol", &mut tol_f, 0.0..=(u8::MAX as f32), false)
                })
                .inner;
            if slider.changed() {
                tolerance = tol_f.round() as u8;
            }
            let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
            if (input.changed() || slider.changed()) && tolerance != view.fill.tolerance {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::FillToleranceChanged(tolerance));
            }
            if reset.clicked() {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::FillToleranceChanged(0));
            }
        });
        // Displays the live effective contiguity (stored XOR Alt). Clicking
        // toggles the stored `fill.contiguous` setting while Alt temporarily
        // inverts the active state.
        let mut effective = view.fill_contiguous_effective;
        if ui.checkbox(&mut effective, "Contiguous (Alt)").changed() {
            events
                .borrow_mut()
                .push(ToolbarEvent::FillContiguousChanged(!view.fill.contiguous));
        }
        let mut restrict = view.fill.restrict_to_region;
        if ui.checkbox(&mut restrict, "Restrict to region").changed() {
            events
                .borrow_mut()
                .push(ToolbarEvent::FillRestrictToRegionChanged(restrict));
        }
    }

    fn text_properties(&self, ui: &mut egui::Ui, view: &ToolboxView, chrome: &PanelChrome) {
        let events = &self.host.events;
        property_row(ui, "Font", |ui| {
            let mut selected = view.text_font.clone();
            egui::ComboBox::from_id_salt("text-tool-font")
                .width((ui.available_width() - 8.0).max(120.0))
                .selected_text(&selected)
                .show_ui(ui, |ui| {
                    for font_name in &view.available_fonts {
                        if ui
                            .selectable_value(&mut selected, font_name.clone(), font_name)
                            .changed()
                        {
                            events
                                .borrow_mut()
                                .push(ToolbarEvent::TextFontChanged(selected.clone()));
                        }
                    }
                });
        });
        ui.add_space(4.0);
        property_row(ui, "Size", |ui| {
            let mut size = view.text_size;
            let input = ui.add_sized(
                vec2(42.0, ui.spacing().interact_size.y),
                egui::DragValue::new(&mut size).range(5..=128),
            );
            let mut size_f = size as f32;
            let slider_w = (ui.available_width() - 50.0).max(40.0);
            let slider = ui
                .allocate_ui(vec2(slider_w, 18.0), |ui| {
                    pro_slider(ui, "text-size", &mut size_f, 5.0..=128.0, false)
                })
                .inner;
            if slider.changed() {
                size = size_f.round() as u32;
            }
            let reset = chrome.button(ui, RESET_LABEL, ButtonStyle::plain());
            if (input.changed() || slider.changed()) && size != view.text_size {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::TextSizeChanged(size));
            }
            if reset.clicked() && view.text_size != 14 {
                events
                    .borrow_mut()
                    .push(ToolbarEvent::TextSizeChanged(14));
            }
        });
        property_row(ui, "Align", |ui| {
            let total_w = ui.available_width().max(72.0);
            let gap = 6.0;
            let btn_w = ((total_w - 2.0 * gap) / 3.0).max(20.0);
            let btn_h = ui.spacing().interact_size.y.max(22.0);
            let (row_rect, _) =
                ui.allocate_exact_size(vec2(total_w, btn_h), egui::Sense::hover());
            for (i, (align, glyph, tip)) in [
                (crate::core::text::TextAlign::Left, crate::ui::icons::ALIGN_LEFT, "Align Left"),
                (crate::core::text::TextAlign::Center, crate::ui::icons::ALIGN_CENTER, "Align Center"),
                (crate::core::text::TextAlign::Right, crate::ui::icons::ALIGN_RIGHT, "Align Right"),
            ]
            .into_iter()
            .enumerate()
            {
                let x = row_rect.left() + i as f32 * (btn_w + gap);
                let slot = egui::Rect::from_min_size(egui::pos2(x, row_rect.top()), vec2(btn_w, btn_h));
                let selected = view.text_align == align;
                let response = ui
                    .interact(
                        slot,
                        ui.id().with(("text-align-btn", i)),
                        egui::Sense::click(),
                    )
                    .on_hover_text(tip);
                let visuals = ui.visuals();
                let fill = if selected {
                    chrome.colors.selection_bg_fill32()
                } else if response.hovered() {
                    visuals.widgets.hovered.bg_fill
                } else {
                    chrome.colors.panel_bg32()
                };
                let stroke = if selected {
                    egui::Stroke::new(1.0, chrome.colors.selection_stroke_color32())
                } else {
                    egui::Stroke::new(1.0, chrome.colors.panel_border32())
                };
                ui.painter().rect_filled(slot, chrome.rounding, fill);
                ui.painter().rect_stroke(
                    slot,
                    chrome.rounding,
                    stroke,
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    slot.center(),
                    egui::Align2::CENTER_CENTER,
                    glyph,
                    egui::FontId::proportional(14.0),
                    visuals.text_color(),
                );
                if response.clicked() && !selected {
                    events
                        .borrow_mut()
                        .push(ToolbarEvent::TextAlignChanged(align));
                }
            }
        });
        self.transform_properties(ui, view, chrome);
    }

    /// The Tile (placer) tool's properties: the placement transform that BOTH the
    /// Q/R/X/Z shortcuts and these controls drive.
    ///
    /// The widgets are a pure VIEW over [`ToolboxView::tile_placer`], which the
    /// App mirrors from the single `tile_placer_transform` — so there is one
    /// source of truth and a key press is immediately visible here (and a panel
    /// edit immediately changes what the next stamp writes).
    ///
    /// Rotation is a plain numeric input in DEGREES, but the value is snapped to
    /// the quarter-turn grid (see [`snap_rotation_degrees`]) before it is emitted,
    /// so the box only ever displays 0 / 90 / 180 / 270 no matter what is typed.
    /// The two flip checkboxes are stacked vertically, horizontal above vertical.
    fn tile_properties(&self, ui: &mut egui::Ui, view: &ToolboxView) {
        let events = &self.host.events;
        property_row(ui, "Rotation (Q / R)", |ui| {
            let mut degrees = view.tile_placer.rotation;
            let input = ui.add(
                egui::DragValue::new(&mut degrees)
                    .range(ROTATION_INPUT_MIN..=ROTATION_INPUT_MAX)
                    .speed(1.0),
            );
            if input.changed() {
                // Snap BEFORE emitting: the event carries an already-normalized
                // 90 multiple, so the App never has to re-derive it and the
                // typed value can never escape the quarter-turn grid.
                let snapped = snap_rotation_degrees(degrees);
                if snapped != view.tile_placer.rotation {
                    events
                        .borrow_mut()
                        .push(ToolbarEvent::TileRotationChanged(snapped));
                }
            }
        });
        let mut flip_x = view.tile_placer.flip_x;
        if ui.checkbox(&mut flip_x, "horizontal flip (X)").changed() {
            events
                .borrow_mut()
                .push(ToolbarEvent::TileFlipXChanged(flip_x));
        }
        let mut flip_y = view.tile_placer.flip_y;
        if ui.checkbox(&mut flip_y, "vertical flip (Z)").changed() {
            events
                .borrow_mut()
                .push(ToolbarEvent::TileFlipYChanged(flip_y));
        }
        let mut tile_eraser = view.tile_placer.tile_eraser;
        if ui.checkbox(&mut tile_eraser, "tile eraser (Alt)").changed() {
            events
                .borrow_mut()
                .push(ToolbarEvent::TileEraserChanged(tile_eraser));
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
                (TransformAlgorithm::PyxRot, "PyxRot"),
            ];
            let current = algorithms
                .iter()
                .find(|(a, _)| *a == view.transform_algorithm)
                .map(|(_, l)| *l)
                .unwrap_or("RotSprite");
            egui::ComboBox::from_id_salt("tool_prop_transform_algorithm")
                .width(ui.available_width().clamp(110.0, 180.0))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (algorithm, label) in algorithms {
                        let selected = view.transform_algorithm == algorithm;
                        if chrome.combo_item(ui, selected, label).clicked() && !selected {
                            events
                                .borrow_mut()
                                .push(ToolbarEvent::TransformAlgorithmChanged(algorithm));
                        }
                    }
                });
        });
        ui.add_space(4.0);
        ui.weak(format!(
            "Rotate ({} / {}) · Scale ({} / {}) · Flip ({} / {}) · Cancel ({})",
            view.shortcuts.transform_rotate_cw,
            view.shortcuts.transform_rotate_ccw,
            view.shortcuts.transform_grow,
            view.shortcuts.transform_shrink,
            view.shortcuts.flip_h,
            view.shortcuts.flip_v,
            view.shortcuts.cancel_transform,
        ));
    }
}

/// Snap an arbitrary typed rotation to the Tile tool's quarter-turn grid and
/// normalize it into `0..360`, so the input can only ever hold 0 / 90 / 180 /
/// 270.
///
/// Rounding is to the NEAREST multiple of 90 (halfway values round away from
/// zero), and the result wraps: `45 → 90`, `100 → 90`, `91 → 90`, `135 → 180`,
/// `360 → 0`, `-45 → 270`, `-450 → 270`.
fn snap_rotation_degrees(degrees: i32) -> i32 {
    let quarters = (f64::from(degrees) / 90.0).round() as i64;
    ((quarters.rem_euclid(4)) * 90) as i32
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
    fn rotation_snaps_to_the_nearest_quarter_turn() {
        assert_eq!(snap_rotation_degrees(0), 0);
        assert_eq!(snap_rotation_degrees(90), 90);
        assert_eq!(snap_rotation_degrees(180), 180);
        assert_eq!(snap_rotation_degrees(270), 270);
        assert_eq!(snap_rotation_degrees(45), 90, "halfway rounds away from zero");
        assert_eq!(snap_rotation_degrees(135), 180);
        assert_eq!(snap_rotation_degrees(100), 90, "nearest, not rounded up");
        assert_eq!(snap_rotation_degrees(91), 90);
        assert_eq!(snap_rotation_degrees(179), 180);
        assert_eq!(snap_rotation_degrees(181), 180);
        assert_eq!(snap_rotation_degrees(226), 270);
        assert_eq!(snap_rotation_degrees(1), 0);
        assert_eq!(snap_rotation_degrees(359), 0);
    }

    #[test]
    fn rotation_normalizes_into_zero_to_three_hundred_sixty() {
        assert_eq!(snap_rotation_degrees(360), 0, "a full turn is no rotation");
        assert_eq!(snap_rotation_degrees(450), 90);
        assert_eq!(snap_rotation_degrees(-90), 270);
        assert_eq!(snap_rotation_degrees(-45), 270);
        assert_eq!(snap_rotation_degrees(-100), 270);
        assert_eq!(snap_rotation_degrees(-180), 180);
        assert_eq!(snap_rotation_degrees(-450), 270);
        for degrees in ROTATION_INPUT_MIN..=ROTATION_INPUT_MAX {
            let snapped = snap_rotation_degrees(degrees);
            assert!(
                (0..360).contains(&snapped) && snapped % 90 == 0,
                "{degrees} snapped to {snapped}, which is not a 90 multiple in 0..360"
            );
        }
    }

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
