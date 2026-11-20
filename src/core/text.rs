//! Crisp, non-antialiased pixel-art text layout & rasterization.
//!
//! Supports:
//! - Built-in `"Pixel 5x7"` 1-bit bitmap font (crisp at integer multiples).
//! - Bundled `"JetBrains Mono"` TTF font.
//! - System-installed `.ttf` / `.otf` fonts discovered across Linux, Windows,
//!   and macOS font directories, rasterized via `ab_glyph` with binary alpha
//!   thresholding (zero anti-aliasing).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

pub const BUILTIN_PIXEL_FONT_NAME: &str = "Pixel 5x7";
pub const BUNDLED_MONO_FONT_NAME: &str = "JetBrains Mono";
const BUNDLED_MONO_BYTES: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMonoNLNerdFont-Regular.ttf");

/// A discoverable font entry (built-in or on-disk).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FontEntry {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// Discovers available fonts: built-in `"Pixel 5x7"`, bundled `"JetBrains Mono"`,
/// and all system-installed `.ttf`/`.otf` fonts sorted case-insensitively.
pub fn discover_fonts() -> Vec<FontEntry> {
    let mut out = vec![
        FontEntry {
            name: BUILTIN_PIXEL_FONT_NAME.to_owned(),
            path: None,
        },
        FontEntry {
            name: BUNDLED_MONO_FONT_NAME.to_owned(),
            path: None,
        },
    ];

    // Map lowercase key -> (Display Name, Path) for case-insensitive dedup + sort.
    let mut by_key: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();

    #[cfg(target_os = "linux")]
    collect_fonts_from_fc_list(&mut by_key);

    for dir in system_font_dirs() {
        collect_fonts_recursive(&dir, 0, &mut by_key);
    }

    for (_key, (name, path)) in by_key {
        if !name.eq_ignore_ascii_case(BUILTIN_PIXEL_FONT_NAME)
            && !name.eq_ignore_ascii_case(BUNDLED_MONO_FONT_NAME)
        {
            out.push(FontEntry {
                name,
                path: Some(path),
            });
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn collect_fonts_from_fc_list(out: &mut BTreeMap<String, (String, PathBuf)>) {
    let Ok(output) = std::process::Command::new("fc-list")
        .args(["--format", "%{family}\t%{file}\n"])
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let Some((families, file)) = line.split_once('\t') else {
            continue;
        };
        let path = PathBuf::from(file.trim());
        let is_supported = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"));
        if !is_supported || !path.is_file() {
            continue;
        }
        let family = families.split(',').next().unwrap_or("").trim();
        if family.is_empty() {
            continue;
        }
        let key = family.to_lowercase();
        out.entry(key).or_insert_with(|| (family.to_owned(), path));
    }
}

fn system_font_dirs() -> Vec<PathBuf> {
    let mut dirs_list = Vec::new();
    #[cfg(target_os = "linux")]
    {
        dirs_list.push(PathBuf::from("/usr/share/fonts"));
        dirs_list.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            dirs_list.push(home.join(".local/share/fonts"));
            dirs_list.push(home.join(".fonts"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(win) = std::env::var_os("WINDIR") {
            dirs_list.push(PathBuf::from(win).join("Fonts"));
        } else {
            dirs_list.push(PathBuf::from(r"C:\Windows\Fonts"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs_list.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs_list.push(PathBuf::from("/System/Library/Fonts"));
        dirs_list.push(PathBuf::from("/Library/Fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs_list.push(PathBuf::from(home).join("Library/Fonts"));
        }
    }
    dirs_list
}

fn collect_fonts_recursive(
    dir: &Path,
    depth: usize,
    out: &mut BTreeMap<String, (String, PathBuf)>,
) {
    if depth > 4 || !dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fonts_recursive(&path, depth + 1, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    let clean = stem.replace('_', " ");
                    let key = clean.to_lowercase();
                    out.entry(key).or_insert((clean, path));
                }
            }
        }
    }
}

/// Loads raw font bytes for `font_name` using `fonts` list, returning `None`
/// for the built-in `"Pixel 5x7"` bitmap font.
pub fn load_font_bytes(font_name: &str, fonts: &[FontEntry]) -> Option<Vec<u8>> {
    if font_name == BUILTIN_PIXEL_FONT_NAME {
        return None;
    }
    if font_name == BUNDLED_MONO_FONT_NAME {
        return Some(BUNDLED_MONO_BYTES.to_vec());
    }
    if let Some(entry) = fonts.iter().find(|f| f.name == font_name) {
        if let Some(path) = &entry.path {
            if let Ok(bytes) = std::fs::read(path) {
                return Some(bytes);
            }
        }
    }
    None
}

/// Horizontal alignment for multi-line text rasterization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

fn align_offset(align: TextAlign, max_content_w: i32, line_w: i32) -> i32 {
    let slack = (max_content_w - line_w).max(0);
    match align {
        TextAlign::Left => 0,
        TextAlign::Center => slack / 2,
        TextAlign::Right => slack,
    }
}

/// Parameters for [`render_text`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextRenderParams {
    pub text: String,
    pub font_name: String,
    pub font_size: u32,
    pub color: [u8; 4],
    pub align: TextAlign,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineStops {
    pub y: i32,
    /// `(x_px, char_index_in_text)` for each character start + line end.
    pub stops: Vec<(i32, usize)>,
}

/// Rasterized text bitmap + exact caret/hit-test metrics in local pixels.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextLayout {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub line_height: i32,
    pub lines: Vec<LineStops>,
}

impl TextLayout {
    /// Returns `(x, y, height)` for `caret` (char index) in local unrotated
    /// source-buffer coordinates.
    pub fn caret_rect(&self, caret: usize) -> (f32, f32, f32) {
        let h = self.line_height.max(4) as f32;
        for line in &self.lines {
            for &(x, idx) in &line.stops {
                if idx == caret {
                    return (x as f32, line.y as f32, h);
                }
            }
            if let (Some(&(_, first)), Some(&(last_x, last_idx))) =
                (line.stops.first(), line.stops.last())
            {
                if caret >= first && caret <= last_idx {
                    return (last_x as f32, line.y as f32, h);
                }
            }
        }
        (1.0, 0.0, h)
    }

    /// Maps local source-buffer coordinates `(lx, ly)` to the nearest character
    /// index between glyphs.
    pub fn hit_test_caret(&self, lx: f32, ly: f32) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let line = self
            .lines
            .iter()
            .min_by_key(|l| {
                let cy = l.y as f32 + self.line_height as f32 * 0.5;
                ((ly - cy).abs() * 100.0) as i64
            })
            .unwrap();
        line.stops
            .iter()
            .min_by_key(|&&(x, _)| ((lx - x as f32).abs() * 100.0) as i64)
            .map(|&(_, idx)| idx)
            .unwrap_or(0)
    }

    /// Moves `caret` up (`delta_lines < 0`) or down (`delta_lines > 0`) keeping
    /// the horizontal pixel offset as close as possible.
    pub fn move_caret_vertical(&self, caret: usize, delta_lines: isize) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let mut cur_line = 0usize;
        let mut cur_x = 0i32;
        for (li, line) in self.lines.iter().enumerate() {
            if let Some(&(x, _)) = line.stops.iter().find(|&&(_, idx)| idx == caret) {
                cur_line = li;
                cur_x = x;
                break;
            }
        }
        let target_line = (cur_line as isize + delta_lines)
            .clamp(0, self.lines.len() as isize - 1) as usize;
        self.lines[target_line]
            .stops
            .iter()
            .min_by_key(|&&(x, _)| (x - cur_x).abs())
            .map(|&(_, idx)| idx)
            .unwrap_or(caret)
    }
}

/// Renders `params` into a crisp, non-antialiased RGBA8 [`TextLayout`].
pub fn render_text(params: &TextRenderParams, font_bytes: Option<&[u8]>) -> TextLayout {
    if let Some(bytes) = font_bytes {
        if let Ok(font) = FontRef::try_from_slice(bytes) {
            return render_ab_glyph(&font, params);
        }
    }
    if params.font_name == BUNDLED_MONO_FONT_NAME {
        if let Ok(font) = FontRef::try_from_slice(BUNDLED_MONO_BYTES) {
            return render_ab_glyph(&font, params);
        }
    }
    render_pixel_5x7(params)
}

fn render_ab_glyph(font: &FontRef<'_>, params: &TextRenderParams) -> TextLayout {
    let size = params.font_size.clamp(5, 256) as f32;
    let scaled = font.as_scaled(PxScale::from(size));
    let ascent = scaled.ascent().ceil() as i32;
    let descent = scaled.descent().floor() as i32;
    let line_gap = scaled.line_gap().ceil() as i32;
    let line_height = (ascent - descent + line_gap).max(size.ceil() as i32).max(6);
    let pad = 2i32;

    let mut lines: Vec<LineStops> = Vec::new();
    let mut line_glyphs: Vec<(usize, ab_glyph::OutlinedGlyph)> = Vec::new();
    let mut max_w = 10i32;

    let mut char_idx = 0usize;
    let raw_lines: Vec<&str> = params.text.split('\n').collect();
    for (li, line_str) in raw_lines.iter().enumerate() {
        let y_top = li as i32 * line_height;
        let baseline_y = y_top + ascent;
        let mut cursor_x = pad as f32;
        let mut prev_gid = None;
        let mut stops = Vec::new();

        for ch in line_str.chars() {
            stops.push((cursor_x.round() as i32, char_idx));
            let gid = scaled.glyph_id(ch);
            if let Some(prev) = prev_gid {
                cursor_x += scaled.kern(prev, gid);
            }
            let glyph = gid.with_scale_and_position(
                PxScale::from(size),
                ab_glyph::point(cursor_x, baseline_y as f32),
            );
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                max_w = max_w.max(bounds.max.x.ceil() as i32 + pad);
                line_glyphs.push((li, outlined));
            }
            cursor_x += scaled.h_advance(gid);
            max_w = max_w.max(cursor_x.ceil() as i32 + pad);
            prev_gid = Some(gid);
            char_idx += 1;
        }
        stops.push((cursor_x.round() as i32, char_idx));
        lines.push(LineStops { y: y_top, stops });
        char_idx += 1; // account for '\n'
    }

    let width = max_w.max(8) as u32;
    let max_content_w = (width as i32 - 2 * pad).max(0);
    let mut line_dx = vec![0i32; lines.len()];
    for (li, line) in lines.iter_mut().enumerate() {
        let line_w = line
            .stops
            .last()
            .map(|&(x, _)| (x - pad).max(0))
            .unwrap_or(0);
        let dx = align_offset(params.align, max_content_w, line_w);
        line_dx[li] = dx;
        if dx != 0 {
            for (x, _) in &mut line.stops {
                *x += dx;
            }
        }
    }

    let height = ((raw_lines.len() as i32) * line_height + pad).max(line_height) as u32;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let color = [params.color[0], params.color[1], params.color[2], 255];

    for (li, outlined) in line_glyphs {
        let ox = line_dx[li];
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, coverage| {
            // Hard binary threshold -> zero anti-aliasing.
            if coverage < 0.45 {
                return;
            }
            let px = bounds.min.x.floor() as i32 + gx as i32 + ox;
            let py = bounds.min.y.floor() as i32 + gy as i32;
            if px >= 0 && py >= 0 && (px as u32) < width && (py as u32) < height {
                let idx = ((py as u32 * width + px as u32) * 4) as usize;
                rgba[idx..idx + 4].copy_from_slice(&color);
            }
        });
    }

    TextLayout {
        width,
        height,
        rgba,
        line_height,
        lines,
    }
}

