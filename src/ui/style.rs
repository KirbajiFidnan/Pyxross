//! UI style configuration: presets, rounding, low-contrast dark palettes,
//! egui visuals synchronization, and JSON file persistence.

use std::fs;
use std::path::{Path, PathBuf};

use egui::{CornerRadius, Stroke};

use crate::ui::theme::{rgba32, ThemeColors};

/// Maximum corner rounding exposed in the Style panel.
pub const MAX_ROUNDING: f32 = 8.0;
/// Minimum / maximum spacing multiplier exposed in the Style panel.
pub const MIN_SPACING: f32 = 0.75;
pub const MAX_SPACING: f32 = 1.5;

fn default_one() -> f32 {
    1.0
}

/// Built-in dark style presets with low-glare contrast and harmonious accents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StylePreset {
    Ash,
    Amber,
    Emerald,
    Slate,
    Rose,
    AshNBlood,
    Terminal,
}

impl StylePreset {
    pub const ALL: [StylePreset; 7] = [
        StylePreset::Ash,
        StylePreset::Amber,
        StylePreset::Emerald,
        StylePreset::Slate,
        StylePreset::Rose,
        StylePreset::AshNBlood,
        StylePreset::Terminal,
    ];

    pub fn name(self) -> &'static str {
        match self {
            StylePreset::Ash => "Ash",
            StylePreset::Amber => "Amber",
            StylePreset::Emerald => "Emerald",
            StylePreset::Slate => "Slate",
            StylePreset::Rose => "Rose",
            StylePreset::AshNBlood => "Ash'n Blood",
            StylePreset::Terminal => "Terminal",
        }
    }

    pub fn style(self) -> UiStyle {
        let colors = match self {
            StylePreset::Ash => StyleColors {
                clear_color: [26, 27, 30, 255],
                panel_bg: [35, 37, 41, 255],
                panel_header_bg: [44, 46, 51, 255],
                panel_border: [58, 61, 68, 255],
                list_bg: [28, 30, 34, 255],
                list_stripe_bg: [32, 34, 38, 255],
                accent_fill: [64, 73, 86, 255],
                accent_stroke: [128, 142, 162, 255],
                text_primary: [214, 217, 222, 255],
                text_muted: [142, 148, 158, 255],
            },
            StylePreset::Amber => StyleColors {
                clear_color: [28, 25, 22, 255],
                panel_bg: [38, 34, 29, 255],
                panel_header_bg: [48, 42, 36, 255],
                panel_border: [64, 56, 47, 255],
                list_bg: [31, 28, 24, 255],
                list_stripe_bg: [35, 31, 27, 255],
                accent_fill: [84, 68, 46, 255],
                accent_stroke: [158, 130, 88, 255],
                text_primary: [222, 215, 204, 255],
                text_muted: [154, 145, 132, 255],
            },
            StylePreset::Emerald => StyleColors {
                clear_color: [23, 28, 26, 255],
                panel_bg: [31, 38, 35, 255],
                panel_header_bg: [39, 48, 44, 255],
                panel_border: [52, 65, 59, 255],
                list_bg: [25, 31, 28, 255],
                list_stripe_bg: [29, 35, 32, 255],
                accent_fill: [48, 78, 65, 255],
                accent_stroke: [96, 145, 122, 255],
                text_primary: [210, 220, 215, 255],
                text_muted: [136, 152, 144, 255],
            },
            StylePreset::Slate => StyleColors {
                clear_color: [24, 27, 33, 255],
                panel_bg: [32, 36, 44, 255],
                panel_header_bg: [40, 45, 55, 255],
                panel_border: [54, 62, 76, 255],
                list_bg: [26, 30, 37, 255],
                list_stripe_bg: [30, 34, 41, 255],
                accent_fill: [55, 72, 96, 255],
                accent_stroke: [106, 132, 168, 255],
                text_primary: [212, 218, 228, 255],
                text_muted: [138, 147, 162, 255],
            },
            StylePreset::Rose => StyleColors {
                clear_color: [29, 23, 25, 255],
                panel_bg: [39, 31, 34, 255],
                panel_header_bg: [49, 39, 43, 255],
                panel_border: [68, 52, 58, 255],
                list_bg: [32, 25, 28, 255],
                list_stripe_bg: [36, 28, 31, 255],
                accent_fill: [92, 52, 64, 255],
                accent_stroke: [168, 102, 122, 255],
                text_primary: [226, 212, 217, 255],
                text_muted: [158, 138, 145, 255],
            },
            StylePreset::AshNBlood => StyleColors {
                clear_color: [24, 24, 26, 255],
                panel_bg: [34, 34, 37, 255],
                panel_header_bg: [43, 42, 46, 255],
                panel_border: [64, 54, 56, 255],
                list_bg: [27, 27, 30, 255],
                list_stripe_bg: [31, 31, 34, 255],
                accent_fill: [94, 34, 40, 255],
                accent_stroke: [168, 66, 76, 255],
                text_primary: [220, 216, 217, 255],
                text_muted: [150, 142, 144, 255],
            },
            StylePreset::Terminal => StyleColors {
                clear_color: [16, 22, 18, 255],
                panel_bg: [22, 30, 25, 255],
                panel_header_bg: [28, 39, 32, 255],
                panel_border: [42, 66, 50, 255],
                list_bg: [18, 25, 20, 255],
                list_stripe_bg: [21, 29, 24, 255],
                accent_fill: [36, 86, 52, 255],
                accent_stroke: [78, 192, 114, 255],
                text_primary: [188, 232, 198, 255],
                text_muted: [112, 158, 126, 255],
            },
        };
        UiStyle {
            preset: self.name().to_string(),
            rounding: 0.0,
            spacing: 1.0,
            colors,
            banner: None,
        }
    }
}

