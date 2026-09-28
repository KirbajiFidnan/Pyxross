//! 9-slice skin/theme system [D10, D11]. R5 milestone.
//!
//! Pure data types: color tokens, atlas region definitions, per-state skins,
//! and a [`ThemeManager`] that resolves named themes through a fallback chain
//! (user theme → built-in default → hardcoded defaults). No egui UI state
//! lives here — the manager is a plain data holder the App shell drives.
//!
//! Phase C note (onion-skin bridge): the `onion_prev_tint`/`onion_next_tint`
//! tokens are defined here but not yet bridged into rendering —
//! [`crate::render::overlay::onion_ghosts`] builds ghost tints from a
//! standalone `OnionConfig`, and no production call site constructs an onion
//! overlay today. The tokens are reserved for when the overlay is wired in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::core::color::Color;

/// Name of the built-in default theme (the last fallback in the chain).
const BUILTIN_THEME_NAME: &str = "Dark";

/// Neutral tint for nine-slice skin meshes: the atlas pixels are used as-is.
///
/// `theme.rs` is exempt from the hardcoded-color invariant test (it owns the
/// tokens); this constant is the single tint the skin renderer may use.
pub const SKIN_TINT: egui::Color32 = egui::Color32::WHITE;

/// All color tokens the UI needs, replacing hardcoded `egui::Color32` values.
///
/// Tokens are stored as straight-alpha `[r, g, b, a]` bytes so they serialize
/// compactly and convert to both [`egui::Color32`] and core [`Color`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ThemeColors {
    // App
    pub clear_color: [u8; 4], // RGBA

    // Canvas
    pub canvas_border: [u8; 4],
    pub selection_fill: [u8; 4],   // translucent overlay
    pub selection_stroke: [u8; 4], // bright selection outline
    pub marching_ants: [u8; 4],
    /// Fixed grey baseline painted under the animated white dashes.
    pub marching_ants_under: [u8; 4],

    // Gizmo
    pub gizmo_outline: [u8; 4],
    pub gizmo_fill_normal: [u8; 4],
    pub gizmo_fill_hover: [u8; 4],
    pub gizmo_pivot: [u8; 4],

    // Onion skin
    pub onion_prev_tint: [u8; 4],
    pub onion_next_tint: [u8; 4],

    // Panels
    pub panel_bg: [u8; 4],
    pub panel_header_bg: [u8; 4],
    pub panel_border: [u8; 4],

    // egui Selection (used by layers/timeline/toolbar for active state)
    pub selection_bg_fill: [u8; 4],
    pub selection_stroke_color: [u8; 4],
}

/// Generates a `_color32()` (egui) and `_core()` (core [`Color`]) accessor
/// pair for every color token.
macro_rules! color_accessors {
    ($(($field:ident, $egui:ident, $core:ident)),+ $(,)?) => {
        $(
            #[doc = concat!("The `", stringify!($field), "` token as an egui [`Color32`].")]
            pub fn $egui(&self) -> egui::Color32 {
                egui::Color32::from_rgba_unmultiplied(
                    self.$field[0],
                    self.$field[1],
                    self.$field[2],
                    self.$field[3],
                )
            }

            #[doc = concat!("The `", stringify!($field), "` token as a core [`Color`].")]
            pub fn $core(&self) -> Color {
                Color::from(self.$field)
            }
        )+
    };
}

impl ThemeColors {
    color_accessors! {
        (clear_color, clear_color32, clear_color_core),
        (canvas_border, canvas_border32, canvas_border_core),
        (selection_fill, selection_fill32, selection_fill_core),
        (selection_stroke, selection_stroke32, selection_stroke_core),
        (marching_ants, marching_ants32, marching_ants_core),
        (
            marching_ants_under,
            marching_ants_under32,
            marching_ants_under_core
        ),
        (gizmo_outline, gizmo_outline32, gizmo_outline_core),
        (gizmo_fill_normal, gizmo_fill_normal32, gizmo_fill_normal_core),
        (gizmo_fill_hover, gizmo_fill_hover32, gizmo_fill_hover_core),
        (gizmo_pivot, gizmo_pivot32, gizmo_pivot_core),
        (onion_prev_tint, onion_prev_tint32, onion_prev_tint_core),
        (onion_next_tint, onion_next_tint32, onion_next_tint_core),
        (panel_bg, panel_bg32, panel_bg_core),
        (panel_header_bg, panel_header_bg32, panel_header_bg_core),
        (panel_border, panel_border32, panel_border_core),
        (selection_bg_fill, selection_bg_fill32, selection_bg_fill_core),
        (selection_stroke_color, selection_stroke_color32, selection_stroke_color_core),
    }

    pub fn gizmo_colors(&self) -> crate::render::gizmo::GizmoColors {
        crate::render::gizmo::GizmoColors {
            outline: self.gizmo_outline32(),
            fill_normal: self.gizmo_fill_normal32(),
            fill_hover: self.gizmo_fill_hover32(),
            pivot: self.gizmo_pivot32(),
        }
    }
}

/// An atlas region definition: where one skin state lives in the atlas PNG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NineSliceSource {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// How many pixels from each edge define the corners.
    pub corner_size: u32,
}

