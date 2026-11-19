use crate::input::{Action, KeyBinding, Keymap};
use crate::ui::theme::ThemeColors;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindingsEvent {
    Reset,
    Apply(Keymap),
}

pub struct KeybindingsPanel {
    committed: Keymap,
    draft: Keymap,
    error: Option<String>,
}

impl KeybindingsPanel {
    pub fn new(keymap: &Keymap) -> Self {
        Self {
            committed: keymap.clone(),
            draft: keymap.clone(),
            error: None,
        }
    }

    /// Returns `true` when the user has modified bindings or reset to defaults
    /// without applying yet.
    pub fn is_dirty(&self) -> bool {
        self.draft != self.committed
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        events: &mut Vec<KeybindingsEvent>,
    ) {
        self.ui_rows(ui, theme);
        self.ui_footer(ui, theme, events);
    }

    /// Renders the scrollable list of keybinding rows with alternating
    /// accordion/zebra row backgrounds.
    pub fn ui_rows(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.strong("Keybindings");
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut row_idx = 0usize;
        for action in Action::ALL {
            let Some(mut binding) = self.draft.binding(action) else {
                continue;
            };
            let original = binding;
            let row_bg = if row_idx % 2 == 0 {
                theme.list_bg32()
            } else {
                theme.list_stripe_bg32()
            };
            row_idx += 1;
            egui::Frame::NONE
                .fill(row_bg)
                .inner_margin(egui::Margin {
                    left: 6,
                    right: 18,
                    top: 3,
                    bottom: 3,
                })
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.set_width(ui.available_width());
                        ui.label(action_label(action));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            for (label, modifier) in [
                                ("Shift", &mut binding.modifiers.shift),
                                ("Alt", &mut binding.modifiers.alt),
                                ("Ctrl", &mut binding.modifiers.ctrl),
                                ("Cmd", &mut binding.modifiers.command),
                            ] {
                                ui.checkbox(modifier, label);
                            }
                            egui::ComboBox::from_id_salt(("keybinding-key", action))
                                .width(54.0)
                                .selected_text(key_label(binding.key))
                                .show_ui(ui, |ui| {
                                    for key in logical_keys() {
                                        ui.selectable_value(&mut binding.key, key, key_label(key));
                                    }
                                });
                        });
                    });
                });
            if binding != original {
                self.draft.bindings.insert(action, binding);
            }
        }
    }

    /// Renders the static bottom bar containing `Reset` and `Apply`.
    pub fn ui_footer(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        events: &mut Vec<KeybindingsEvent>,
    ) {
        egui::Frame::NONE
            .fill(theme.panel_header_bg32())
            .stroke(egui::Stroke::new(1.0, theme.panel_border32()))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    if ui.button("Reset").clicked() {
                        self.reset(events);
                    }
                    if ui.button("Apply").clicked() {
                        self.try_apply(events);
                    }
                    if let Some(error) = &self.error {
                        ui.colored_label(theme.selection_stroke32(), error);
                    }
                });
            });
    }

    /// Validates the draft keymap and emits [`KeybindingsEvent::Apply`] if valid.
    pub fn try_apply(&mut self, events: &mut Vec<KeybindingsEvent>) -> bool {
        match self.draft.validate() {
            Ok(()) => {
                self.committed = self.draft.clone();
                self.error = None;
                events.push(KeybindingsEvent::Apply(self.draft.clone()));
                true
            }
            Err(error) => {
                self.error = Some(format!("{error:?}"));
                false
            }
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
        Action::SelectAnimationPlacer => "Select animation placer (A)",
        Action::SelectText => "Select text",
        Action::SaveProject => "Save project",
        Action::SaveProjectAs => "Save project as",
        Action::OpenProject => "Open project",
    }
}

pub(crate) fn binding_label(binding: Option<KeyBinding>) -> String {
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

fn logical_keys() -> [crate::input::LogicalKey; 25] {
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
        crate::input::LogicalKey::A,
        crate::input::LogicalKey::O,
        crate::input::LogicalKey::Q,
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
        crate::input::LogicalKey::A => "A",
        crate::input::LogicalKey::O => "O",
        crate::input::LogicalKey::Q => "Q",
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
                key: crate::input::LogicalKey::B,
                modifiers: crate::input::Modifiers {
                    ctrl: true,
                    shift: true,
                    ..crate::input::Modifiers::default()
                },
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

    #[test]
    fn dirty_tracks_unapplied_edits_until_apply() {
        let mut panel = KeybindingsPanel::default();
        assert!(!panel.is_dirty());
        panel.draft_mut().bindings.insert(
            Action::Undo,
            KeyBinding {
                key: crate::input::LogicalKey::B,
                modifiers: crate::input::Modifiers {
                    ctrl: true,
                    shift: true,
                    ..crate::input::Modifiers::default()
                },
            },
        );
        assert!(panel.is_dirty());
        let mut events = Vec::new();
        assert!(panel.try_apply(&mut events));
        assert!(!panel.is_dirty());
        assert_eq!(events.len(), 1);
    }
}
