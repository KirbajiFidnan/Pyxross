use serde::{Deserialize, Serialize};

const DEFAULT_KEYMAP_JSON: &str = include_str!("../assets/keybindings.json");

#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    TransformRotateCw,
    TransformRotateCcw,
    TransformGrow,
    TransformShrink,
    FlipHorizontal,
    FlipVertical,
    CancelTransform,
    ToggleGrid,
    SelectPen,
    SelectEraser,
    ToggleColorPicker,
    SwapColors,
    SelectDelete,
    InvertSelection,
    FlipSelectionHorizontal,
    FlipSelectionVertical,
    RotateSelection,
    SelectRectangle,
    SelectWand,
    SelectLasso,
}

impl Action {
    pub const ALL: [Self; 25] = [
        Self::Undo,
        Self::Redo,
        Self::Copy,
        Self::Cut,
        Self::Paste,
        Self::TransformRotateCw,
        Self::TransformRotateCcw,
        Self::TransformGrow,
        Self::TransformShrink,
        Self::FlipHorizontal,
        Self::FlipVertical,
        Self::CancelTransform,
        Self::ToggleGrid,
        Self::SelectPen,
        Self::SelectEraser,
        Self::ToggleColorPicker,
        Self::SwapColors,
        Self::SelectDelete,
        Self::InvertSelection,
        Self::FlipSelectionHorizontal,
        Self::FlipSelectionVertical,
        Self::RotateSelection,
        Self::SelectRectangle,
        Self::SelectWand,
        Self::SelectLasso,
    ];
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicalKey {
    Z,
    Y,
    C,
    X,
    V,
    R,
    Plus,
    Equals,
    Minus,
    H,
    Escape,
    G,
    D,
    E,
    Delete,
    I,
    F,
    S,
    W,
    L,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct Modifiers {
    #[serde(default)]
    pub command: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub ctrl: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: LogicalKey,
    #[serde(default)]
    pub modifiers: Modifiers,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Keymap {
    pub bindings: std::collections::BTreeMap<Action, KeyBinding>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KeymapError {
    Duplicate(KeyBinding),
    Missing(Action),
    Unknown(String),
}

impl std::fmt::Display for KeymapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(_) => write!(f, "duplicate key binding"),
            Self::Missing(action) => write!(f, "missing binding for {action:?}"),
            Self::Unknown(action) => write!(f, "unknown action {action}"),
        }
    }
}

impl std::error::Error for KeymapError {}

impl Keymap {
    pub fn defaults() -> Self {
        match serde_json::from_str(DEFAULT_KEYMAP_JSON) {
            Ok(keymap) => keymap,
            Err(_) => Self {
                bindings: std::collections::BTreeMap::new(),
            },
        }
    }

    pub fn validate(&self) -> Result<(), KeymapError> {
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            let Some(binding) = self.bindings.get(&action) else {
                return Err(KeymapError::Missing(action));
            };
            if !seen.insert(*binding) {
                return Err(KeymapError::Duplicate(*binding));
            }
        }
        Ok(())
    }

    pub fn merge(&self, overrides: &Self) -> Result<Self, KeymapError> {
        let mut merged = self.clone();
        for (action, binding) in &overrides.bindings {
            merged.bindings.insert(*action, *binding);
        }
        for action in Action::ALL {
            if !merged.bindings.contains_key(&action) {
                return Err(KeymapError::Missing(action));
            }
        }
        Ok(merged)
    }

    pub fn binding(&self, action: Action) -> Option<KeyBinding> {
        self.bindings.get(&action).copied()
    }

    pub fn pressed(&self, action: Action, input: &egui::InputState) -> bool {
        let Some(binding) = self.binding(action) else {
            return false;
        };
        if action == Action::Redo
            && matches!(Keymap::defaults().binding(Action::Redo), Some(default) if binding == default)
            && input.key_pressed(egui::Key::Z)
            && input.modifiers.command
            && input.modifiers.shift
        {
            return true;
        }
        let modifiers = binding.modifiers;
        let hit = |key| {
            input.key_pressed(key)
                && input.modifiers.matches_exact(egui::Modifiers {
                    alt: modifiers.alt,
                    ctrl: modifiers.ctrl,
                    shift: modifiers.shift,
                    mac_cmd: false,
                    command: modifiers.command,
                })
        };
        let key = match binding.key {
            LogicalKey::Z => egui::Key::Z,
            LogicalKey::Y => egui::Key::Y,
            LogicalKey::C => egui::Key::C,
            LogicalKey::X => egui::Key::X,
            LogicalKey::V => egui::Key::V,
            LogicalKey::R => egui::Key::R,
            LogicalKey::Plus => egui::Key::Plus,
            LogicalKey::Equals => egui::Key::Equals,
            LogicalKey::Minus => egui::Key::Minus,
            LogicalKey::H => egui::Key::H,
            LogicalKey::Escape => egui::Key::Escape,
            LogicalKey::G => egui::Key::G,
            LogicalKey::D => egui::Key::D,
            LogicalKey::E => egui::Key::E,
            LogicalKey::I => egui::Key::I,
            LogicalKey::F => egui::Key::F,
            LogicalKey::Delete => {
                return hit(egui::Key::Delete) || hit(egui::Key::Backspace);
            }
            LogicalKey::S => egui::Key::S,
            LogicalKey::W => egui::Key::W,
            LogicalKey::L => egui::Key::L,
        };
        hit(key)
    }
}