/// Editable color tokens for the modern flat UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StyleColors {
    pub clear_color: [u8; 4],
    pub panel_bg: [u8; 4],
    pub panel_header_bg: [u8; 4],
    pub panel_border: [u8; 4],
    pub list_bg: [u8; 4],
    pub list_stripe_bg: [u8; 4],
    pub accent_fill: [u8; 4],
    pub accent_stroke: [u8; 4],
    pub text_primary: [u8; 4],
    pub text_muted: [u8; 4],
}

/// Persistent UI style configuration (preset name, corner rounding, spacing,
/// colors, banner).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UiStyle {
    pub preset: String,
    pub rounding: f32,
    #[serde(default = "default_one")]
    pub spacing: f32,
    pub colors: StyleColors,
    #[serde(default)]
    pub banner: Option<String>,
}

impl Default for UiStyle {
    fn default() -> Self {
        StylePreset::Ash.style()
    }
}

impl UiStyle {
    /// Clamps rounding to `[0.0, MAX_ROUNDING]`.
    pub fn clamped_rounding(&self) -> f32 {
        self.rounding.clamp(0.0, MAX_ROUNDING)
    }

    /// Clamps spacing multiplier to `[MIN_SPACING, MAX_SPACING]`.
    pub fn clamped_spacing(&self) -> f32 {
        self.spacing.clamp(MIN_SPACING, MAX_SPACING)
    }

    /// Returns `true` when this style is a custom user-modified theme (eligible
    /// to be saved as a custom theme file).
    pub fn is_custom(&self) -> bool {
        self.preset.eq_ignore_ascii_case("Custom")
    }

    /// Projects these style colors onto `ThemeColors` so all panels, canvas
    /// border, and top bar read from one consistent palette.
    pub fn apply_to_theme_colors(&self, target: &mut ThemeColors) {
        target.clear_color = self.colors.clear_color;
        target.canvas_border = self.colors.panel_border;
        target.panel_bg = self.colors.panel_bg;
        target.panel_header_bg = self.colors.panel_header_bg;
        target.panel_border = self.colors.panel_border;
        target.selection_bg_fill = self.colors.accent_fill;
        target.selection_stroke_color = self.colors.accent_stroke;
        target.list_bg = self.colors.list_bg;
        target.list_stripe_bg = self.colors.list_stripe_bg;
    }