fn render_pixel_5x7(params: &TextRenderParams) -> TextLayout {
    let scale = (params.font_size / 7).clamp(1, 32) as i32;
    let char_w = 5 * scale;
    let advance = 6 * scale;
    let line_height = 8 * scale;
    let pad = scale;

    let raw_lines: Vec<&str> = params.text.split('\n').collect();
    let max_chars = raw_lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as i32;
    let width = (max_chars * advance + pad * 2).max(8 * scale) as u32;
    let height = ((raw_lines.len() as i32) * line_height + pad).max(line_height) as u32;

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let color = [params.color[0], params.color[1], params.color[2], 255];
    let mut lines = Vec::new();
    let mut char_idx = 0usize;

    let max_content_w = (max_chars * advance).max(0);
    for (li, line_str) in raw_lines.iter().enumerate() {
        let y0 = li as i32 * line_height + pad;
        let line_w = line_str.chars().count() as i32 * advance;
        let dx = align_offset(params.align, max_content_w, line_w);
        let mut x0 = pad + dx;
        let mut stops = Vec::new();
        for ch in line_str.chars() {
            stops.push((x0, char_idx));
            let bitmap = glyph_5x7(ch);
            for (row, &bits) in bitmap.iter().enumerate() {
                for col in 0..5 {
                    if (bits >> (4 - col)) & 1 != 0 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                let px = x0 + col * scale + sx;
                                let py = y0 + row as i32 * scale + sy;
                                if px >= 0
                                    && py >= 0
                                    && (px as u32) < width
                                    && (py as u32) < height
                                {
                                    let idx = ((py as u32 * width + px as u32) * 4) as usize;
                                    rgba[idx..idx + 4].copy_from_slice(&color);
                                }
                            }
                        }
                    }
                }
            }
            let _ = char_w;
            x0 += advance;
            char_idx += 1;
        }
        stops.push((x0, char_idx));
        lines.push(LineStops { y: y0, stops });
        char_idx += 1;
    }

    TextLayout {
        width,
        height,
        rgba,
        line_height,
        lines,
    }
}

