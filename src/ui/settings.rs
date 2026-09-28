//! Theme browser panel (F4). R5 milestone.
//!
//! A settings area where the user can see all installed themes, switch the
//! active theme, refresh the theme list, and preview the current theme's
//! colors. Unlike the other panels (pure views that emit events for the App
//! shell to apply), this panel IS the controller for theme switching: it
//! mutates the [`ThemeManager`] directly. Every color painted here comes from
//! the theme system — no hardcoded `Color32` (hard rule 3).

use crate::ui::theme::{ThemeColors, ThemeManager};

/// Side length of one color swatch in the preview strip, in points.
const SWATCH_SIZE: f32 = 16.0;

/// Vertical gap between settings items (heading → swatches → combo → refresh).
const ITEM_SPACING: f32 = 4.0;

/// Theme browser panel.
///
/// Holds only the most recent [`ThemeError`] message so a failed switch or
/// refresh can be shown to the user in the default egui text color.
pub struct SettingsPanel {
    last_error: Option<String>,
}

impl SettingsPanel {
    /// New panel.
    pub fn new() -> Self {
        Self { last_error: None }
    }

    /// Render the panel for one frame.
    ///
    /// * `theme` — the theme manager; switching and refreshing mutate it.
    ///
    /// Layout: a heading, a live swatch strip of the current theme's colors,
    /// a theme selector combo, a refresh button, and an error line.
    pub fn ui(&mut self, ui: &mut egui::Ui, theme: &mut ThemeManager) {
        ui.strong("Themes");
        ui.add_space(ITEM_SPACING);

        // Live preview of the current theme's colors.
        swatch_strip(ui, &theme.current().colors);

        ui.add_space(ITEM_SPACING);

        // Theme selector. The combo always shows the ACTIVE theme; switching
        // only happens when the user picks a different name.
        let current_name = theme.current().name.clone();
        let mut selection = current_name.clone();
        egui::ComboBox::from_id_salt("theme_selector")
            .selected_text(&current_name)
            .show_ui(ui, |ui| {
                for meta in theme.available() {
                    ui.selectable_value(&mut selection, meta.name.clone(), &meta.name);
                }
            });
        if selection != current_name {
            self.select_theme(&selection, theme);
        }

        ui.add_space(ITEM_SPACING);

        // Refresh: re-scan the theme dirs for newly installed themes.
        if ui.button("⟳ Refresh").clicked() {
            self.refresh(theme);
        }

        // Error line — default egui text color only (no hardcoded Color32).
        if let Some(err) = &self.last_error {
            ui.label(err);
        }
    }

    /// Switch the active theme; stores the error message on failure.
    fn select_theme(&mut self, name: &str, theme: &mut ThemeManager) {
        match theme.switch(name) {
            Ok(()) => self.last_error = None,
            Err(e) => self.last_error = Some(format!("{e}")),
        }
    }

    /// Re-scan the theme dirs; stores the error message on failure.
    fn refresh(&mut self, theme: &mut ThemeManager) {
        match theme.refresh() {
            Ok(_) => self.last_error = None,
            Err(e) => self.last_error = Some(format!("{e}")),
        }
    }
}

impl Default for SettingsPanel {
    fn default() -> Self {
        Self::new()
    }
}

