use crate::input::{Action, KeyBinding, Keymap};
use crate::ui::theme::ThemeColors;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindingsEvent {
    Reset,
    Apply(Keymap),
}

pub struct KeybindingsPanel {
    draft: Keymap,
    error: Option<String>,
}

impl KeybindingsPanel {
    pub fn new(keymap: &Keymap) -> Self {
        Self {
            draft: keymap.clone(),
            error: None,
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        events: &mut Vec<KeybindingsEvent>,
    ) {
        ui.strong("Keybindings");
        for action in Action::ALL {
            let Some(mut binding) = self.draft.binding(action) else {
                continue;
            };
            let original = binding;
            ui.horizontal(|ui| {
                ui.label(action_label(action));
                egui::ComboBox::from_id_salt(("keybinding-key", action))
                    .selected_text(key_label(binding.key))
                    .show_ui(ui, |ui| {
                        for key in logical_keys() {
                            ui.selectable_value(&mut binding.key, key, key_label(key));
                        }
                    });
                for (label, modifier) in [
                    ("Cmd", &mut binding.modifiers.command),
                    ("Ctrl", &mut binding.modifiers.ctrl),
                    ("Alt", &mut binding.modifiers.alt),
                    ("Shift", &mut binding.modifiers.shift),
                ] {
                    ui.checkbox(modifier, label);
                }
            });
            if binding != original {
                self.draft.bindings.insert(action, binding);
            }
        }
        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                self.reset(events);
            }
            if ui.button("Apply").clicked() {
                match self.draft.validate() {
                    Ok(()) => {
                        self.error = None;
                        events.push(KeybindingsEvent::Apply(self.draft.clone()));
                    }
                    Err(error) => self.error = Some(format!("{error:?}")),
                }
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(theme.selection_stroke32(), error);
        }
    }

    pub fn draft_mut(&mut self) -> &mut Keymap {
        &mut self.draft
    }

    pub fn draft(&self) -> &Keymap {
        &self.draft
    }

    pub fn reset(&mut self, events: &mut Vec<KeybindingsEvent>) {
        self.draft = Keymap::defaults();
        self.error = None;
        events.push(KeybindingsEvent::Reset);
    }
}

fn action_label(action: Action) -> &'static str {
    match action {
        Action::Undo => "Undo",
        Action::Redo => "Redo",
        Action::Copy => "Copy",
        Action::Cut => "Cut",
        Action::Paste => "Paste",
        Action::SelectDelete => "Clear selection pixels",
        Action::InvertSelection => "Invert selection",
        Action::FlipSelectionHorizontal => "Flip selection H",
        Action::FlipSelectionVertical => "Flip selection V",
        Action::RotateSelection => "Rotate selection",
        Action::TransformRotateCw => "Rotate CW",
        Action::TransformRotateCcw => "Rotate CCW",
        Action::TransformGrow => "Grow",
        Action::TransformShrink => "Shrink",
        Action::FlipHorizontal => "Flip horizontal",
        Action::FlipVertical => "Flip vertical",
        Action::CancelTransform => "Cancel transform",
        Action::ToggleGrid => "Toggle grid",
        Action::SelectPen => "Select pen",
        Action::SelectEraser => "Select eraser",
        Action::ToggleColorPicker => "Toggle color picker",
        // UX item 2 conflict: `x` doubles as the Tile-placer's horizontal-flip
        // modifier. The App resolves it by tool context — while the Tile tool
        // is active the placer reads X directly and this shortcut is
        // suppressed; outside the Tile tool X still swaps colors.
        Action::SwapColors => "Swap colors",
        Action::SelectRectangle => "Select rectangle",
        Action::SelectWand => "Select wand",
        Action::SelectLasso => "Select lasso",
        Action::SelectFill => "Select fill",
        Action::SelectTileTool => "Select tile tool",
    }
}

fn binding_label(binding: Option<KeyBinding>) -> String {
    let Some(binding) = binding else {
        return "Unbound".to_string();
    };
    let mut parts = Vec::new();
    if binding.modifiers.command {
        parts.push("Cmd");
    }
    if binding.modifiers.ctrl {
        parts.push("Ctrl");
    }
    if binding.modifiers.alt {
        parts.push("Alt");
    }
    if binding.modifiers.shift {
        parts.push("Shift");
    }
    parts.push(key_label(binding.key));
    parts.join("+")
}