/// The 9 regions a [`NineSliceSource`] divides into, in atlas pixel space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NineSlice {
    pub top_left: egui::Rect,
    pub top_center: egui::Rect,
    pub top_right: egui::Rect,
    pub middle_left: egui::Rect,
    pub center: egui::Rect,
    pub middle_right: egui::Rect,
    pub bottom_left: egui::Rect,
    pub bottom_center: egui::Rect,
    pub bottom_right: egui::Rect,
}

impl NineSlice {
    /// Divides `source` into 9 regions based on `corner_size`.
    ///
    /// Defensive clamping keeps the result valid for degenerate input: the
    /// source rect is clamped to the atlas bounds, and the corner size is
    /// clamped to half the region's width/height so the center regions never
    /// invert (a zero-size center is allowed).
    pub fn from_source(source: &NineSliceSource, atlas_size: (u32, u32)) -> Self {
        let max_w = atlas_size.0.saturating_sub(source.x);
        let max_h = atlas_size.1.saturating_sub(source.y);
        let w = source.width.min(max_w);
        let h = source.height.min(max_h);
        let corner = source.corner_size.min(w / 2).min(h / 2);

        let (x, y) = (source.x as f32, source.y as f32);
        let (w, h) = (w as f32, h as f32);
        let c = corner as f32;
        let rect = |x0: f32, y0: f32, w0: f32, h0: f32| {
            egui::Rect::from_min_size(egui::pos2(x0, y0), egui::vec2(w0, h0))
        };

        Self {
            top_left: rect(x, y, c, c),
            top_center: rect(x + c, y, w - 2.0 * c, c),
            top_right: rect(x + w - c, y, c, c),
            middle_left: rect(x, y + c, c, h - 2.0 * c),
            center: rect(x + c, y + c, w - 2.0 * c, h - 2.0 * c),
            middle_right: rect(x + w - c, y + c, c, h - 2.0 * c),
            bottom_left: rect(x, y + h - c, c, c),
            bottom_center: rect(x + c, y + h - c, w - 2.0 * c, c),
            bottom_right: rect(x + w - c, y + h - c, c, c),
        }
    }
}

/// Widget interaction state a skin can be drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SkinState {
    Normal,
    Hover,
    Pressed,
    Disabled,
}

/// A 9-slice skin: one atlas PNG plus a [`NineSliceSource`] per state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Skin {
    /// Relative path to the atlas PNG within the theme dir.
    pub atlas_path: String,
    pub normal: NineSliceSource,
    pub hover: NineSliceSource,
    pub pressed: NineSliceSource,
    pub disabled: NineSliceSource,
}

impl Skin {
    /// The atlas region for `state`.
    pub fn source_for(&self, state: SkinState) -> &NineSliceSource {
        match state {
            SkinState::Normal => &self.normal,
            SkinState::Hover => &self.hover,
            SkinState::Pressed => &self.pressed,
            SkinState::Disabled => &self.disabled,
        }
    }
}

/// A named theme: colors plus named skins (e.g. "panel", "button").
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Theme {
    pub name: String,
    pub version: String,
    pub colors: ThemeColors,
    pub skins: HashMap<String, Skin>,
}

impl Theme {
    /// The built-in dark theme with Pyxel Edit-faithful colors.
    pub fn default_dark() -> Self {
        Self {
            name: BUILTIN_THEME_NAME.to_string(),
            version: "1.0.0".to_string(),
            colors: ThemeColors {
                clear_color: [30, 30, 30, 255],
                canvas_border: [90, 90, 90, 255],
                selection_fill: [255, 255, 255, 16],
                selection_stroke: [80, 160, 255, 255],
                marching_ants: [255, 255, 255, 255],
                marching_ants_under: [96, 96, 96, 255],
                gizmo_outline: [255, 255, 255, 255],
                gizmo_fill_normal: [255, 255, 255, 180],
                gizmo_fill_hover: [100, 180, 255, 255],
                gizmo_pivot: [255, 100, 100, 255],
                onion_prev_tint: [255, 80, 80, 255],
                onion_next_tint: [80, 255, 80, 255],
                panel_bg: [42, 42, 42, 255],
                panel_header_bg: [58, 58, 58, 255],
                panel_border: [74, 74, 74, 255],
                selection_bg_fill: [58, 106, 154, 255],
                selection_stroke_color: [255, 255, 255, 255],
            },
            skins: HashMap::new(),
        }
    }

    /// The named skin, if this theme defines one.
    pub fn skin(&self, name: &str) -> Option<&Skin> {
        self.skins.get(name)
    }
}

/// Where a theme file lives and whether it ships with the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeMetadata {
    pub name: String,
    pub path: PathBuf,
    pub is_builtin: bool,
}

/// Errors from theme loading, switching, and atlas parsing.
#[derive(Debug)]
pub enum ThemeError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    AtlasLoad(String),
    NotFound(String),
    Invalid(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::Io(e) => write!(f, "theme I/O error: {e}"),
            ThemeError::Parse(e) => write!(f, "theme parse error: {e}"),
            ThemeError::AtlasLoad(msg) => write!(f, "atlas load error: {msg}"),
            ThemeError::NotFound(name) => write!(f, "theme not found: {name}"),
            ThemeError::Invalid(msg) => write!(f, "invalid theme: {msg}"),
        }
    }
}

