use crate::input::{Action, KeyBinding, Keymap, LogicalKey, Modifiers};
use crate::ui::theme::ThemeColors;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeybindingsEvent {
    Reset,
    Apply(Keymap),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeybindingCategory {
    #[default]
    All,
    Tools,
    Edit,
    Transform,
    File,
    ImportExport,
    View,
}

impl KeybindingCategory {
    pub const TABS: [Self; 7] = [
        Self::All,
        Self::Tools,
        Self::Edit,
        Self::Transform,
        Self::File,
        Self::ImportExport,
        Self::View,
    ];

    pub const SECTIONS: [Self; 6] = [
        Self::Tools,
        Self::Edit,
        Self::Transform,
        Self::File,
        Self::ImportExport,
        Self::View,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Tools => "Tools",
            Self::Edit => "Edit",
            Self::Transform => "Transform",
            Self::File => "File",
            Self::ImportExport => "Import / Export",
            Self::View => "View",
        }
    }
}

fn action_category(action: Action) -> KeybindingCategory {
    match action {
        Action::SelectPen
        | Action::SelectEraser
        | Action::SelectFill
        | Action::SelectRectangle
        | Action::SelectWand
        | Action::SelectLasso
        | Action::SelectBrushSelector
        | Action::SelectTileTool
        | Action::SelectAnimationPlacer
        | Action::SelectText
        | Action::ToggleColorPicker
        | Action::SwapColors => KeybindingCategory::Tools,

        Action::Undo
        | Action::Redo
        | Action::Copy
        | Action::Cut
        | Action::Paste
        | Action::SelectDelete
        | Action::InvertSelection
        | Action::FlipSelectionHorizontal
        | Action::FlipSelectionVertical
        | Action::RotateSelection => KeybindingCategory::Edit,

        Action::TransformRotateCw
        | Action::TransformRotateCcw
        | Action::TransformGrow
        | Action::TransformShrink
        | Action::FlipHorizontal
        | Action::FlipVertical
        | Action::CancelTransform => KeybindingCategory::Transform,

        Action::SaveProject
        | Action::SaveProjectAs
        | Action::OpenProject
        | Action::ResizeCanvas => KeybindingCategory::File,

        Action::ImportImages
        | Action::ImportGif
        | Action::ImportPyxel
        | Action::ImportAseprite
        | Action::ImportImage2Pixel
        | Action::ExportImage
        | Action::ExportAnimation
        | Action::ExportPyxel
        | Action::ExportAseprite
        | Action::ExportSelected => KeybindingCategory::ImportExport,

        Action::ToggleGrid => KeybindingCategory::View,
    }
}

fn egui_key_to_logical(key: egui::Key) -> Option<LogicalKey> {
    match key {
        egui::Key::A => Some(LogicalKey::A),
        egui::Key::B => Some(LogicalKey::B),
        egui::Key::C => Some(LogicalKey::C),
        egui::Key::D => Some(LogicalKey::D),
        egui::Key::E => Some(LogicalKey::E),
        egui::Key::F => Some(LogicalKey::F),
        egui::Key::G => Some(LogicalKey::G),
        egui::Key::H => Some(LogicalKey::H),
        egui::Key::I => Some(LogicalKey::I),
        egui::Key::L => Some(LogicalKey::L),
        egui::Key::O => Some(LogicalKey::O),
        egui::Key::P => Some(LogicalKey::P),
        egui::Key::Q => Some(LogicalKey::Q),
        egui::Key::R => Some(LogicalKey::R),
        egui::Key::S => Some(LogicalKey::S),
        egui::Key::T => Some(LogicalKey::T),
        egui::Key::V => Some(LogicalKey::V),
        egui::Key::W => Some(LogicalKey::W),
        egui::Key::X => Some(LogicalKey::X),
        egui::Key::Y => Some(LogicalKey::Y),
        egui::Key::Z => Some(LogicalKey::Z),
        egui::Key::Plus => Some(LogicalKey::Plus),
        egui::Key::Equals => Some(LogicalKey::Equals),
        egui::Key::Minus => Some(LogicalKey::Minus),
        egui::Key::Delete | egui::Key::Backspace => Some(LogicalKey::Delete),
        egui::Key::Escape => Some(LogicalKey::Escape),
        _ => None,
    }
}