fn logical_keys() -> [crate::input::LogicalKey; 22] {
    [
        crate::input::LogicalKey::Z,
        crate::input::LogicalKey::Y,
        crate::input::LogicalKey::C,
        crate::input::LogicalKey::X,
        crate::input::LogicalKey::V,
        crate::input::LogicalKey::R,
        crate::input::LogicalKey::Plus,
        crate::input::LogicalKey::Equals,
        crate::input::LogicalKey::Minus,
        crate::input::LogicalKey::H,
        crate::input::LogicalKey::Escape,
        crate::input::LogicalKey::G,
        crate::input::LogicalKey::D,
        crate::input::LogicalKey::E,
        crate::input::LogicalKey::Delete,
        crate::input::LogicalKey::I,
        crate::input::LogicalKey::F,
        crate::input::LogicalKey::S,
        crate::input::LogicalKey::W,
        crate::input::LogicalKey::L,
        crate::input::LogicalKey::B,
        crate::input::LogicalKey::T,
    ]
}

fn key_label(key: crate::input::LogicalKey) -> &'static str {
    match key {
        crate::input::LogicalKey::Z => "Z",
        crate::input::LogicalKey::Y => "Y",
        crate::input::LogicalKey::C => "C",
        crate::input::LogicalKey::X => "X",
        crate::input::LogicalKey::V => "V",
        crate::input::LogicalKey::R => "R",
        crate::input::LogicalKey::Plus => "+",
        crate::input::LogicalKey::Equals => "=",
        crate::input::LogicalKey::Minus => "-",
        crate::input::LogicalKey::H => "H",
        crate::input::LogicalKey::Escape => "Esc",
        crate::input::LogicalKey::G => "G",
        crate::input::LogicalKey::D => "D",
        crate::input::LogicalKey::E => "E",
        crate::input::LogicalKey::Delete => "Del",
        crate::input::LogicalKey::I => "I",
        crate::input::LogicalKey::F => "F",
        crate::input::LogicalKey::S => "S",
        crate::input::LogicalKey::W => "W",
        crate::input::LogicalKey::L => "L",
        crate::input::LogicalKey::B => "B",
        crate::input::LogicalKey::T => "T",
    }
}

impl Default for KeybindingsPanel {
    fn default() -> Self {
        Self::new(&Keymap::defaults())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;

    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        panel: &mut KeybindingsPanel,
        output_events: &mut Vec<KeybindingsEvent>,
    ) -> egui::FullOutput {
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                // Tall enough for every action row plus the Reset/Apply
                // buttons (the panel grew when tools were added).
                egui::vec2(800.0, 1400.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = Theme::default_dark().colors;
        let mut output = ctx.run_ui(raw_input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| panel.ui(ui, &theme, output_events));
        });
        output.textures_delta.clear();
        output
    }

    fn text_position(output: &egui::FullOutput, expected: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == expected => Some(text.pos),
                _ => None,
            })
            .expect("expected text should be rendered")
    }

    #[test]
    fn panel_starts_with_all_actions() {
        let panel = KeybindingsPanel::default();
        assert_eq!(panel.draft.bindings.len(), Action::ALL.len());
    }

    #[test]
    fn binding_display_is_stable() {
        assert_eq!(
            binding_label(Keymap::defaults().binding(Action::Undo)),
            "Cmd+Z"
        );
    }

    #[test]
    fn panel_renders_edit_controls_for_key_and_modifiers() {
        let mut panel = KeybindingsPanel::default();
        let mut events = Vec::new();
        let ctx = egui::Context::default();
        let theme = Theme::default_dark().colors;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| panel.ui(ui, &theme, &mut events));
        });
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(text.iter().any(|value| value == "Cmd"));
        assert!(text.iter().any(|value| value == "Shift"));
        assert!(text.iter().any(|value| value == "Toggle grid"));
        output.textures_delta.clear();
    }

    #[test]
    fn apply_rejects_duplicate_bindings() {
        let mut panel = KeybindingsPanel::default();
        let binding = panel.draft.binding(Action::Undo).unwrap();
        panel.draft.bindings.insert(Action::Redo, binding);
        let mut events = Vec::new();
        let ctx = egui::Context::default();
        let output = run_frame(&ctx, Vec::new(), &mut panel, &mut events);
        let apply = text_position(&output, "Apply") + egui::vec2(4.0, 8.0);
        run_frame(
            &ctx,
            vec![egui::Event::PointerMoved(apply)],
            &mut panel,
            &mut events,
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: apply,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut panel,
            &mut events,
        );
        run_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: apply,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &mut panel,
            &mut events,
        );
        assert!(panel.error.is_some());
        assert!(events.is_empty());
    }

    #[test]
    fn reset_replaces_stale_draft_before_apply() {
        let mut panel = KeybindingsPanel::default();
        panel.draft_mut().bindings.insert(
            Action::Undo,
            KeyBinding {
                key: crate::input::LogicalKey::G,
                modifiers: crate::input::Modifiers::default(),
            },
        );
        let mut events = Vec::new();
        panel.reset(&mut events);
        assert_eq!(
            panel.draft().binding(Action::Undo),
            Keymap::defaults().binding(Action::Undo)
        );
        assert_eq!(events, vec![KeybindingsEvent::Reset]);
    }
}