impl std::error::Error for ThemeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ThemeError::Io(e) => Some(e),
            ThemeError::Parse(e) => Some(e),
            _ => None,
        }
    }
}

/// Owns the active theme and the set of installable themes.
///
/// Resolution fallback chain: user theme file → built-in default → hardcoded
/// defaults ([`Theme::default_dark`]). Pure data — no egui context or UI.
#[derive(Debug, Clone)]
pub struct ThemeManager {
    current: Theme,
    available: Vec<ThemeMetadata>,
    theme_dirs: Vec<PathBuf>,
    /// Directory of the loaded theme file; `None` for the hardcoded default
    /// theme, which has no file on disk.
    current_dir: Option<PathBuf>,
}

impl ThemeManager {
    /// Initializes the built-in dark theme, scans the theme dirs, and adopts
    /// the on-disk built-in theme when one is installed.
    pub fn new() -> Self {
        let mut manager = Self {
            current: Theme::default_dark(),
            available: Vec::new(),
            theme_dirs: default_theme_dirs(),
            current_dir: None,
        };
        let _ = manager.refresh();
        manager.adopt_installed_default();
        manager
    }

    /// Loads the installed theme named [`BUILTIN_THEME_NAME`] so packaged
    /// builds render with its skins and atlas instead of the code-only
    /// fallback; stays on [`Theme::default_dark`] when no file is installed.
    fn adopt_installed_default(&mut self) {
        let Some(path) = self
            .available
            .iter()
            .find(|meta| meta.name == BUILTIN_THEME_NAME && !meta.path.as_os_str().is_empty())
            .map(|meta| meta.path.clone())
        else {
            return;
        };
        let Ok(theme) = Self::load_theme(&path) else {
            return;
        };
        self.current = theme;
        self.current_dir = path.parent().map(Path::to_path_buf);
    }

    /// The active theme.
    pub fn current(&self) -> &Theme {
        &self.current
    }

    /// Directory containing the active theme's file, if it was loaded from
    /// disk. `None` for the hardcoded default.
    pub fn current_dir(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// Switch to a named theme. User themes win over the built-in default;
    /// unknown names yield [`ThemeError::NotFound`].
    ///
    /// A builtin entry with a resolved file path is loaded from disk so
    /// packaged themes contribute their skins; only the empty-path fallback
    /// entry from [`Self::refresh`] uses [`Theme::default_dark`].
    pub fn switch(&mut self, name: &str) -> Result<(), ThemeError> {
        if name == self.current.name {
            return Ok(());
        }
        let Some(meta) = self.available.iter().find(|m| m.name == name) else {
            return Err(ThemeError::NotFound(format!(
                "theme '{name}' is not installed"
            )));
        };
        let (is_builtin, path) = (meta.is_builtin, meta.path.clone());
        self.current = if is_builtin && path.as_os_str().is_empty() {
            Theme::default_dark()
        } else {
            Self::load_theme(&path)?
        };
        self.current_dir = if path.as_os_str().is_empty() {
            None
        } else {
            path.parent().map(Path::to_path_buf)
        };
        Ok(())
    }

    /// Rescan the theme dirs, replacing `available` with what is found.
    /// Returns how many themes are new since the last scan.
    pub fn refresh(&mut self) -> Result<usize, ThemeError> {
        let previous: std::collections::HashSet<String> =
            self.available.iter().map(|m| m.name.clone()).collect();
        let mut scanned: Vec<ThemeMetadata> = Vec::new();
        for dir in &self.theme_dirs {
            if !dir.is_dir() {
                continue;
            }
            let entries = std::fs::read_dir(dir).map_err(ThemeError::Io)?;
            for entry in entries {
                let entry = entry.map_err(ThemeError::Io)?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(theme) = Self::load_theme(&path) else {
                    continue; // Skip malformed theme files.
                };
                if scanned.iter().any(|m| m.name == theme.name) {
                    continue; // First dir wins (user overrides built-in).
                }
                scanned.push(ThemeMetadata {
                    name: theme.name,
                    path,
                    is_builtin: dir.ends_with("builtin"),
                });
            }
        }
        let new_count = scanned
            .iter()
            .filter(|m| !previous.contains(&m.name))
            .count();
        // The built-in default is always available, even with no theme files.
        if !scanned.iter().any(|m| m.name == BUILTIN_THEME_NAME) {
            scanned.push(ThemeMetadata {
                name: BUILTIN_THEME_NAME.to_string(),
                path: PathBuf::new(),
                is_builtin: true,
            });
        }
        self.available = scanned;
        Ok(new_count)
    }

    /// Themes available for switching.
    pub fn available(&self) -> &[ThemeMetadata] {
        &self.available
    }

    /// Load a theme from a JSON file. The theme name must be non-empty.
    pub fn load_theme(path: &Path) -> Result<Theme, ThemeError> {
        let data = std::fs::read(path).map_err(ThemeError::Io)?;
        let theme: Theme = serde_json::from_slice(&data).map_err(ThemeError::Parse)?;
        if theme.name.trim().is_empty() {
            return Err(ThemeError::Invalid(format!(
                "theme at '{}' has an empty name",
                path.display()
            )));
        }
        Ok(theme)
    }
}

impl Default for ThemeManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Theme search paths: `$PYXROSS_THEMES` (user override), package-local user
/// and builtin themes relative to the executable, then development/repo paths
/// relative to the working directory.
fn default_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("PYXROSS_THEMES") {
        dirs.push(PathBuf::from(dir));
    }
    let executable_root = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    if let Some(root) = &executable_root {
        dirs.push(root.join("themes/user"));
        dirs.push(root.join("themes/builtin"));
    }
    dirs.push(PathBuf::from("themes/user"));
    dirs.push(PathBuf::from("themes/builtin"));
    dirs
}