pub struct KeybindingsPanel {
    committed: Keymap,
    draft: Keymap,
    error: Option<String>,
    search: String,
    category: KeybindingCategory,
    recording: Option<Action>,
}

impl KeybindingsPanel {
    pub fn new(keymap: &Keymap) -> Self {
        Self {
            committed: keymap.clone(),
            draft: keymap.clone(),
            error: None,
            search: String::new(),
            category: KeybindingCategory::All,
            recording: None,
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
        self.ui_toolbar(ui, theme);
        self.ui_rows(ui, theme);
        self.ui_footer(ui, theme, events);
    }

    /// Pinned top toolbar: live search box + category filter tabs + column header.
    pub fn ui_toolbar(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        // Handle live key-capture if recording a shortcut.
        if let Some(target_action) = self.recording {
            let captured = ui.input(|i| {
                for ev in &i.events {
                    if let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = ev
                    {
                        if *key == egui::Key::Escape && !modifiers.any() {
                            return Some(None);
                        }
                        if let Some(logical) = egui_key_to_logical(*key) {
                            return Some(Some(KeyBinding {
                                key: logical,
                                modifiers: Modifiers {
                                    command: modifiers.command,
                                    ctrl: modifiers.ctrl && !modifiers.command,
                                    alt: modifiers.alt,
                                    shift: modifiers.shift,
                                },
                            }));
                        }
                    }
                }
                None
            });
            if let Some(result) = captured {
                if let Some(new_binding) = result {
                    self.draft.bindings.insert(target_action, new_binding);
                    self.error = None;
                }
                self.recording = None;
            }
        }

        egui::Frame::NONE
            .fill(theme.panel_bg32())
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 6,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let clear_w = if self.search.is_empty() { 0.0 } else { 26.0 };
                    let edit_w = (ui.available_width() - clear_w - 4.0).max(100.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("Search action or shortcut...")
                            .desired_width(edit_w),
                    );
                    if !self.search.is_empty()
                        && ui
                            .add_sized(egui::vec2(22.0, 20.0), egui::Button::new("×"))
                            .on_hover_text("Clear search")
                            .clicked()
                    {
                        self.search.clear();
                    }
                });
                ui.add_space(5.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                    for cat in KeybindingCategory::TABS {
                        let selected = self.category == cat;
                        let resp = ui.selectable_label(selected, cat.label());
                        if resp.clicked() {
                            self.category = cat;
                        }
                    }
                });
            });

        // Column header band.
        egui::Frame::NONE
            .fill(theme.panel_header_bg32())
            .stroke(egui::Stroke::new(1.0, theme.panel_border32()))
            .inner_margin(egui::Margin {
                left: 10,
                right: 22,
                top: 4,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.weak("Action");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak("Shortcut (click to bind)");
                    });
                });
            });
    }

    /// Renders the scrollable categorized list of keybinding rows.
    pub fn ui_rows(&mut self, ui: &mut egui::Ui, theme: &ThemeColors) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let query = self.search.trim().to_lowercase();

        // Precompute duplicate bindings so conflicts are highlighted live.
        let mut counts = std::collections::HashMap::<KeyBinding, usize>::new();
        for action in Action::ALL {
            if let Some(b) = self.draft.binding(action) {
                *counts.entry(b).or_insert(0) += 1;
            }
        }

        let sections: Vec<KeybindingCategory> = if self.category == KeybindingCategory::All {
            KeybindingCategory::SECTIONS.to_vec()
        } else {
            vec![self.category]
        };

        let mut any_shown = false;
        for section in sections {
            let matching: Vec<Action> = Action::ALL
                .iter()
                .copied()
                .filter(|&action| action_category(action) == section)
                .filter(|&action| {
                    if query.is_empty() {
                        return true;
                    }
                    let label = action_label(action).to_lowercase();
                    let shortcut = self
                        .draft
                        .binding(action)
                        .map(|b| b.display_label().to_lowercase())
                        .unwrap_or_default();
                    label.contains(&query) || shortcut.contains(&query)
                })
                .collect();

            if matching.is_empty() {
                continue;
            }
            any_shown = true;

            // Section header when viewing All categories.
            if self.category == KeybindingCategory::All {
                egui::Frame::NONE
                    .fill(theme.panel_bg32())
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 18,
                        top: 7,
                        bottom: 3,
                    })
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.label(
                            egui::RichText::new(section.label().to_uppercase())
                                .small()
                                .strong()
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
            }

            for (row_idx, action) in matching.into_iter().enumerate() {
                let Some(mut binding) = self.draft.binding(action) else {
                    continue;
                };
                let original = binding;
                let is_recording = self.recording == Some(action);
                let is_conflict = counts.get(&binding).copied().unwrap_or(0) > 1;
                let is_modified = self.committed.binding(action) != Some(binding);
                let row_bg = if is_recording {
                    theme.selection_bg_fill32().gamma_multiply(0.55)
                } else if row_idx % 2 == 0 {
                    theme.list_bg32()
                } else {
                    theme.list_stripe_bg32()
                };

                egui::Frame::NONE
                    .fill(row_bg)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 18,
                        top: 4,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.set_width(ui.available_width());
                            let name_text = if is_modified {
                                egui::RichText::new(format!("{} •", action_label(action)))
                                    .color(theme.selection_stroke_color32())
                            } else {
                                egui::RichText::new(action_label(action))
                            };
                            ui.label(name_text);

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 5.0;

                                    // Compact manual editor popup (key + modifiers).
                                    let popup_id = ui.id().with(("kb-edit-pop", action));
                                    let edit_btn = ui
                                        .add_sized(egui::vec2(22.0, 22.0), egui::Button::new("▾"))
                                        .on_hover_text("Edit key and modifiers");
                                    if edit_btn.clicked() {
                                        egui::Popup::toggle_id(ui.ctx(), popup_id);
                                    }
                                    egui::Popup::from_toggle_button_response(&edit_btn)
                                        .id(popup_id)
                                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                        .show(|ui| {
                                            ui.set_min_width(170.0);
                                            ui.horizontal(|ui| {
                                                ui.label("Key:");
                                                egui::ComboBox::from_id_salt((
                                                    "keybinding-key",
                                                    action,
                                                ))
                                                .width(64.0)
                                                .selected_text(key_label(binding.key))
                                                .show_ui(ui, |ui| {
                                                    for key in logical_keys() {
                                                        ui.selectable_value(
                                                            &mut binding.key,
                                                            key,
                                                            key_label(key),
                                                        );
                                                    }
                                                });
                                            });
                                            ui.separator();
                                            ui.horizontal(|ui| {
                                                let mut ctrl_cmd = binding.modifiers.command
                                                    || binding.modifiers.ctrl;
                                                if ui.checkbox(&mut ctrl_cmd, "Ctrl").changed() {
                                                    binding.modifiers.command = ctrl_cmd;
                                                    binding.modifiers.ctrl = false;
                                                }
                                                ui.checkbox(&mut binding.modifiers.shift, "Shift");
                                                ui.checkbox(&mut binding.modifiers.alt, "Alt");
                                            });
                                        });

                                    // Clickable keycap badge: click to record a new shortcut.
                                    let badge_text = if is_recording {
                                        "Press key... (Esc)".to_owned()
                                    } else {
                                        binding.display_label()
                                    };
                                    let badge_fill = if is_recording {
                                        theme.selection_bg_fill32()
                                    } else {
                                        theme.panel_header_bg32()
                                    };
                                    let badge_stroke = if is_recording || is_conflict {
                                        egui::Stroke::new(1.5, theme.selection_stroke_color32())
                                    } else {
                                        egui::Stroke::new(1.0, theme.panel_border32())
                                    };
                                    let badge_btn = egui::Button::new(
                                        egui::RichText::new(badge_text).monospace().small(),
                                    )
                                    .fill(badge_fill)
                                    .stroke(badge_stroke)
                                    .min_size(egui::vec2(104.0, 22.0));
                                    let resp = ui.add(badge_btn).on_hover_text(if is_conflict {
                                        "Conflict: duplicate shortcut! Click to rebind."
                                    } else {
                                        "Click and press any key combination to rebind"
                                    });
                                    if resp.clicked() {
                                        self.recording = if is_recording {
                                            None
                                        } else {
                                            Some(action)
                                        };
                                    }

                                    if is_conflict {
                                        ui.label(
                                            egui::RichText::new("!")
                                                .strong()
                                                .color(theme.selection_stroke_color32()),
                                        )
                                        .on_hover_text("Duplicate keybinding conflict");
                                    }
                                },
                            );
                        });
                    });

                if binding != original {
                    self.draft.bindings.insert(action, binding);
                    self.error = None;
                }
            }
        }

        if !any_shown {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.weak("No matching key bindings.");
            });
        }
    }

    /// Renders the static bottom bar containing `Reset`, status/error, and `Apply`.
    pub fn ui_footer(
        &mut self,
        ui: &mut egui::Ui,
        theme: &ThemeColors,
        events: &mut Vec<KeybindingsEvent>,
    ) {
        let dirty = self.is_dirty();
        egui::Frame::NONE
            .fill(theme.panel_header_bg32())
            .stroke(egui::Stroke::new(1.0, theme.panel_border32()))
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    if ui
                        .button("Reset")
                        .on_hover_text("Reset all key bindings to defaults")
                        .clicked()
                    {
                        self.recording = None;
                        self.reset(events);
                    }
                    if let Some(error) = &self.error {
                        ui.colored_label(theme.selection_stroke_color32(), error);
                    } else if dirty {
                        ui.weak("Unsaved changes");
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let apply_btn = if dirty {
                            egui::Button::new("Apply")
                                .fill(theme.selection_bg_fill32())
                                .stroke(egui::Stroke::new(1.0, theme.selection_stroke_color32()))
                        } else {
                            egui::Button::new("Apply")
                        };
                        if ui.add(apply_btn).clicked() {
                            self.recording = None;
                            self.try_apply(events);
                        }
                    });
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
        Action::SelectPen => "Pencil",
        Action::SelectEraser => "Eraser",
        Action::ToggleColorPicker => "Toggle Color Picker",
        // UX item 2 conflict: `x` doubles as the Tile-placer's horizontal-flip
        // modifier. The App resolves it by tool context — while the Tile tool
        // is active the placer reads X directly and this shortcut is
        // suppressed; outside the Tile tool X still swaps colors.
        Action::SwapColors => "Swap Colors",
        Action::SelectRectangle => "Rectangle Select",
        Action::SelectWand => "Magic Wand",
        Action::SelectLasso => "Lasso Select",
        Action::SelectBrushSelector => "Brush Selector",
        Action::SelectFill => "Fill Bucket",
        Action::SelectTileTool => "Tile Tool",
        Action::SelectAnimationPlacer => "Animation Placer",
        Action::SelectText => "Text Tool",
        Action::SaveProject => "Save project",
        Action::SaveProjectAs => "Save project as",
        Action::OpenProject => "Open project",
        Action::ImportImages => "Import images",
        Action::ImportGif => "Import GIF",
        Action::ImportPyxel => "Import Pyxel",
        Action::ImportAseprite => "Import Aseprite",
        Action::ImportImage2Pixel => "Image to pixel",
        Action::ExportImage => "Export image",
        Action::ExportAnimation => "Export animation",
        Action::ExportPyxel => "Export .pyxel",
        Action::ExportAseprite => "Export .ase",
        Action::ExportSelected => "Export selection",
        Action::ResizeCanvas => "Resize canvas",
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

fn logical_keys() -> [crate::input::LogicalKey; 26] {
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
        crate::input::LogicalKey::P,
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
        crate::input::LogicalKey::P => "P",
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
                // Tall enough for every categorized action row plus the Reset/Apply
                // buttons without scrolling.
                egui::vec2(800.0, 2000.0),
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
    fn panel_renders_categorized_shortcuts_and_controls() {
        let mut panel = KeybindingsPanel::default();
        let mut events = Vec::new();
        let ctx = egui::Context::default();
        let theme = Theme::default_dark().colors;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(800.0, 1600.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| panel.ui(ui, &theme, &mut events));
            },
        );
        output.textures_delta.clear();
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(text.iter().any(|value| value == "Pencil"));
        assert!(text.iter().any(|value| value == "Ctrl+Z"));
        assert!(text.iter().any(|value| value == "Toggle grid"));
        assert!(text.iter().any(|value| value == "Reset"));
        assert!(text.iter().any(|value| value == "Apply"));
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