#[cfg(test)]
mod keymap_tests {
    use super::*;

    #[test]
    fn defaults_preserve_existing_shortcuts() {
        let keymap = Keymap::defaults();
        assert_eq!(keymap.binding(Action::Undo).unwrap().key, LogicalKey::Z);
        assert!(keymap.binding(Action::Undo).unwrap().modifiers.command);
        assert_eq!(
            keymap.binding(Action::CancelTransform).unwrap().key,
            LogicalKey::Escape
        );
    }

    #[test]
    fn selection_actions_have_default_bindings() {
        let keymap = Keymap::defaults();
        let delete = keymap.binding(Action::SelectDelete).unwrap();
        assert_eq!(delete.key, LogicalKey::Delete);
        assert_eq!(delete.modifiers, Modifiers::default());
        let invert = keymap.binding(Action::InvertSelection).unwrap();
        assert_eq!(invert.key, LogicalKey::I);
        assert!(invert.modifiers.command);
    }

    #[test]
    fn keymap_defaults_validate_with_fieldier_children() {
        let keymap = Keymap::defaults();
        assert_eq!(keymap.bindings.len(), Action::ALL.len());
        assert!(
            keymap.validate().is_ok(),
            "every action, including the Fieldier children, must have a distinct default binding"
        );
    }

    #[test]
    fn s_w_l_are_free_unique_bindings() {
        let keymap = Keymap::defaults();
        assert_eq!(
            keymap.binding(Action::SelectRectangle).unwrap().key,
            LogicalKey::S
        );
        assert_eq!(
            keymap.binding(Action::SelectWand).unwrap().key,
            LogicalKey::W
        );
        assert_eq!(
            keymap.binding(Action::SelectLasso).unwrap().key,
            LogicalKey::L
        );
        assert_eq!(
            keymap.binding(Action::SelectWand).unwrap().modifiers,
            Modifiers::default()
        );
        for key in [LogicalKey::S, LogicalKey::W, LogicalKey::L] {
            let count = Action::ALL
                .iter()
                .filter(|action| {
                    keymap
                        .binding(**action)
                        .is_some_and(|binding| binding.key == key)
                })
                .count();
            assert_eq!(count, 1, "{key:?} must be bound to exactly one action");
        }
    }