/// Reads a PNG atlas and returns raw straight-alpha RGBA pixel data plus the
/// atlas dimensions.
///
/// Any PNG color type (palette/indexed, grayscale, grayscale+alpha, RGB,
/// RGBA; 1/2/4/8/16-bit) is normalized to 8-bit RGBA. The dimensions are
/// passed separately to [`NineSlice::from_source`].
pub fn load_atlas_with_size(path: &Path) -> Result<(Vec<u8>, (u32, u32)), ThemeError> {
    let data = std::fs::read(path).map_err(ThemeError::Io)?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| ThemeError::AtlasLoad(e.to_string()))?;
    let buf_len = reader
        .output_buffer_size()
        .ok_or_else(|| ThemeError::AtlasLoad("output buffer size overflow".to_string()))?;
    let mut buf = vec![0u8; buf_len];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| ThemeError::AtlasLoad(e.to_string()))?;
    reader
        .finish()
        .map_err(|e| ThemeError::AtlasLoad(e.to_string()))?;

    let (color_type, bit_depth) = reader.output_color_type();
    if bit_depth != png::BitDepth::Eight {
        return Err(ThemeError::AtlasLoad(format!(
            "unsupported output bit depth after normalization: {bit_depth:?}"
        )));
    }
    let width = frame.width as usize;
    let height = frame.height as usize;
    let mut rgba = Vec::with_capacity(width * height * 4);
    match color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(&buf),
        png::ColorType::Rgb => {
            for px in buf.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in &buf {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in buf.chunks_exact(2) {
                rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        png::ColorType::Indexed => {
            return Err(ThemeError::AtlasLoad(
                "indexed color type was not expanded".to_string(),
            ));
        }
    }
    Ok((rgba, (width as u32, height as u32)))
}

/// Reads a PNG atlas and returns raw straight-alpha RGBA pixel data.
///
/// Thin wrapper over [`load_atlas_with_size`] for callers that only need the
/// pixels; the atlas dimensions are passed separately to
/// [`NineSlice::from_source`].
pub fn load_atlas(path: &Path) -> Result<Vec<u8>, ThemeError> {
    load_atlas_with_size(path).map(|(bytes, _)| bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: &Path) -> Self {
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
        fn path(&self) -> &Path {
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
            std::env::temp_dir().join(format!("pyxross_theme_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    fn sample_theme(name: &str) -> Theme {
        let mut theme = Theme::default_dark();
        theme.name = name.to_string();
        theme
    }

    fn sample_skin() -> Skin {
        let src = |x: u32| NineSliceSource {
            x,
            y: 0,
            width: 32,
            height: 32,
            corner_size: 8,
        };
        Skin {
            atlas_path: "atlas.png".to_string(),
            normal: src(0),
            hover: src(32),
            pressed: src(64),
            disabled: src(96),
        }
    }

    fn rgba_png(width: u32, height: u32, pixel: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let data = vec![pixel; (width * height) as usize]
                .into_iter()
                .flatten()
                .collect::<Vec<u8>>();
            writer.write_image_data(&data).expect("png data");
            writer.finish().expect("png finish");
        }
        out
    }

    fn rgb_png(width: u32, height: u32, pixel: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            let data = vec![pixel; (width * height) as usize]
                .into_iter()
                .flatten()
                .collect::<Vec<u8>>();
            writer.write_image_data(&data).expect("png data");
            writer.finish().expect("png finish");
        }
        out
    }

    #[test]
    fn theme_colors_roundtrip() {
        let colors = Theme::default_dark().colors;
        let json = serde_json::to_string(&colors).unwrap();
        let back: ThemeColors = serde_json::from_str(&json).unwrap();
        assert_eq!(back, colors);
    }

    #[test]
    fn theme_roundtrip() {
        let theme = sample_theme("Roundtrip");
        let json = serde_json::to_string(&theme).unwrap();
        let back: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(back, theme);
    }

    #[test]
    fn shipped_default_theme_asset_matches_default_dark() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/themes/default/theme.json");
        let theme = ThemeManager::load_theme(&path).expect("shipped default theme must parse");
        assert_eq!(theme.name, Theme::default_dark().name);
        assert_eq!(
            theme.colors,
            Theme::default_dark().colors,
            "assets/themes/default/theme.json must stay in sync with Theme::default_dark()"
        );
    }

    #[test]
    fn nine_slice_source_roundtrip() {
        let source = NineSliceSource {
            x: 4,
            y: 8,
            width: 64,
            height: 32,
            corner_size: 8,
        };
        let json = serde_json::to_string(&source).unwrap();
        let back: NineSliceSource = serde_json::from_str(&json).unwrap();
        assert_eq!(back, source);
    }

    #[test]
    fn skin_roundtrip() {
        let skin = sample_skin();
        let json = serde_json::to_string(&skin).unwrap();
        let back: Skin = serde_json::from_str(&json).unwrap();
        assert_eq!(back, skin);
    }

    #[test]
    fn skin_state_roundtrip() {
        for state in [
            SkinState::Normal,
            SkinState::Hover,
            SkinState::Pressed,
            SkinState::Disabled,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let back: SkinState = serde_json::from_str(&json).unwrap();
            assert_eq!(back, state);
        }
    }

    #[test]
    fn nine_slice_from_source_splits_into_nine_regions() {
        let source = NineSliceSource {
            x: 10,
            y: 20,
            width: 40,
            height: 30,
            corner_size: 5,
        };
        let ns = NineSlice::from_source(&source, (100, 100));
        assert_eq!(ns.top_left, rect(10.0, 20.0, 5.0, 5.0));
        assert_eq!(ns.top_center, rect(15.0, 20.0, 30.0, 5.0));
        assert_eq!(ns.top_right, rect(45.0, 20.0, 5.0, 5.0));
        assert_eq!(ns.middle_left, rect(10.0, 25.0, 5.0, 20.0));
        assert_eq!(ns.center, rect(15.0, 25.0, 30.0, 20.0));
        assert_eq!(ns.middle_right, rect(45.0, 25.0, 5.0, 20.0));
        assert_eq!(ns.bottom_left, rect(10.0, 45.0, 5.0, 5.0));
        assert_eq!(ns.bottom_center, rect(15.0, 45.0, 30.0, 5.0));
        assert_eq!(ns.bottom_right, rect(45.0, 45.0, 5.0, 5.0));
    }

    #[test]
    fn nine_slice_clamps_corner_to_half() {
        let source = NineSliceSource {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            corner_size: 99,
        };
        let ns = NineSlice::from_source(&source, (10, 10));
        // Corner clamped to 5 (half of 10); the center collapses to zero size.
        assert_eq!(ns.top_left, rect(0.0, 0.0, 5.0, 5.0));
        assert_eq!(ns.center, rect(5.0, 5.0, 0.0, 0.0));
        assert_eq!(ns.bottom_right, rect(5.0, 5.0, 5.0, 5.0));
    }

    #[test]
    fn nine_slice_clamps_to_atlas_bounds() {
        let source = NineSliceSource {
            x: 90,
            y: 90,
            width: 40,
            height: 40,
            corner_size: 4,
        };
        let ns = NineSlice::from_source(&source, (100, 100));
        // Region clamped to 10x10 at (90, 90).
        assert_eq!(ns.top_left, rect(90.0, 90.0, 4.0, 4.0));
        assert_eq!(ns.bottom_right, rect(96.0, 96.0, 4.0, 4.0));
    }

    #[test]
    fn nine_slice_zero_corner() {
        let source = NineSliceSource {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
            corner_size: 0,
        };
        let ns = NineSlice::from_source(&source, (16, 16));
        assert_eq!(ns.top_left, rect(0.0, 0.0, 0.0, 0.0));
        assert_eq!(ns.center, rect(0.0, 0.0, 16.0, 16.0));
        assert_eq!(ns.bottom_right, rect(16.0, 16.0, 0.0, 0.0));
    }

    #[test]
    fn default_dark_has_expected_colors() {
        let theme = Theme::default_dark();
        assert_eq!(theme.name, "Dark");
        assert_eq!(theme.colors.clear_color, [30, 30, 30, 255]);
        assert_eq!(theme.colors.selection_fill, [255, 255, 255, 16]);
        assert_eq!(theme.colors.selection_stroke, [80, 160, 255, 255]);
        assert_eq!(theme.colors.marching_ants, [255, 255, 255, 255]);
        assert_eq!(theme.colors.marching_ants_under, [96, 96, 96, 255]);
        assert_eq!(theme.colors.panel_bg, [42, 42, 42, 255]);
        assert_eq!(theme.colors.selection_bg_fill, [58, 106, 154, 255]);
        assert_eq!(theme.colors.selection_stroke_color, [255, 255, 255, 255]);
        assert!(theme.skins.is_empty());
    }

    #[test]
    fn theme_colors_accessors() {
        let colors = Theme::default_dark().colors;
        assert_eq!(
            colors.clear_color32(),
            egui::Color32::from_rgba_unmultiplied(30, 30, 30, 255)
        );
        assert_eq!(colors.clear_color_core(), Color::rgb(30, 30, 30));
        assert_eq!(
            colors.selection_stroke32(),
            egui::Color32::from_rgba_unmultiplied(80, 160, 255, 255)
        );
        assert_eq!(colors.selection_stroke_core(), Color::rgb(80, 160, 255));
        assert_eq!(
            colors.selection_fill32(),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 16)
        );
        assert_eq!(colors.selection_fill_core(), Color::rgba(255, 255, 255, 16));
        assert_eq!(
            colors.marching_ants32(),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 255)
        );
        assert_eq!(
            colors.marching_ants_under32(),
            egui::Color32::from_rgba_unmultiplied(96, 96, 96, 255)
        );
        assert_eq!(
            colors.marching_ants_under_core(),
            Color::rgba(96, 96, 96, 255)
        );
    }

    #[test]
    fn skin_source_for_maps_states() {
        let skin = sample_skin();
        assert_eq!(skin.source_for(SkinState::Normal), &skin.normal);
        assert_eq!(skin.source_for(SkinState::Hover), &skin.hover);
        assert_eq!(skin.source_for(SkinState::Pressed), &skin.pressed);
        assert_eq!(skin.source_for(SkinState::Disabled), &skin.disabled);
    }

    #[test]
    fn theme_error_display() {
        let io = ThemeError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
        assert!(io.to_string().contains("theme I/O error"));
        let parse = ThemeError::Parse(serde_json::from_str::<Theme>("{").unwrap_err());
        assert!(parse.to_string().contains("theme parse error"));
        let atlas = ThemeError::AtlasLoad("bad png".to_string());
        assert_eq!(atlas.to_string(), "atlas load error: bad png");
        let not_found = ThemeError::NotFound("Nope".to_string());
        assert_eq!(not_found.to_string(), "theme not found: Nope");
        let invalid = ThemeError::Invalid("empty name".to_string());
        assert_eq!(invalid.to_string(), "invalid theme: empty name");
    }

    #[test]
    fn load_atlas_decodes_rgba_png() {
        let dir = temp_dir("atlas_rgba");
        let path = dir.path().join("atlas.png");
        std::fs::write(&path, rgba_png(32, 32, [10, 20, 30, 40])).unwrap();
        let pixels = load_atlas(&path).unwrap();
        assert_eq!(pixels.len(), 32 * 32 * 4);
        assert_eq!(&pixels[..4], &[10, 20, 30, 40]);
        assert_eq!(&pixels[4..8], &[10, 20, 30, 40]);
    }

    #[test]
    fn load_atlas_expands_rgb_png() {
        let dir = temp_dir("atlas_rgb");
        let path = dir.path().join("atlas.png");
        std::fs::write(&path, rgb_png(4, 4, [1, 2, 3])).unwrap();
        let pixels = load_atlas(&path).unwrap();
        assert_eq!(pixels.len(), 4 * 4 * 4);
        assert_eq!(&pixels[..4], &[1, 2, 3, 255]);
    }

    #[test]
    fn load_atlas_missing_file_errors() {
        let err = load_atlas(Path::new("/nonexistent/atlas.png")).unwrap_err();
        assert!(matches!(err, ThemeError::Io(_)));
    }

    #[test]
    fn theme_manager_new_uses_default_dark() {
        let manager = ThemeManager::new();
        assert_eq!(manager.current().name, "Dark");
        assert!(manager
            .available()
            .iter()
            .any(|m| m.name == "Dark" && m.is_builtin));
    }

    #[test]
    fn theme_manager_adopts_installed_default_with_skins() {
        let dir = temp_dir("adopt_default");
        let mut theme = sample_theme("Dark");
        theme.skins.insert("panel_bg".to_string(), sample_skin());
        std::fs::write(
            dir.path().join("theme.json"),
            serde_json::to_vec(&theme).unwrap(),
        )
        .unwrap();

        let mut manager = ThemeManager::new();
        manager.theme_dirs = vec![dir.path().to_path_buf()];
        manager.refresh().unwrap();
        manager.adopt_installed_default();

        assert_eq!(manager.current().name, "Dark");
        assert!(!manager.current().skins.is_empty());
        assert_eq!(manager.current_dir(), Some(dir.path()));
    }

    #[test]
    fn theme_manager_switch_valid_and_invalid() {
        let mut manager = ThemeManager::new();
        assert!(manager.switch("Dark").is_ok());
        let err = manager.switch("DoesNotExist").unwrap_err();
        assert!(matches!(err, ThemeError::NotFound(_)));
    }

    #[test]
    fn theme_manager_load_theme_from_json() {
        let dir = temp_dir("load_theme");
        let path = dir.path().join("theme.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&sample_theme("JsonTheme")).unwrap(),
        )
        .unwrap();
        let theme = ThemeManager::load_theme(&path).unwrap();
        assert_eq!(theme.name, "JsonTheme");
    }

    #[test]
    fn theme_manager_switch_to_user_theme() {
        let dir = temp_dir("switch_user");
        let path = dir.path().join("user_theme.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&sample_theme("UserTheme")).unwrap(),
        )
        .unwrap();
        let mut manager = ThemeManager::new();
        manager.theme_dirs = vec![dir.path().to_path_buf()];
        let new_count = manager.refresh().unwrap();
        assert_eq!(new_count, 1);
        manager.switch("UserTheme").unwrap();
        assert_eq!(manager.current().name, "UserTheme");
        // Switching back to the built-in default works.
        manager.switch("Dark").unwrap();
        assert_eq!(manager.current().name, "Dark");
    }

    #[test]
    fn load_atlas_with_size_returns_dimensions() {
        let dir = temp_dir("atlas_dims");
        let path = dir.path().join("atlas.png");
        std::fs::write(&path, rgba_png(32, 48, [1, 2, 3, 4])).unwrap();
        let (pixels, (width, height)) = load_atlas_with_size(&path).unwrap();
        assert_eq!((width, height), (32, 48));
        assert_eq!(pixels.len(), 32 * 48 * 4);
        assert_eq!(&pixels[..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn load_atlas_wrapper_matches_with_size() {
        let dir = temp_dir("atlas_wrapper");
        let path = dir.path().join("atlas.png");
        std::fs::write(&path, rgba_png(16, 16, [9, 8, 7, 6])).unwrap();
        let (pixels, (width, height)) = load_atlas_with_size(&path).unwrap();
        assert_eq!(load_atlas(&path).unwrap(), pixels);
        assert_eq!((width, height), (16, 16));
    }

    #[test]
    fn theme_manager_current_dir_none_for_default() {
        let manager = ThemeManager::new();
        assert_eq!(manager.current_dir(), None);
    }

    #[test]
    fn theme_manager_current_dir_tracks_loaded_theme_file() {
        let dir = temp_dir("current_dir");
        std::fs::write(
            dir.path().join("dir_theme.json"),
            serde_json::to_vec(&sample_theme("DirTheme")).unwrap(),
        )
        .unwrap();
        let mut manager = ThemeManager::new();
        manager.theme_dirs = vec![dir.path().to_path_buf()];
        manager.refresh().unwrap();
        manager.switch("DirTheme").unwrap();
        assert_eq!(manager.current_dir(), Some(dir.path()));
        // The empty-path fallback entry clears it again.
        manager.switch("Dark").unwrap();
        assert_eq!(manager.current_dir(), None);
    }

    /// A builtin theme with a resolved file path loads from disk (skins
    /// included); only the empty-path fallback entry uses `default_dark`.
    #[test]
    fn theme_manager_builtin_with_file_loads_skins() {
        let dir = temp_dir("builtin_skins");
        let user_dir = dir.path().join("user");
        let builtin_dir = dir.path().join("builtin");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&builtin_dir).unwrap();
        std::fs::write(
            user_dir.join("other.json"),
            serde_json::to_vec(&sample_theme("Other")).unwrap(),
        )
        .unwrap();
        let mut builtin = sample_theme("Dark");
        builtin.skins.insert("panel_bg".to_string(), sample_skin());
        std::fs::write(
            builtin_dir.join("dark.json"),
            serde_json::to_vec(&builtin).unwrap(),
        )
        .unwrap();

        let mut manager = ThemeManager::new();
        manager.theme_dirs = vec![user_dir.clone(), builtin_dir.clone()];
        manager.refresh().unwrap();
        // Switch away first so the same-name early return does not skip the load.
        manager.switch("Other").unwrap();
        assert_eq!(manager.current_dir(), Some(user_dir.as_path()));
        manager.switch("Dark").unwrap();
        assert_eq!(manager.current().name, "Dark");
        assert!(
            manager.current().skins.contains_key("panel_bg"),
            "builtin theme with a file must load its skins"
        );
        assert_eq!(manager.current_dir(), Some(builtin_dir.as_path()));
    }

    #[test]
    fn theme_manager_refresh_counts_new_themes() {
        let dir = temp_dir("refresh_count");
        let mut manager = ThemeManager::new();
        manager.theme_dirs = vec![dir.path().to_path_buf()];
        assert_eq!(manager.refresh().unwrap(), 0);
        std::fs::write(
            dir.path().join("a.json"),
            serde_json::to_vec(&sample_theme("ThemeA")).unwrap(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.json"),
            serde_json::to_vec(&sample_theme("ThemeB")).unwrap(),
        )
        .unwrap();
        assert_eq!(manager.refresh().unwrap(), 2);
        // Re-scanning finds nothing new.
        assert_eq!(manager.refresh().unwrap(), 0);
    }

    #[test]
    fn load_theme_rejects_empty_name() {
        let dir = temp_dir("empty_name");
        let path = dir.path().join("bad.json");
        let mut theme = sample_theme("Bad");
        theme.name = "  ".to_string();
        std::fs::write(&path, serde_json::to_vec(&theme).unwrap()).unwrap();
        let err = ThemeManager::load_theme(&path).unwrap_err();
        assert!(matches!(err, ThemeError::Invalid(_)));
    }

    /// `PYXROSS_THEMES` env override appears first in `default_theme_dirs()`.
    #[test]
    fn theme_manager_pyxross_themes_env_dir_wins() {
        let dir = temp_dir("env_dirs");
        let _env = EnvVarGuard::set("PYXROSS_THEMES", dir.path());
        let dirs = default_theme_dirs();
        assert_eq!(dirs[0], dir.path());
        assert!(dirs.len() >= 3);
    }

    #[test]
    fn theme_manager_includes_executable_resource_root() {
        let executable = std::env::current_exe().unwrap();
        let root = executable.parent().unwrap();
        let dirs = default_theme_dirs();

        assert!(dirs.contains(&root.join("themes/user")));
        assert!(dirs.contains(&root.join("themes/builtin")));
    }

    /// When two theme dirs contain a theme with the same name the first dir
    /// wins (user overrides built-in).
    #[test]
    fn theme_manager_user_theme_overrides_builtin_same_name() {
        let user_dir = temp_dir("user_wins");
        let builtin_dir = temp_dir("builtin_loses");

        // User "Dark" with a distinct panel_bg.
        let mut user_theme = sample_theme("Dark");
        user_theme.colors.panel_bg = [9, 9, 9, 255];
        std::fs::write(
            user_dir.path().join("dark_user.json"),
            serde_json::to_vec(&user_theme).unwrap(),
        )
        .unwrap();

        // Built-in "Dark" with the default panel_bg.
        let builtin_theme = sample_theme("Dark");
        std::fs::write(
            builtin_dir.path().join("dark_builtin.json"),
            serde_json::to_vec(&builtin_theme).unwrap(),
        )
        .unwrap();

        // A second user theme so switching away from "Dark" forces a reload.
        std::fs::write(
            user_dir.path().join("other.json"),
            serde_json::to_vec(&sample_theme("Other")).unwrap(),
        )
        .unwrap();

        let mut manager = ThemeManager::new();
        manager.theme_dirs = vec![
            user_dir.path().to_path_buf(),
            builtin_dir.path().to_path_buf(),
        ];
        let new_count = manager.refresh().unwrap();
        assert_eq!(new_count, 1); // "Other" is new; "Dark" was already known

        // Exactly one "Dark" entry, and it points inside user_dir.
        let dark_entries: Vec<_> = manager
            .available()
            .iter()
            .filter(|m| m.name == "Dark")
            .collect();
        assert_eq!(dark_entries.len(), 1);
        assert!(dark_entries[0].path.starts_with(user_dir.path()));

        // Switching loads the user version (switch away first so the
        // same-name early return in `switch` does not skip the reload).
        manager.switch("Other").unwrap();
        manager.switch("Dark").unwrap();
        assert_eq!(manager.current().colors.panel_bg, [9, 9, 9, 255]);
    }

    /// Malformed JSON files are silently skipped; a good file in the same dir
    /// still loads, and the built-in fallback is always present.
    #[test]
    fn theme_manager_refresh_skips_malformed_json() {
        let dir = temp_dir("malformed_skip");

        // Write a broken file and a valid one.
        std::fs::write(dir.path().join("broken.json"), b"{ not valid json !!").unwrap();
        std::fs::write(
            dir.path().join("good.json"),
            serde_json::to_vec(&sample_theme("GoodOne")).unwrap(),
        )
        .unwrap();

        let mut manager = ThemeManager::new();
        manager.theme_dirs = vec![dir.path().to_path_buf()];
        let count = manager.refresh().unwrap();
        assert_eq!(count, 1); // only GoodOne is new

        // GoodOne is available, "broken" is not, built-in "Dark" is still present.
        let names: Vec<&str> = manager
            .available()
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert!(names.contains(&"GoodOne"));
        assert!(!names.contains(&"broken"));
        assert!(names.contains(&"Dark"));

        manager.switch("GoodOne").unwrap();
        assert_eq!(manager.current().name, "GoodOne");
    }

    #[test]
    fn no_hardcoded_chrome_colors_in_ui_or_render() {
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for dir in ["src/ui", "src/render"] {
            let dir = manifest_dir.join(dir);
            let mut entries: Vec<_> = std::fs::read_dir(&dir)
                .expect("scan dir")
                .map(|e| e.expect("entry").path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("rs"))
                .collect();
            entries.sort();
            files.extend(entries);
        }

        let patterns = [
            "Color32::from_rgb(",
            "Color32::from_rgba_unmultiplied(",
            "Color32::from_rgba_premultiplied(",
            "Color32::from_gray(",
            "Color32::from_white_alpha(",
            "Color32::from_black_alpha(",
            "Color32::from_luminance(",
            "Color32::from_additive_luminance(",
            "Color32::WHITE",
            "Color32::BLACK",
            "Color32::RED",
            "Color32::GREEN",
            "Color32::BLUE",
            "Color32::YELLOW",
            "Color32::GRAY",
            "Color32::LIGHT_GRAY",
            "Color32::DARK_GRAY",
        ];

        let mut violations: Vec<String> = Vec::new();
        for file in files {
            let name = file.file_name().unwrap().to_string_lossy();
            if name == "theme.rs" {
                continue;
            }
            let src = std::fs::read_to_string(&file).expect("read file");
            // All scanned files use `#[cfg(test)]` as the test module marker;
            // `// ----` banners are production section separators, NOT test
            // markers (verified by grep across the codebase).
            let split_at = src.find("#[cfg(test)]");
            let prod = split_at.map(|i| &src[..i]).unwrap_or(&src);

            let mut in_gizmo_default = false;
            for (idx, line) in prod.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("impl Default for GizmoColors") {
                    in_gizmo_default = true;
                    continue;
                }
                if in_gizmo_default {
                    if trimmed == "}" {
                        in_gizmo_default = false;
                    }
                    continue;
                }
                if line.contains("tint.r, tint.g, tint.b") {
                    continue;
                }
                if line.contains("color.r, color.g, color.b") {
                    continue;
                }
                if line.contains("// NB: neutral texture tint") {
                    continue;
                }
                if line.contains("// NB: fixed mask border") {
                    continue;
                }
                for p in patterns {
                    if line.contains(p) {
                        violations.push(format!("{}:{}: {}", name, idx + 1, trimmed));
                        break;
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "hardcoded chrome colors in production code (route through ThemeColors):\n{}",
            violations.join("\n")
        );
    }
}