    /// Synchronizes egui's global `Style`/`Visuals` with this `UiStyle`.
    pub fn apply_to_egui(&self, ctx: &egui::Context) {
        let r = self.clamped_rounding().round() as u8;
        let corner = CornerRadius::same(r);
        let sp = self.clamped_spacing();
        let clear = rgba32(self.colors.clear_color);
        let panel = rgba32(self.colors.panel_bg);
        let header = rgba32(self.colors.panel_header_bg);
        let border = rgba32(self.colors.panel_border);
        let well = rgba32(self.colors.list_bg);
        let accent_fill = rgba32(self.colors.accent_fill);
        let accent_stroke = rgba32(self.colors.accent_stroke);
        let text_primary = rgba32(self.colors.text_primary);

        ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(6.0 * sp, 4.0 * sp);
            style.spacing.button_padding = egui::vec2(6.0 * sp, 3.0 * sp);

            let mut v = egui::Visuals::dark();
            v.override_text_color = Some(text_primary);
            v.panel_fill = panel;
            v.window_fill = panel;
            v.faint_bg_color = well;
            v.extreme_bg_color = clear;
            v.window_corner_radius = corner;
            v.menu_corner_radius = corner;
            v.window_stroke = Stroke::new(1.0, border);
            v.window_shadow = egui::epaint::Shadow::NONE;
            v.popup_shadow = egui::epaint::Shadow::NONE;

            v.selection.bg_fill = accent_fill;
            v.selection.stroke = Stroke::new(1.0, accent_stroke);

            v.widgets.noninteractive.bg_fill = panel;
            v.widgets.noninteractive.weak_bg_fill = panel;
            v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, border);
            v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, text_primary);
            v.widgets.noninteractive.corner_radius = corner;
            v.widgets.noninteractive.expansion = 0.0;

            v.widgets.inactive.bg_fill = header;
            v.widgets.inactive.weak_bg_fill = header;
            v.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
            v.widgets.inactive.fg_stroke = Stroke::new(1.0, text_primary);
            v.widgets.inactive.corner_radius = corner;
            v.widgets.inactive.expansion = 0.0;

            v.widgets.hovered.bg_fill = accent_fill;
            v.widgets.hovered.weak_bg_fill = accent_fill;
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent_stroke);
            v.widgets.hovered.fg_stroke = Stroke::new(1.0, text_primary);
            v.widgets.hovered.corner_radius = corner;
            v.widgets.hovered.expansion = 0.0;

            v.widgets.active.bg_fill = accent_fill;
            v.widgets.active.weak_bg_fill = accent_fill;
            v.widgets.active.bg_stroke = Stroke::new(1.0, accent_stroke);
            v.widgets.active.fg_stroke = Stroke::new(1.0, text_primary);
            v.widgets.active.corner_radius = corner;
            v.widgets.active.expansion = 0.0;

            v.widgets.open.bg_fill = header;
            v.widgets.open.weak_bg_fill = header;
            v.widgets.open.bg_stroke = Stroke::new(1.0, accent_stroke);
            v.widgets.open.fg_stroke = Stroke::new(1.0, text_primary);
            v.widgets.open.corner_radius = corner;
            v.widgets.open.expansion = 0.0;

            style.visuals = v;
        });
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize style: {e}"))?;
        fs::write(path, json).map_err(|e| format!("Failed to write style file: {e}"))
    }

    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("Failed to read style file: {e}"))?;
        let mut parsed: Self = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Failed to parse style file: {e}"))?;
        parsed.rounding = parsed.clamped_rounding();
        parsed.spacing = parsed.clamped_spacing();
        Ok(parsed)
    }

    pub fn load_user_or_default() -> Self {
        if cfg!(test) {
            return Self::default();
        }
        user_style_path()
            .and_then(|path| Self::load_from_file(&path).ok())
            .unwrap_or_default()
    }
}

/// Default config file location: `<config>/pyxross/style.json`.
pub fn user_style_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("pyxross").join("style.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_presets_use_zero_rounding() {
        for preset in StylePreset::ALL {
            let style = preset.style();
            assert_eq!(style.rounding, 0.0, "{} must default to 0 rounding", preset.name());
        }
        assert_eq!(UiStyle::default().preset, "Ash");
        assert_eq!(UiStyle::default().rounding, 0.0);
    }

    #[test]
    fn style_roundtrip_via_file() {
        let dir = std::env::temp_dir().join(format!("pyx_style_test_{}", std::process::id()));
        let path = dir.join("custom_style.json");
        let mut style = StylePreset::Amber.style();
        style.rounding = 3.0;
        style.preset = "Custom".to_string();
        style.save_to_file(&path).unwrap();

        let loaded = UiStyle::load_from_file(&path).unwrap();
        assert_eq!(loaded, style);

        // Legacy JSON without ui_scale / border_width / spacing falls back to 1.0 defaults.
        let legacy_json = r#"{
            "preset": "Ash",
            "rounding": 2.0,
            "colors": {
                "clear_color": [26,27,30,255],
                "panel_bg": [35,37,41,255],
                "panel_header_bg": [44,46,51,255],
                "panel_border": [58,61,68,255],
                "list_bg": [28,30,34,255],
                "list_stripe_bg": [32,34,38,255],
                "accent_fill": [64,73,86,255],
                "accent_stroke": [128,142,162,255],
                "text_primary": [214,217,222,255],
                "text_muted": [142,148,158,255]
            }
        }"#;
        let legacy_path = dir.join("legacy_style.json");
        fs::write(&legacy_path, legacy_json).unwrap();
        let legacy = UiStyle::load_from_file(&legacy_path).unwrap();
        assert_eq!(legacy.rounding, 2.0);
        assert_eq!(legacy.spacing, 1.0);
        assert!(legacy.banner.is_none());

        let _ = fs::remove_dir_all(&dir);
    }
}