    #[test]
    fn every_action_has_a_unique_default_binding() {
        let keymap = Keymap::defaults();
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            let binding = keymap
                .binding(action)
                .unwrap_or_else(|| panic!("{action:?} has no default binding"));
            assert!(
                seen.insert(binding),
                "{action:?} reuses a key already bound to another action"
            );
        }
    }

    #[test]
    fn action_names_are_stable() {
        let json = serde_json::to_string(&Action::TransformRotateCw).unwrap();
        assert_eq!(json, "\"transform_rotate_cw\"");
    }

    #[test]
    fn one_override_merges_with_defaults() {
        let mut override_bindings = std::collections::BTreeMap::new();
        override_bindings.insert(
            Action::Undo,
            KeyBinding {
                key: LogicalKey::G,
                modifiers: Modifiers::default(),
            },
        );
        let merged = Keymap::defaults()
            .merge(&Keymap {
                bindings: override_bindings,
            })
            .unwrap();
        assert_eq!(merged.binding(Action::Undo).unwrap().key, LogicalKey::G);
        assert_eq!(merged.binding(Action::Redo).unwrap().key, LogicalKey::Y);
    }

    #[test]
    fn incomplete_and_duplicate_maps_are_rejected() {
        let mut bindings = Keymap::defaults().bindings;
        bindings.remove(&Action::Undo);
        assert!(matches!(
            Keymap { bindings }.validate(),
            Err(KeymapError::Missing(Action::Undo))
        ));
        let mut bindings = Keymap::defaults().bindings;
        let undo = bindings[&Action::Undo];
        bindings.insert(Action::Redo, undo);
        assert!(matches!(
            Keymap { bindings }.validate(),
            Err(KeymapError::Duplicate(_))
        ));
    }

    #[test]
    fn stable_json_rejects_unknown_actions() {
        let value = serde_json::json!({"bindings": {"not_an_action": {"key": "z"}}});
        assert!(
            serde_json::from_value::<std::collections::BTreeMap<Action, KeyBinding>>(
                value["bindings"].clone()
            )
            .is_err()
        );
    }

    fn pressed(
        ctx: &egui::Context,
        keymap: &Keymap,
        action: Action,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> bool {
        let mut result = false;
        let mut output = ctx.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ],
                ..Default::default()
            },
            |ui| {
                result = ui.ctx().input(|input| keymap.pressed(action, input));
            },
        );
        output.textures_delta.clear();
        result
    }

    #[test]
    fn ctrl_z_dispatches_undo_when_ctrl_reports_both_ctrl_and_command() {
        let keymap = Keymap::defaults();
        let ctx = egui::Context::default();
        // On non-macOS egui reports Ctrl as BOTH `ctrl` and `command`.
        let ctrl = egui::Modifiers {
            ctrl: true,
            command: true,
            ..egui::Modifiers::NONE
        };

        assert!(pressed(&ctx, &keymap, Action::Undo, egui::Key::Z, ctrl));
        assert!(pressed(&ctx, &keymap, Action::Redo, egui::Key::Y, ctrl));
        assert!(!pressed(
            &ctx,
            &keymap,
            Action::Undo,
            egui::Key::Z,
            egui::Modifiers::NONE
        ));
    }
}

/// The active editing tool.
///
/// Deliberate switches go through [`ToolState::select_tool`]; the temporary
/// right-button eyedropper is driven by [`ToolState::temporary_eyedropper`] /
/// [`ToolState::release_temporary`] and never counts as a deliberate switch
/// (FEATURES.md §1, D62 semantics).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Pencil,
    Eraser,
    Fill,
    Eyedropper,
    /// Marquee selection: drag a rectangle, then move / copy / cut / paste /
    /// delete / flip it.
    Fieldier,
    /// Hidden draw-tool mode (the Pen's [`DrawMode`](crate::core::brush::DrawMode)).
    /// Present in the code but never listed in the toolbox `TOOLS` and never
    /// selectable from the UI; treated as the Pen's mode wherever a match needs
    /// a behaviour.
    Draw,
}

/// The active Fieldier child: which selection behaviour the Fieldier performs.
///
/// S / W / L pick the child; the default is the rectangular marquee.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FieldierChild {
    #[default]
    Rectangle,
    Wand,
    Lasso,
}

/// The wand child's flood-fill parameters.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WandSettings {
    /// Whether the flood fill spreads only to touching pixels (`true`) or to
    /// every matching pixel on the layer (`false`).
    pub contiguous: bool,
    /// Maximum colour distance still treated as a match.
    pub tolerance: u8,
    /// Whether the flood fill stays inside the current selection.
    pub restrict_to_region: bool,
}

impl Default for WandSettings {
    fn default() -> Self {
        Self {
            contiguous: true,
            tolerance: 0,
            restrict_to_region: false,
        }
    }
}

/// Current tool plus the tool to restore after a temporary eyedropper session.
///
/// `previous` is `Some` only while the temporary eyedropper is active; a
/// deliberate [`ToolState::select_tool`] cancels any pending restore.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ToolState {
    tool: Tool,
    previous: Option<Tool>,
    child: FieldierChild,
    wand: WandSettings,
}

impl ToolState {
    /// New state with the default tool ([`Tool::Pencil`]) and no pending restore.
    pub fn new() -> Self {
        Self {
            tool: Tool::Pencil,
            previous: None,
            child: FieldierChild::default(),
            wand: WandSettings::default(),
        }
    }

    /// Deliberate tool switch (toolbox click, keybinding, …).
    ///
    /// Sets the current tool and cancels any pending temporary-eyedropper
    /// restore: a deliberate choice supersedes the temporary session.
    pub fn select_tool(&mut self, tool: Tool) {
        self.tool = tool;
        self.previous = None;
    }

    /// Select a Fieldier child and switch to [`Tool::Fieldier`].
    ///
    /// Like [`ToolState::select_tool`], a deliberate choice cancels any pending
    /// temporary-eyedropper restore.
    pub fn select_child(&mut self, child: FieldierChild) {
        self.child = child;
        self.tool = Tool::Fieldier;
        self.previous = None;
    }