/// A strip of small colored rects previewing the current theme's tokens.
fn swatch_strip(ui: &mut egui::Ui, colors: &ThemeColors) {
    let tokens: [(&str, egui::Color32); 6] = [
        ("clear", colors.clear_color32()),
        ("panel", colors.panel_bg32()),
        ("border", colors.canvas_border32()),
        ("sel fill", colors.selection_fill32()),
        ("sel stroke", colors.selection_stroke32()),
        ("gizmo", colors.gizmo_fill_normal32()),
    ];
    ui.horizontal_wrapped(|ui| {
        for (label, color) in tokens {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(SWATCH_SIZE, SWATCH_SIZE), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, color);
            ui.label(label);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::Theme;

    struct TempDir(std::path::PathBuf);

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var_os(name);
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }

    impl TempDir {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A unique scratch dir per test (process id + tag), so parallel tests
    /// never collide; Drop removes it on success and panic.
    fn temp_dir(tag: &str) -> TempDir {
        let dir =
            std::env::temp_dir().join(format!("pyxross_settings_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    /// A theme with a distinct name, otherwise identical to the built-in dark.
    fn sample_theme(name: &str) -> Theme {
        let mut theme = Theme::default_dark();
        theme.name = name.to_string();
        theme
    }

    /// A manager whose theme dirs point at `dir` (via `$PYXROSS_THEMES`, the
    /// only public way to steer `ThemeManager::new()`'s scan). The env var is
    /// removed right after construction — the manager keeps its captured dirs.
    fn manager_with_dir(dir: &std::path::Path) -> ThemeManager {
        let _env = EnvVarGuard::set("PYXROSS_THEMES", dir);
        let manager = ThemeManager::new();
        manager
    }

    /// Run one headless frame of the panel in a full-screen central panel.
    ///
    /// Same harness style as `src/ui/layers.rs` tests: a manual
    /// `egui::Context` driven by `run_ui` with synthetic events.
    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        panel: &mut SettingsPanel,
        theme: &mut ThemeManager,
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
        let mut output = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ui, |ui| {
                    panel.ui(ui, theme);
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
        panel: &mut SettingsPanel,
        theme: &mut ThemeManager,
    ) {
        run_frame(ctx, vec![egui::Event::PointerMoved(pos)], panel, theme);
        run_frame(
            ctx,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            panel,
            theme,
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
            theme,
        );
    }

    /// Click the first widget whose rendered text is `text`, clicking just
    /// inside the text's top-left corner (which is inside the widget).
    fn click_text(
        ctx: &egui::Context,
        output: &egui::FullOutput,
        text: &str,
        panel: &mut SettingsPanel,
        theme: &mut ThemeManager,
    ) {
        let pos = text_positions(output, text)
            .into_iter()
            .next()
            .expect("text should be rendered");
        click(ctx, pos + egui::vec2(4.0, 8.0), panel, theme);
    }

    #[test]
    fn default_panel_shows_current_theme_selected() {
        let ctx = egui::Context::default();
        let mut panel = SettingsPanel::new();
        let mut manager = ThemeManager::new();
        let output = run_frame(&ctx, vec![], &mut panel, &mut manager);

        // The combo's selected text is the active theme ("Dark" builtin).
        assert!(
            !text_positions(&output, "Dark").is_empty(),
            "combo should show the active theme name"
        );
        // The swatch strip labels render.
        for label in [
            "clear",
            "panel",
            "border",
            "sel fill",
            "sel stroke",
            "gizmo",
        ] {
            assert!(
                !text_positions(&output, label).is_empty(),
                "missing swatch label {label}"
            );
        }
        assert!(panel.last_error.is_none(), "a plain frame stores no error");
    }

    #[test]
    fn combo_lists_all_available_themes() {
        let dir = temp_dir("combo_list");
        std::fs::write(
            dir.path().join("user.json"),
            serde_json::to_vec(&sample_theme("UserTheme")).unwrap(),
        )
        .unwrap();
        let mut manager = manager_with_dir(dir.path());

        let ctx = egui::Context::default();
        let mut panel = SettingsPanel::new();
        let output = run_frame(&ctx, vec![], &mut panel, &mut manager);
        // The combo button shows the active theme.
        assert!(
            !text_positions(&output, "Dark").is_empty(),
            "combo should show the active theme"
        );
        // Open the combo; the popup lists the user theme too.
        click_text(&ctx, &output, "Dark", &mut panel, &mut manager);
        let output = run_frame(&ctx, vec![], &mut panel, &mut manager);
        assert!(
            !text_positions(&output, "UserTheme").is_empty(),
            "popup should list the user theme"
        );
    }

    #[test]
    fn switching_theme_via_combo_updates_manager() {
        let dir = temp_dir("switch_combo");
        std::fs::write(
            dir.path().join("user.json"),
            serde_json::to_vec(&sample_theme("UserTheme")).unwrap(),
        )
        .unwrap();
        let mut manager = manager_with_dir(dir.path());

        let ctx = egui::Context::default();
        let mut panel = SettingsPanel::new();
        let output = run_frame(&ctx, vec![], &mut panel, &mut manager);
        // Open the combo.
        click_text(&ctx, &output, "Dark", &mut panel, &mut manager);
        // Click the user theme in the popup.
        let output = run_frame(&ctx, vec![], &mut panel, &mut manager);
        click_text(&ctx, &output, "UserTheme", &mut panel, &mut manager);

        assert_eq!(manager.current().name, "UserTheme");
        assert!(
            panel.last_error.is_none(),
            "a successful switch stores no error"
        );
    }

    #[test]
    fn refresh_button_rescans_new_themes() {
        let dir = temp_dir("refresh_btn");
        let mut manager = manager_with_dir(dir.path());
        assert!(
            !manager.available().iter().any(|m| m.name == "FreshTheme"),
            "theme should not be known before the refresh"
        );

        // A theme appears on disk after the initial scan.
        std::fs::write(
            dir.path().join("fresh.json"),
            serde_json::to_vec(&sample_theme("FreshTheme")).unwrap(),
        )
        .unwrap();

        let ctx = egui::Context::default();
        let mut panel = SettingsPanel::new();
        let output = run_frame(&ctx, vec![], &mut panel, &mut manager);
        click_text(&ctx, &output, "⟳ Refresh", &mut panel, &mut manager);

        assert!(
            manager.available().iter().any(|m| m.name == "FreshTheme"),
            "refresh should rescan and find the new theme"
        );
        assert!(
            panel.last_error.is_none(),
            "a successful refresh stores no error"
        );
    }

    #[test]
    fn failed_switch_stores_error() {
        let mut manager = ThemeManager::new();
        let mut panel = SettingsPanel::new();
        panel.select_theme("Nope", &mut manager);

        let err = panel.last_error.expect("error should be stored");
        assert!(err.contains("not found"), "got: {err}");
        assert_eq!(
            manager.current().name,
            "Dark",
            "a failed switch keeps the current theme"
        );
    }

    #[test]
    fn swatch_strip_previews_current_theme_colors() {
        let ctx = egui::Context::default();
        let mut panel = SettingsPanel::new();
        let mut manager = ThemeManager::new();
        let output = run_frame(&ctx, vec![], &mut panel, &mut manager);

        // The clear-color swatch is painted with the theme's clear token.
        let clear = Theme::default_dark().colors.clear_color32();
        assert!(
            rect_fills(&output).iter().any(|(_, fill)| *fill == clear),
            "clear-color swatch should be painted with the theme token"
        );
    }
}