/// Crisp 5×7 1-bit pixel font for printable ASCII `0x20..=0x7E`.
fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        ' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        '!' => [0x04, 0x04, 0x04, 0x04, 0x00, 0x00, 0x04],
        '"' => [0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00, 0x00],
        '#' => [0x0A, 0x1F, 0x0A, 0x0A, 0x1F, 0x0A, 0x00],
        '$' => [0x04, 0x0F, 0x14, 0x0E, 0x05, 0x1E, 0x04],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '&' => [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D],
        '\'' => [0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '*' => [0x00, 0x0A, 0x04, 0x1F, 0x04, 0x0A, 0x00],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x08],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x06, 0x08, 0x10, 0x1F],
        '3' => [0x1F, 0x01, 0x02, 0x06, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        ':' => [0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00],
        ';' => [0x00, 0x04, 0x00, 0x00, 0x04, 0x04, 0x08],
        '<' => [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02],
        '=' => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
        '>' => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '@' => [0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0E],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '[' => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],
        '\\' => [0x10, 0x10, 0x08, 0x04, 0x02, 0x01, 0x01],
        ']' => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],
        '^' => [0x04, 0x0A, 0x11, 0x00, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
        '`' => [0x08, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00],
        'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        'b' => [0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x1E],
        'c' => [0x00, 0x00, 0x0E, 0x10, 0x10, 0x10, 0x0E],
        'd' => [0x01, 0x01, 0x0F, 0x11, 0x11, 0x11, 0x0F],
        'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        'f' => [0x06, 0x08, 0x1E, 0x08, 0x08, 0x08, 0x08],
        'g' => [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x0E],
        'h' => [0x10, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x11],
        'i' => [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E],
        'j' => [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C],
        'k' => [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],
        'l' => [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'm' => [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11],
        'n' => [0x00, 0x00, 0x1E, 0x11, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
        'p' => [0x00, 0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10],
        'q' => [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x01],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0F, 0x10, 0x0E, 0x01, 0x1E],
        't' => [0x08, 0x08, 0x1E, 0x08, 0x08, 0x08, 0x06],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x11, 0x0F],
        'v' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'w' => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A],
        'x' => [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11],
        'y' => [0x00, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x0E],
        'z' => [0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F],
        '{' => [0x02, 0x04, 0x04, 0x08, 0x04, 0x04, 0x02],
        '|' => [0x04, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        '}' => [0x08, 0x04, 0x04, 0x02, 0x04, 0x04, 0x08],
        '~' => [0x00, 0x08, 0x15, 0x02, 0x00, 0x00, 0x00],
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_5x7_renders_without_antialiasing() {
        let layout = render_text(
            &TextRenderParams {
                text: "AB".to_owned(),
                font_name: BUILTIN_PIXEL_FONT_NAME.to_owned(),
                font_size: 14,
                color: [255, 80, 40, 255],
                align: TextAlign::Left,
            },
            None,
        );
        assert!(layout.width >= 12);
        assert!(layout.height >= 14);
        for px in layout.rgba.chunks_exact(4) {
            assert!(
                px == [0, 0, 0, 0] || px == [255, 80, 40, 255],
                "unexpected anti-aliased pixel {px:?}"
            );
        }
    }

    #[test]
    fn ab_glyph_renders_without_antialiasing() {
        let layout = render_text(
            &TextRenderParams {
                text: "Hello\nWorld".to_owned(),
                font_name: BUNDLED_MONO_FONT_NAME.to_owned(),
                font_size: 16,
                color: [10, 220, 120, 255],
                align: TextAlign::Left,
            },
            None,
        );
        assert_eq!(layout.lines.len(), 2);
        assert!(layout.rgba.chunks_exact(4).any(|px| px[3] == 255));
        for px in layout.rgba.chunks_exact(4) {
            assert!(
                px == [0, 0, 0, 0] || px == [10, 220, 120, 255],
                "unexpected anti-aliased pixel {px:?}"
            );
        }
    }

    #[test]
    fn hit_test_and_caret_rect_round_trip() {
        let layout = render_text(
            &TextRenderParams {
                text: "ABC".to_owned(),
                font_name: BUILTIN_PIXEL_FONT_NAME.to_owned(),
                font_size: 7,
                color: [255, 255, 255, 255],
                align: TextAlign::Left,
            },
            None,
        );
        for idx in 0..=3 {
            let (x, y, h) = layout.caret_rect(idx);
            let hit = layout.hit_test_caret(x, y + h * 0.5);
            assert_eq!(hit, idx);
        }
    }

    #[test]
    fn pixel_5x7_align_centers_and_right_aligns_shorter_lines() {
        let base = |align| {
            render_text(
                &TextRenderParams {
                    text: "AAAA\nB".to_owned(),
                    font_name: BUILTIN_PIXEL_FONT_NAME.to_owned(),
                    font_size: 7,
                    color: [255, 255, 255, 255],
                    align,
                },
                None,
            )
        };
        let left = base(TextAlign::Left);
        let center = base(TextAlign::Center);
        let right = base(TextAlign::Right);
        let x_left = left.lines[1].stops[0].0;
        let x_center = center.lines[1].stops[0].0;
        let x_right = right.lines[1].stops[0].0;
        assert!(x_left < x_center && x_center < x_right);
    }
}