    /// The active Fieldier child.
    pub fn child(&self) -> FieldierChild {
        self.child
    }

    /// The Fieldier wand settings.
    pub fn wand(&self) -> WandSettings {
        self.wand
    }

    /// Mutable access to the Fieldier wand settings.
    pub fn wand_mut(&mut self) -> &mut WandSettings {
        &mut self.wand
    }

    /// Enter the temporary eyedropper: remember the current tool as `previous`
    /// and switch to [`Tool::Eyedropper`].
    ///
    /// If already eyedroppering, the earlier `previous` is kept so a nested
    /// call cannot clobber the tool to restore.
    pub fn temporary_eyedropper(&mut self) {
        if self.tool != Tool::Eyedropper {
            self.previous = Some(self.tool);
        }
        self.tool = Tool::Eyedropper;
    }

    /// Leave the temporary eyedropper: restore `previous` if set, then clear it.
    pub fn release_temporary(&mut self) {
        if let Some(previous) = self.previous.take() {
            self.tool = previous;
        }
    }

    /// The currently active tool.
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// The tool to restore after the temporary eyedropper, if any.
    pub fn previous(&self) -> Option<Tool> {
        self.previous
    }
}

impl Default for ToolState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults_to_pencil_with_no_previous() {
        let state = ToolState::new();
        assert_eq!(state.tool(), Tool::Pencil);
        assert_eq!(state.previous(), None);
    }

    #[test]
    fn select_tool_switches_current_tool() {
        let mut state = ToolState::new();
        state.select_tool(Tool::Eraser);
        assert_eq!(state.tool(), Tool::Eraser);
        assert_eq!(state.previous(), None);
    }

    #[test]
    fn temporary_eyedropper_remembers_previous() {
        let mut state = ToolState::new();
        state.select_tool(Tool::Fill);
        state.temporary_eyedropper();
        assert_eq!(state.tool(), Tool::Eyedropper);
        assert_eq!(state.previous(), Some(Tool::Fill));
    }

    #[test]
    fn release_temporary_restores_previous() {
        let mut state = ToolState::new();
        state.select_tool(Tool::Fill);
        state.temporary_eyedropper();
        state.release_temporary();
        assert_eq!(state.tool(), Tool::Fill);
        assert_eq!(state.previous(), None);
    }

    #[test]
    fn temporary_eyedropper_when_already_eyedropper_keeps_earlier_previous() {
        let mut state = ToolState::new();
        state.select_tool(Tool::Fill);
        state.temporary_eyedropper();
        state.temporary_eyedropper();
        assert_eq!(state.tool(), Tool::Eyedropper);
        assert_eq!(state.previous(), Some(Tool::Fill));
    }

    #[test]
    fn release_temporary_without_previous_is_noop() {
        let mut state = ToolState::new();
        state.release_temporary();
        assert_eq!(state.tool(), Tool::Pencil);
        assert_eq!(state.previous(), None);
    }

    #[test]
    fn select_tool_cancels_pending_temporary_restore() {
        let mut state = ToolState::new();
        state.select_tool(Tool::Fill);
        state.temporary_eyedropper();
        state.select_tool(Tool::Eraser);
        assert_eq!(state.tool(), Tool::Eraser);
        assert_eq!(state.previous(), None);
    }

    #[test]
    fn tool_state_defaults_to_rectangle_child() {
        let state = ToolState::new();
        assert_eq!(state.child(), FieldierChild::Rectangle);
        assert_eq!(state.child(), FieldierChild::default());
    }

    #[test]
    fn select_child_switches_to_fieldier_and_clears_previous() {
        let mut state = ToolState::new();
        state.select_tool(Tool::Fill);
        state.temporary_eyedropper();
        state.select_child(FieldierChild::Wand);
        assert_eq!(state.tool(), Tool::Fieldier);
        assert_eq!(state.child(), FieldierChild::Wand);
        assert_eq!(state.previous(), None);
    }

    #[test]
    fn wand_settings_defaults_are_on_zero_off() {
        let settings = WandSettings::default();
        assert!(settings.contiguous, "contiguous defaults on");
        assert_eq!(settings.tolerance, 0, "tolerance defaults to zero");
        assert!(
            !settings.restrict_to_region,
            "restrict-to-region defaults off"
        );

        let mut state = ToolState::new();
        assert_eq!(state.wand(), settings);
        state.wand_mut().tolerance = 7;
        assert_eq!(state.wand().tolerance, 7);
    }
}
