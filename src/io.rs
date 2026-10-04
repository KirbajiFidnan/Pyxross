//! .pyxross format, PNG import/export, GIF export, sprite-sheet metadata.
//! R6/R7 milestones.
//!
//! [`save_document`] / [`load_document`] persist a [`Document`] as a project
//! directory: `manifest.json` (metadata) plus one full-canvas RGBA PNG per
//! layer under `layers/`. The PNG codec ([`encode_png`] / [`decode_png`] /
//! [`PngImage`]) is defined in [`crate::core::png_codec`] (pure core, D51) and
//! re-exported here for the persistence and import/export paths. R7 adds
//! [`encode_gif`] (animation export), [`sprite_sheet_meta_json`] (atlas
//! metadata), and [`read_png_file`] (PNG import).

use std::fmt;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::document::{Document, FORMAT_VERSION};
use crate::core::palette::Palette;
use crate::core::tilemap::TilePalette;
use crate::input::Keymap;

pub use crate::core::png_codec::{decode_png, encode_png, PngError, PngImage};

const EMBEDDED_PALETTE_JSON: [&str; 2] = [
    include_str!("../assets/palettes/db16.json"),
    include_str!("../assets/palettes/pico8.json"),
];

pub fn load_palettes_from_dirs(dirs: &[PathBuf]) -> Vec<Palette> {
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
            .collect();
        paths.sort();
        files.extend(paths);
    }
    let mut palettes = Vec::new();
    for path in files {
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(palette) = serde_json::from_slice::<Palette>(&bytes) else {
            continue;
        };
        if palette.validate().is_err()
            || palettes
                .iter()
                .any(|item: &Palette| item.name == palette.name)
        {
            continue;
        }
        palettes.push(palette);
    }
    palettes
}

pub fn load_embedded_palettes() -> Vec<Palette> {
    EMBEDDED_PALETTE_JSON
        .iter()
        .filter_map(|json| serde_json::from_str::<Palette>(json).ok())
        .filter(|palette| palette.validate().is_ok())
        .collect()
}

pub fn user_palette_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("pyxross").join("palettes"))
}

pub fn project_palette_dir(project_dir: &Path) -> PathBuf {
    project_dir.join("palettes")
}

pub fn load_project_palettes(project_dir: &Path) -> Vec<Palette> {
    load_palettes_from_dirs(&[project_palette_dir(project_dir)])
}

pub fn load_palettes(user_dir: Option<&Path>) -> Vec<Palette> {
    let mut dirs = Vec::new();
    if let Some(dir) = user_dir {
        dirs.push(dir.to_path_buf());
    }
    let mut palettes = load_embedded_palettes();
    let user = load_palettes_from_dirs(&dirs);
    for palette in user.into_iter().rev() {
        palettes.retain(|existing| existing.name != palette.name);
        palettes.insert(0, palette);
    }
    palettes
}

pub fn save_palette(palette: &Palette, dir: &Path) -> Result<(), PersistenceError> {
    palette
        .validate()
        .map_err(|error| PersistenceError::Manifest(error.to_string()))?;
    fs::create_dir_all(dir).map_err(PersistenceError::Io)?;
    let name = palette.name.replace('/', "_").replace('\\', "_");
    let json = serde_json::to_string_pretty(palette)
        .map_err(|error| PersistenceError::Json(error.to_string()))?;
    fs::write(dir.join(format!("{name}.json")), json).map_err(PersistenceError::Io)
}

pub fn default_keymap() -> Keymap {
    Keymap::defaults()
}

pub fn load_keymap(user_path: Option<&Path>) -> Keymap {
    let defaults = Keymap::defaults();
    let Some(path) = user_path else {
        return defaults;
    };
    let Ok(bytes) = fs::read(path) else {
        return defaults;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return defaults;
    };
    let Some(bindings) = value.get("bindings") else {
        return defaults;
    };
    let Ok(override_bindings) = serde_json::from_value(bindings.clone()) else {
        return defaults;
    };
    let override_map = Keymap {
        bindings: override_bindings,
    };
    match defaults.merge(&override_map) {
        Ok(keymap) => keymap,
        Err(_) => defaults,
    }
}

pub fn user_keymap_path() -> Option<PathBuf> {
    dirs::config_dir().map(|base| base.join("pyxross").join("keybindings.json"))
}

pub fn save_keymap(keymap: &Keymap, path: &Path) -> Result<(), PersistenceError> {
    let parent = path.parent().map_or_else(|| Path::new("."), |value| value);
    fs::create_dir_all(parent).map_err(PersistenceError::Io)?;
    let json = serde_json::to_string_pretty(keymap)
        .map_err(|error| PersistenceError::Json(error.to_string()))?;
    fs::write(path, json).map_err(PersistenceError::Io)
}

/// Errors produced by `.pyxross` document save/load.
#[derive(Debug)]
pub enum PersistenceError {
    /// Underlying filesystem error.
    Io(std::io::Error),
    /// JSON parse or serialize error.
    Json(String),
    /// A layer's PNG file is missing.
    MissingLayer(String),
    /// A pixel buffer or PNG dimension does not match the canvas.
    SizeMismatch(String),
    /// The manifest was saved by a newer Pyxross version.
    UnsupportedVersion { found: u32, supported: u32 },
    /// The manifest is structurally invalid.
    Manifest(String),
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PersistenceError::Io(e) => write!(f, "io error: {e}"),
            PersistenceError::Json(msg) => write!(f, "json error: {msg}"),
            PersistenceError::MissingLayer(msg) => write!(f, "missing layer: {msg}"),
            PersistenceError::SizeMismatch(msg) => write!(f, "size mismatch: {msg}"),
            PersistenceError::UnsupportedVersion { found, supported } => write!(
                f,
                "unsupported format version {found}: saved by a newer version of Pyxross \
                 (this build supports up to {supported})"
            ),
            PersistenceError::Manifest(msg) => write!(f, "manifest error: {msg}"),
        }
    }
}

impl std::error::Error for PersistenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PersistenceError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<PngError> for PersistenceError {
    fn from(e: PngError) -> Self {
        match e {
            PngError::Io(e) => PersistenceError::Io(e),
            PngError::Decode(msg) => {
                PersistenceError::Manifest(format!("layer PNG decode error: {msg}"))
            }
            PngError::Encode(msg) => {
                PersistenceError::Manifest(format!("layer PNG encode error: {msg}"))
            }
        }
    }
}

/// `width * height * 4` with overflow checking.
fn canvas_byte_len(width: u32, height: u32) -> Result<usize, PersistenceError> {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| {
            PersistenceError::SizeMismatch(format!(
                "canvas {width}x{height} overflows the pixel byte count"
            ))
        })
}

/// Saves `doc` as a `.pyxross` project directory.
///
/// Layout:
/// ```text
/// <dir>/
/// ├── manifest.json   — metadata (layer pixels skipped)
/// └── layers/
///     └── <layer_id>.png
/// ```
///
/// All layer PNGs are written first and `manifest.json` last, so a reader
/// never observes a new manifest over old PNGs. On error the partially
/// written directory is left in place and reported via the returned error.
///
/// Pixel buffers are validated against the canvas size before anything is
/// written, so a wrong-length buffer fails without touching the directory.
/// Group layers (`is_group == true`) carry no pixels and no PNG.
pub fn save_document(doc: &Document, dir: &Path) -> Result<(), PersistenceError> {
    let expected = canvas_byte_len(doc.canvas_width, doc.canvas_height)?;
    for layer in &doc.layers {
        if layer.is_group {
            continue;
        }
        if layer.pixels.len() != expected {
            return Err(PersistenceError::SizeMismatch(format!(
                "layer {} pixel buffer length {} does not match canvas {}x{} ({} bytes)",
                layer.id,
                layer.pixels.len(),
                doc.canvas_width,
                doc.canvas_height,
                expected
            )));
        }
    }

    let layers_dir = dir.join("layers");
    fs::create_dir_all(&layers_dir).map_err(PersistenceError::Io)?;

    for layer in &doc.layers {
        if layer.is_group {
            continue;
        }
        let png = encode_png(
            doc.canvas_width as usize,
            doc.canvas_height as usize,
            &layer.pixels,
        )?;
        fs::write(layers_dir.join(format!("{}.png", layer.id)), png)
            .map_err(PersistenceError::Io)?;
    }

    let mut value = serde_json::to_value(doc).map_err(|e| PersistenceError::Json(e.to_string()))?;
    value["format_version"] = serde_json::json!(FORMAT_VERSION);
    let json =
        serde_json::to_string_pretty(&value).map_err(|e| PersistenceError::Json(e.to_string()))?;
    fs::write(dir.join("manifest.json"), json).map_err(PersistenceError::Io)?;
    Ok(())
}

/// Loads a `.pyxross` project directory saved by [`save_document`].
///
/// Reads `manifest.json`, migrates the format version forward if needed
/// (rejecting manifests from a newer Pyxross), then fills each layer's
/// `pixels` from its `layers/<id>.png`. The PNG must be exactly
/// `canvas_width × canvas_height`; anything else is a
/// [`PersistenceError::SizeMismatch`]. Group layers carry no PNG; their
/// `pixels` stay empty. The layer parent graph is validated against the
/// manifest before any PNG is read.
pub fn load_document(dir: &Path) -> Result<Document, PersistenceError> {
    let manifest_path = dir.join("manifest.json");
    let bytes = fs::read(&manifest_path).map_err(PersistenceError::Io)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| PersistenceError::Json(format!("manifest.json is not valid JSON: {e}")))?;

    let from = match value.get("format_version") {
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| {
                PersistenceError::Manifest(
                    "format_version must be a non-negative integer".to_string(),
                )
            })?,
        // Pre-versioned manifests are treated as format v0 and migrated.
        None => 0,
    };
    let value = migrate_document(value, from)?;

    let mut doc: Document = serde_json::from_value(value).map_err(|e| {
        PersistenceError::Manifest(format!(
            "manifest.json does not match format version {FORMAT_VERSION}: {e}"
        ))
    })?;
    validate_layer_tree(&doc)?;

    for layer in &mut doc.layers {
        if layer.is_group {
            continue;
        }
        let path = dir.join("layers").join(format!("{}.png", layer.id));
        let png_bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(PersistenceError::MissingLayer(format!(
                    "layer {}: PNG file not found at {}",
                    layer.id,
                    path.display()
                )));
            }
            Err(e) => return Err(PersistenceError::Io(e)),
        };
        let img = decode_png(&png_bytes)?;
        if img.width != doc.canvas_width as usize || img.height != doc.canvas_height as usize {
            return Err(PersistenceError::SizeMismatch(format!(
                "layer {} PNG is {}x{} but the canvas is {}x{}",
                layer.id, img.width, img.height, doc.canvas_width, doc.canvas_height
            )));
        }
        layer.pixels = img.rgba;
    }
    Ok(doc)
}

/// Validates the layer parent graph of a deserialized manifest.
///
/// The manifest stores layers in DFS pre-order: every `Some(parent)` must
/// resolve to a layer id present in the manifest, and each parent must appear
/// BEFORE its children in the stored order. The before-child rule also rules
/// out cycles (a cycle would force some node to precede its own ancestor).
fn validate_layer_tree(doc: &Document) -> Result<(), PersistenceError> {
    let ids: std::collections::HashSet<u64> = doc.layers.iter().map(|layer| layer.id).collect();
    let mut appeared = std::collections::HashSet::with_capacity(doc.layers.len());
    for layer in &doc.layers {
        if let Some(parent) = layer.parent {
            if !ids.contains(&parent) {
                return Err(PersistenceError::Manifest(format!(
                    "layer {} references unknown parent {}",
                    layer.id, parent
                )));
            }
            if parent == layer.id {
                return Err(PersistenceError::Manifest(format!(
                    "layer {} cannot be its own parent",
                    layer.id
                )));
            }
            if !appeared.contains(&parent) {
                return Err(PersistenceError::Manifest(format!(
                    "layer {} parent {} must appear before it in the layer order (DFS pre-order)",
                    layer.id, parent
                )));
            }
        }
        appeared.insert(layer.id);
    }
    Ok(())
}

/// Migrates a parsed manifest forward to the current format version.
///
/// `from` is the `format_version` stored in the manifest. Versions below
/// [`FORMAT_VERSION`] are stepped forward one version at a time; a version
/// above it is rejected as [`PersistenceError::UnsupportedVersion`].
///
/// v0 → v1 is an identity migration (no structural changes yet). v1 → v2
/// injects the new layer-tree fields: every v1 layer was a flat root-level
/// layer, so each entry gains `is_group: false` and `parent: null`. v2 → v5
/// are identity steps (the intermediate tile systems were removed; their
/// fields are stripped at v5 → v6). v5 → v6 replaces the OLD tile system
/// (`tiles` / `tile_references` keys) with the new [`TilePalette`] under
/// `tile_palette`, and gives every layer doc a `tilemap: null` / `locked:
/// false`. **Documented data loss:** old tile data is dropped (the old model
/// is incompatible with the tilemap model).
pub fn migrate_document(
    mut value: serde_json::Value,
    from: u32,
) -> Result<serde_json::Value, PersistenceError> {
    if from > FORMAT_VERSION {
        return Err(PersistenceError::UnsupportedVersion {
            found: from,
            supported: FORMAT_VERSION,
        });
    }
    let mut current = from;
    while current < FORMAT_VERSION {
        match current {
            // v0 -> v1: no structural changes yet (identity stub).
            0 => {}
            // v1 -> v2: layers gain `is_group` and `parent`; v1 files are flat.
            1 => {
                if let Some(layers) = value.get_mut("layers").and_then(|v| v.as_array_mut()) {
                    for layer in layers {
                        if let Some(obj) = layer.as_object_mut() {
                            obj.insert("is_group".to_string(), serde_json::json!(false));
                            obj.insert("parent".to_string(), serde_json::Value::Null);
                        }
                    }
                }
            }
            // v2 -> v3, v3 -> v4, v4 -> v5: identity steps. The intermediate
            // tile systems they once injected have been removed; the old keys
            // (if any) are dropped by the v5 -> v6 step below.
            2 | 3 | 4 => {}
            // v5 -> v6: the OLD tile system is replaced by the tilemap palette.
            // Strip the old `tiles`/`tile_references` keys (data loss), inject
            // an empty `tile_palette`, and give every layer doc a `tilemap`
            // and `locked`.
            5 => {
                if let Some(obj) = value.as_object_mut() {
                    obj.remove("tiles");
                    obj.remove("tile_references");
                    if !obj.contains_key("tile_palette") {
                        obj.insert(
                            "tile_palette".to_string(),
                            serde_json::to_value(TilePalette::default())
                                .map_err(|e| PersistenceError::Json(e.to_string()))?,
                        );
                    }
                    if let Some(layers) = obj.get_mut("layers").and_then(|v| v.as_array_mut()) {
                        for layer in layers {
                            if let Some(layer_obj) = layer.as_object_mut() {
                                if !layer_obj.contains_key("tilemap") {
                                    layer_obj
                                        .insert("tilemap".to_string(), serde_json::Value::Null);
                                }
                                if !layer_obj.contains_key("locked") {
                                    layer_obj
                                        .insert("locked".to_string(), serde_json::json!(false));
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                return Err(PersistenceError::Manifest(format!(
                    "no migration path from format version {current}"
                )));
            }
        }
        current += 1;
    }
    Ok(value)
}

/// R6 F4: the recovery journal tracks the latest autosave so a crash can be
/// offered for restore on the next launch.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct RecoveryJournal {
    pub format_version: u32,
    pub project_dir: Option<PathBuf>,
    pub autosave_dir: PathBuf,
    pub project_name: String,
    pub saved_at: u64,
}

/// The autosave directory for `project_name` under `base`: `base/autosave/<name>`.
pub fn autosave_dir_for(base: &Path, project_name: &str) -> PathBuf {
    base.join("autosave")
        .join(sanitize_project_name(project_name))
}

/// Maps a project name to a single safe path segment (no separators, no `..`).
fn sanitize_project_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_whitespace() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Writes `recovery.json` under `base`, creating `base` if needed.
pub fn write_recovery_journal(
    base: &Path,
    journal: &RecoveryJournal,
) -> Result<(), PersistenceError> {
    fs::create_dir_all(base).map_err(PersistenceError::Io)?;
    let json =
        serde_json::to_string_pretty(journal).map_err(|e| PersistenceError::Json(e.to_string()))?;
    fs::write(base.join("recovery.json"), json).map_err(PersistenceError::Io)
}

/// Reads `recovery.json` under `base`; `None` when absent or unreadable.
pub fn read_recovery_journal(base: &Path) -> Option<RecoveryJournal> {
    let bytes = fs::read(base.join("recovery.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Best-effort removal of `recovery.json` under `base` (a manual save, new
/// canvas, or successful load supersedes the autosave copy).
pub fn clear_recovery_journal(base: &Path) {
    let _ = fs::remove_file(base.join("recovery.json"));
}

/// One frame of a GIF export: RGBA pixels plus a display delay.
pub struct GifFrameInput {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

/// Errors produced by the GIF encoder.
#[derive(Debug)]
pub enum GifError {
    Io(std::io::Error),
    Encode(String),
    FrameMismatch(String),
}

impl fmt::Display for GifError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GifError::Io(e) => write!(f, "gif io error: {e}"),
            GifError::Encode(msg) => write!(f, "gif encode error: {msg}"),
            GifError::FrameMismatch(msg) => write!(f, "gif frame mismatch: {msg}"),
        }
    }
}

impl std::error::Error for GifError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GifError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for GifError {
    fn from(e: std::io::Error) -> Self {
        GifError::Io(e)
    }
}

/// Encodes animation frames as an infinite-looping GIF.
///
/// Every frame must share the first frame's canvas size and carry exactly
/// `width * height * 4` RGBA bytes; the delay is rounded to the nearest
/// 10 ms (the GIF time unit) and clamped to `1..=u16::MAX`.
pub fn encode_gif(frames: &[GifFrameInput]) -> Result<Vec<u8>, GifError> {
    let Some(first) = frames.first() else {
        return Err(GifError::FrameMismatch("no frames".to_string()));
    };
    let canvas_w = first.width;
    let canvas_h = first.height;
    if canvas_w == 0 || canvas_h == 0 {
        return Err(GifError::FrameMismatch(format!(
            "frame dimensions {canvas_w}x{canvas_h} must be non-zero"
        )));
    }
    let w = u16::try_from(canvas_w).map_err(|_| {
        GifError::FrameMismatch(format!("width {canvas_w} does not fit the GIF format"))
    })?;
    let h = u16::try_from(canvas_h).map_err(|_| {
        GifError::FrameMismatch(format!("height {canvas_h} does not fit the GIF format"))
    })?;

    let mut out = Vec::new();
    {
        let mut encoder = gif::Encoder::new(Cursor::new(&mut out), w, h, &[])
            .map_err(|e| GifError::Encode(e.to_string()))?;
        encoder
            .set_repeat(gif::Repeat::Infinite)
            .map_err(|e| GifError::Encode(e.to_string()))?;
        for frame in frames {
            if frame.width != canvas_w || frame.height != canvas_h {
                return Err(GifError::FrameMismatch(format!(
                    "frame {}x{} does not match canvas {canvas_w}x{canvas_h}",
                    frame.width, frame.height
                )));
            }
            let expected = match canvas_w
                .checked_mul(canvas_h)
                .and_then(|n| n.checked_mul(4))
            {
                Some(n) => n,
                None => {
                    return Err(GifError::FrameMismatch(format!(
                        "dimensions {canvas_w}x{canvas_h} overflow the byte count"
                    )));
                }
            };
            if frame.rgba.len() != expected {
                return Err(GifError::FrameMismatch(format!(
                    "rgba buffer length {} does not match width*height*4 = {expected}",
                    frame.rgba.len()
                )));
            }
            let mut rgba = frame.rgba.clone();
            let mut gif_frame = gif::Frame::from_rgba_speed(w, h, &mut rgba, 10);
            gif_frame.delay =
                (frame.delay_ms.saturating_add(5) / 10).clamp(1, u16::MAX as u32) as u16;
            encoder
                .write_frame(&gif_frame)
                .map_err(|e| GifError::Encode(e.to_string()))?;
        }
    }
    Ok(out)
}

/// JSON metadata for a sprite-sheet export, mirroring the document's
/// regions/frames/sequences so a game engine can slice the atlas.
#[derive(Serialize)]
struct SpriteSheetMeta {
    canvas_width: u32,
    canvas_height: u32,
    tile_size: u32,
    regions: Vec<RegionMeta>,
    frames: Vec<FrameMeta>,
    sequences: Vec<SequenceMeta>,
}

#[derive(Serialize)]
struct RegionMeta {
    id: u64,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
struct FrameMeta {
    id: u64,
    region_id: u64,
    delay_ms: u32,
}

#[derive(Serialize)]
struct SequenceMeta {
    id: u64,
    name: String,
    frame_ids: Vec<u64>,
    loop_flag: bool,
    tags: Vec<String>,
}

/// Serializes the sprite-sheet metadata for `doc` as pretty-printed JSON.
pub fn sprite_sheet_meta_json(doc: &Document) -> Result<Vec<u8>, serde_json::Error> {
    let meta = SpriteSheetMeta {
        canvas_width: doc.canvas_width,
        canvas_height: doc.canvas_height,
        tile_size: doc.tile_size,
        regions: doc
            .regions
            .iter()
            .map(|r| RegionMeta {
                id: r.id,
                x: r.x,
                y: r.y,
                width: r.width,
                height: r.height,
            })
            .collect(),
        frames: doc
            .frames
            .iter()
            .enumerate()
            .map(|(i, f)| FrameMeta {
                id: i as u64,
                region_id: f.region_id,
                delay_ms: f.delay_ms,
            })
            .collect(),
        sequences: doc
            .sequences
            .iter()
            .enumerate()
            .map(|(i, s)| SequenceMeta {
                id: i as u64,
                name: s.name.clone(),
                frame_ids: s.frame_ids.clone(),
                loop_flag: s.loop_flag,
                tags: s.tags.clone(),
            })
            .collect(),
    };
    serde_json::to_vec_pretty(&meta)
}

/// Reads a PNG file from disk and decodes it to RGBA.
pub fn read_png_file(path: &Path) -> Result<PngImage, PngError> {
    let bytes = fs::read(path).map_err(PngError::Io)?;
    decode_png(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_fixture(width: usize, height: usize, seed: u8) -> Vec<u8> {
        let mut px = Vec::with_capacity(width * height * 4);
        for i in 0..width * height {
            px.push((i as u8).wrapping_mul(seed).wrapping_add(1));
            px.push((i as u8).wrapping_mul(seed).wrapping_add(2));
            px.push((i as u8).wrapping_mul(seed).wrapping_add(3));
            px.push((i as u8).wrapping_mul(seed).wrapping_add(4));
        }
        px
    }

    use crate::core::document::{EditorSettings, FrameDoc, LayerDoc, RegionDoc, SequenceDoc};
    use crate::core::math::Rect2i;
    use crate::core::model::BlendMode;
    use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "pyxross-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_document() -> Document {
        let mut opaque = vec![0u8; 4 * 4 * 4];
        for chunk in opaque.chunks_exact_mut(4) {
            chunk.copy_from_slice(&[255, 0, 0, 255]);
        }
        let mut gradient = Vec::with_capacity(4 * 4 * 4);
        for i in 0..16u8 {
            gradient.extend_from_slice(&[i, 255 - i, i.wrapping_mul(2), i.wrapping_mul(3)]);
        }
        Document {
            canvas_width: 4,
            canvas_height: 4,
            tile_size: 16,
            layers: vec![
                LayerDoc {
                    id: 1,
                    name: "base".to_string(),
                    visible: true,
                    opacity: 1.0,
                    blend: BlendMode::Normal,
                    parent: None,
                    is_group: false,
                    tilemap: None,
                    locked: false,
                    pixels: opaque,
                },
                LayerDoc {
                    id: 2,
                    name: "empty".to_string(),
                    visible: false,
                    opacity: 0.5,
                    blend: BlendMode::Multiply,
                    parent: None,
                    is_group: false,
                    tilemap: None,
                    locked: false,
                    pixels: vec![0u8; 4 * 4 * 4],
                },
                LayerDoc {
                    id: 3,
                    name: "gradient".to_string(),
                    visible: true,
                    opacity: 1.0,
                    blend: BlendMode::Screen,
                    parent: None,
                    is_group: false,
                    tilemap: None,
                    locked: false,
                    pixels: gradient,
                },
            ],
            regions: vec![
                RegionDoc {
                    id: 10,
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 2,
                },
                RegionDoc {
                    id: 11,
                    x: 2,
                    y: 2,
                    width: 2,
                    height: 2,
                },
            ],
            frames: vec![
                FrameDoc {
                    region_id: 10,
                    delay_ms: 100,
                },
                FrameDoc {
                    region_id: 11,
                    delay_ms: 200,
                },
            ],
            sequences: vec![SequenceDoc {
                name: "walk".to_string(),
                frame_ids: vec![0, 1],
                loop_flag: true,
                tags: vec!["loop".to_string()],
            }],
            palette: vec![[255, 0, 0, 255], [0, 255, 0, 255]],
            tile_palette: TilePalette::default(),
            editor: EditorSettings {
                autosave_interval_min: 7,
            },
        }
    }

    #[test]
    fn save_load_roundtrip_full_document() {
        let dir = TempDir::new();
        let doc = sample_document();
        save_document(&doc, dir.path()).unwrap();
        let loaded = load_document(dir.path()).unwrap();
        assert_eq!(loaded, doc);
    }

    #[test]
    fn save_load_roundtrip_single_layer() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers.truncate(1);
        doc.regions.clear();
        doc.frames.clear();
        doc.sequences.clear();
        save_document(&doc, dir.path()).unwrap();
        assert_eq!(load_document(dir.path()).unwrap(), doc);
    }

    #[test]
    fn save_load_roundtrip_empty_canvas() {
        let dir = TempDir::new();
        let doc = Document {
            canvas_width: 0,
            canvas_height: 0,
            ..Document::default()
        };
        save_document(&doc, dir.path()).unwrap();
        assert_eq!(load_document(dir.path()).unwrap(), doc);
    }

    #[test]
    fn save_wrong_pixel_length_fails_without_touching_dir() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers[0].pixels.truncate(4 * 4 * 4 - 1);
        let err = save_document(&doc, dir.path()).unwrap_err();
        assert!(matches!(err, PersistenceError::SizeMismatch(_)));
        assert!(!dir.path().join("manifest.json").exists());
        assert!(!dir.path().join("layers").exists());
    }

    #[test]
    fn load_missing_layer_png_reports_missing_layer() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        fs::remove_file(dir.path().join("layers").join("1.png")).unwrap();
        let err = load_document(dir.path()).unwrap_err();
        match err {
            PersistenceError::MissingLayer(msg) => {
                assert!(msg.contains("1"), "message should name layer id: {msg}");
                assert!(msg.contains("1.png"), "message should name the file: {msg}");
            }
            other => panic!("expected MissingLayer, got {other:?}"),
        }
    }

    #[test]
    fn load_png_size_mismatch_reports_size_mismatch() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        let small = encode_png(2, 2, &[0u8; 2 * 2 * 4]).unwrap();
        fs::write(dir.path().join("layers").join("1.png"), small).unwrap();
        let err = load_document(dir.path()).unwrap_err();
        assert!(matches!(err, PersistenceError::SizeMismatch(_)));
    }

    #[test]
    fn load_corrupt_layer_png_returns_err() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        let path = dir.path().join("layers").join("1.png");
        let mut bytes = fs::read(&path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        fs::write(path, bytes).unwrap();
        assert!(load_document(dir.path()).is_err());
    }

    #[test]
    fn load_truncated_layer_png_returns_err() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        let path = dir.path().join("layers").join("1.png");
        let bytes = fs::read(&path).unwrap();
        fs::write(path, &bytes[..bytes.len() / 2]).unwrap();
        assert!(load_document(dir.path()).is_err());
    }

    #[test]
    fn load_malformed_manifest_returns_json_error() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        fs::write(dir.path().join("manifest.json"), b"{ not json").unwrap();
        let err = load_document(dir.path()).unwrap_err();
        assert!(matches!(err, PersistenceError::Json(_)));
    }

    #[test]
    fn load_newer_format_version_is_rejected() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        let path = dir.path().join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["format_version"] = serde_json::json!(FORMAT_VERSION + 1);
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let err = load_document(dir.path()).unwrap_err();
        match err {
            PersistenceError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, FORMAT_VERSION + 1);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn load_missing_format_version_migrates_from_zero() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        let path = dir.path().join("manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value.as_object_mut().unwrap().remove("format_version");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(load_document(dir.path()).unwrap(), sample_document());
    }

    #[test]
    fn manifest_contains_no_pixel_data() {
        let dir = TempDir::new();
        save_document(&sample_document(), dir.path()).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(value["format_version"], serde_json::json!(FORMAT_VERSION));
        let layers = value["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 3);
        for layer in layers {
            assert!(
                layer.get("pixels").is_none(),
                "manifest must not embed pixel data"
            );
        }
    }

    #[test]
    fn migrate_document_v0_migrates_forward_and_injects_tilemap_fields() {
        // v0 -> v1 is an identity step, but the full chain to the current
        // version injects the new `tile_palette` and strips the old tile keys.
        // Existing fields must be preserved untouched.
        let value = serde_json::json!({"canvas_width": 4});
        let migrated = migrate_document(value.clone(), 0).unwrap();
        assert_eq!(migrated["canvas_width"], value["canvas_width"]);
        let palette: TilePalette =
            serde_json::from_value(migrated["tile_palette"].clone()).unwrap();
        assert_eq!(palette, TilePalette::default());
        assert!(palette.is_empty());
        assert!(
            migrated.get("tiles").is_none(),
            "the old `tiles` key must be stripped"
        );
        assert!(
            migrated.get("tile_references").is_none(),
            "the old `tile_references` key must be stripped"
        );
    }

    #[test]
    fn migrate_document_current_version_passes_through() {
        let value = serde_json::json!({"canvas_width": 4});
        assert_eq!(
            migrate_document(value.clone(), FORMAT_VERSION).unwrap(),
            value
        );
    }

    #[test]
    fn migrate_document_newer_version_is_rejected() {
        let err = migrate_document(serde_json::json!({}), FORMAT_VERSION + 1).unwrap_err();
        match err {
            PersistenceError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, FORMAT_VERSION + 1);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn migrate_document_v1_injects_flat_group_fields() {
        let value = serde_json::json!({
            "layers": [
                {"id": 1, "name": "a"},
                {"id": 2, "name": "b"}
            ]
        });
        let migrated = migrate_document(value, 1).unwrap();
        let layers = migrated["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 2);
        for layer in layers {
            assert_eq!(layer["is_group"], serde_json::json!(false));
            assert_eq!(layer["parent"], serde_json::Value::Null);
        }
    }

    #[test]
    fn migrate_document_v5_strips_old_tile_keys_and_injects_tile_palette() {
        let value = serde_json::json!({
            "tiles": {"tiles": [], "selected": null},
            "tile_references": {"cells": []},
            "layers": [
                {"id": 1, "name": "base", "visible": true, "opacity": 1.0,
                 "blend": "Normal", "is_group": false, "parent": null}
            ]
        });
        let migrated = migrate_document(value, 5).unwrap();
        assert!(
            migrated.get("tiles").is_none(),
            "the old `tiles` key is dropped"
        );
        assert!(
            migrated.get("tile_references").is_none(),
            "the old `tile_references` key is dropped"
        );
        let palette: TilePalette =
            serde_json::from_value(migrated["tile_palette"].clone()).unwrap();
        assert_eq!(palette, TilePalette::default());
        let layers = migrated["layers"].as_array().unwrap();
        assert_eq!(layers[0]["tilemap"], serde_json::Value::Null);
        assert_eq!(layers[0]["locked"], serde_json::json!(false));
    }

    #[test]
    fn migrate_document_v5_preserves_tile_palette_and_layer_fields() {
        let value = serde_json::json!({
            "tile_palette": {
                "tiles": [{"id": 1, "w": 2, "h": 2, "pixels": [1, 2, 3, 4]}],
                "selected": 1
            },
            "layers": [
                {"id": 1, "name": "base", "visible": true, "opacity": 1.0,
                 "blend": "Normal", "is_group": false, "parent": null,
                 "tilemap": {"tile_size": 4, "cols": 2, "rows": 2,
                             "cells": [null, null, null, null]},
                 "locked": true}
            ]
        });
        let migrated = migrate_document(value, 5).unwrap();
        let palette: TilePalette =
            serde_json::from_value(migrated["tile_palette"].clone()).unwrap();
        assert_eq!(palette.tiles.len(), 1);
        assert_eq!(palette.selected, Some(crate::core::tilemap::TileId(1)));
        let layers = migrated["layers"].as_array().unwrap();
        assert_eq!(layers[0]["locked"], serde_json::json!(true));
        assert!(layers[0].get("tilemap").is_some());
    }

    #[test]
    fn v5_manifest_loads_end_to_end() {
        let dir = TempDir::new();
        let manifest = serde_json::json!({
            "format_version": 5,
            "canvas_width": 8,
            "canvas_height": 8,
            "tile_size": 4,
            "layers": [
                {"id": 1, "name": "base", "visible": true, "opacity": 1.0,
                 "blend": "Normal", "is_group": false, "parent": null}
            ],
            "regions": [],
            "frames": [],
            "sequences": [],
            "palette": [],
            "tiles": {"tiles": [], "selected": null},
            "tile_references": {"cells": []},
            "editor": {"autosave_interval_min": 5}
        });
        fs::create_dir_all(dir.path().join("layers")).unwrap();
        fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.path().join("layers").join("1.png"),
            encode_png(8, 8, &[0u8; 8 * 8 * 4]).unwrap(),
        )
        .unwrap();

        let doc = load_document(dir.path()).unwrap();
        assert_eq!(doc.tile_palette, TilePalette::default());
        assert!(doc.tile_palette.is_empty());
        assert_eq!(doc.layers[0].tilemap, None);
        assert!(!doc.layers[0].locked);
    }

    #[test]
    fn tile_palette_roundtrip_through_save_load() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers.truncate(1);
        doc.regions.clear();
        doc.frames.clear();
        doc.sequences.clear();
        let mut palette = TilePalette::new();
        palette.add(Tile {
            id: TileId(1),
            w: 2,
            h: 2,
            pixels: vec![9u8; 2 * 2 * 4],
        });
        palette.add(Tile {
            id: TileId(0),
            w: 3,
            h: 1,
            pixels: vec![5u8; 3 * 1 * 4],
        });
        palette.select(TileId(1));
        doc.tile_palette = palette;
        save_document(&doc, dir.path()).unwrap();
        let loaded = load_document(dir.path()).unwrap();
        assert_eq!(loaded.tile_palette, doc.tile_palette);
        assert_eq!(loaded, doc);
    }

    #[test]
    fn tilemap_and_locked_layer_roundtrip_through_save_load() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers.truncate(1);
        doc.regions.clear();
        doc.frames.clear();
        doc.sequences.clear();
        let mut map = TileMap::new(4, 2, 2);
        map.set_cell(
            (1, 0),
            Some(TileCell {
                tile_id: TileId(3),
                rotation: 2,
                flip_x: true,
                flip_y: false,
            }),
        );
        doc.layers[0].tilemap = Some(map);
        doc.layers[0].locked = true;
        save_document(&doc, dir.path()).unwrap();
        let loaded = load_document(dir.path()).unwrap();
        assert_eq!(loaded.layers[0].tilemap, doc.layers[0].tilemap);
        assert!(loaded.layers[0].locked);
        assert_eq!(loaded, doc);
    }

    #[test]
    fn project_session_document_roundtrip_preserves_tile_palette() {
        use crate::ui::project::{ProjectId, ProjectSession};

        let mut session = ProjectSession::new(ProjectId::new(1), "tiles", 4, 4);
        let first = session.tile_palette.add(Tile {
            id: TileId(0),
            w: 2,
            h: 2,
            pixels: vec![0u8; 2 * 2 * 4],
        });
        let second = session.tile_palette.add(Tile {
            id: TileId(0),
            w: 2,
            h: 2,
            pixels: vec![0u8; 2 * 2 * 4],
        });
        assert!(session.tile_palette.select(second));
        let doc = session.to_document();
        assert_eq!(doc.tile_palette, session.tile_palette);
        assert_eq!(doc.tile_palette.tiles.len(), 2);
        assert_eq!(doc.tile_palette.selected, Some(second));

        let mut restored = ProjectSession::new(ProjectId::new(2), "restored", 4, 4);
        restored.load_document(&doc);
        assert_eq!(restored.tile_palette, doc.tile_palette);
        assert_eq!(
            restored.tile_palette.selected_tile().map(|tile| tile.id),
            Some(second)
        );
        assert_eq!(first, TileId(1));
    }

    #[test]
    fn v2_round_trip_nested_groups_byte_exact() {
        use crate::core::color::Color;
        use crate::core::model::LayerStack;
        use crate::ui::project::{ProjectId, ProjectSession};

        let dir = TempDir::new();
        let mut stack = LayerStack::new(4, 4);
        let root = stack.active_layer_id();
        stack
            .layer_mut(root)
            .unwrap()
            .buffer
            .fill(Color::rgb(1, 2, 3));
        let a = stack.add_layer("A");
        stack.layer_mut(a).unwrap().buffer.fill(Color::rgb(4, 5, 6));
        let b = stack.add_layer("B");
        stack.layer_mut(b).unwrap().buffer.fill(Color::rgb(7, 8, 9));
        let g = stack.create_group_around(&[a, b], "G").unwrap();
        let c = stack.add_layer("C");
        stack
            .layer_mut(c)
            .unwrap()
            .buffer
            .fill(Color::rgb(10, 11, 12));
        let g2 = stack.create_group_around(&[c], "G2").unwrap();
        assert!(stack.set_parent_and_position(g2, Some(g), 1));
        let expected_structure = stack.structure();

        let mut session = ProjectSession::new(ProjectId::new(1), "roundtrip", 4, 4);
        session.layers = stack;
        let doc = session.to_document();
        save_document(&doc, dir.path()).unwrap();
        let loaded = load_document(dir.path()).unwrap();
        assert_eq!(loaded, doc);

        let mut restored = ProjectSession::new(ProjectId::new(2), "restored", 4, 4);
        restored.load_document(&loaded);
        assert_eq!(restored.layers.structure(), expected_structure);
    }

    #[test]
    fn v1_flat_manifest_migrates_to_v2_flat() {
        let dir = TempDir::new();
        let manifest = serde_json::json!({
            "format_version": 1,
            "canvas_width": 2,
            "canvas_height": 2,
            "tile_size": 16,
            "layers": [
                {"id": 1, "name": "base", "visible": true, "opacity": 1.0, "blend": "Normal"},
                {"id": 2, "name": "top", "visible": true, "opacity": 0.5, "blend": "Multiply"}
            ],
            "regions": [],
            "frames": [],
            "sequences": [],
            "palette": [],
            "editor": {"autosave_interval_min": 5}
        });
        fs::create_dir_all(dir.path().join("layers")).unwrap();
        fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        for id in [1, 2] {
            fs::write(
                dir.path().join("layers").join(format!("{id}.png")),
                encode_png(2, 2, &[0u8; 2 * 2 * 4]).unwrap(),
            )
            .unwrap();
        }

        let doc = load_document(dir.path()).unwrap();
        assert_eq!(doc.layers.len(), 2);
        assert!(doc
            .layers
            .iter()
            .all(|layer| !layer.is_group && layer.parent.is_none()));
        assert_eq!(doc.layers[0].id, 1);
        assert_eq!(doc.layers[1].id, 2);
        assert_eq!(doc.layers[0].pixels, vec![0u8; 2 * 2 * 4]);
    }

    #[test]
    fn group_layers_write_no_png() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers.insert(
            1,
            LayerDoc {
                id: 99,
                name: "Group".to_string(),
                visible: true,
                opacity: 1.0,
                blend: BlendMode::Normal,
                parent: None,
                is_group: true,
                tilemap: None,
                locked: false,
                pixels: Vec::new(),
            },
        );
        save_document(&doc, dir.path()).unwrap();
        assert!(!dir.path().join("layers").join("99.png").exists());
        for id in [1, 2, 3] {
            assert!(
                dir.path().join("layers").join(format!("{id}.png")).exists(),
                "leaf layer {id} must still have a PNG"
            );
        }
        assert_eq!(load_document(dir.path()).unwrap(), doc);
    }

    #[test]
    fn manifest_with_unknown_parent_is_rejected() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers[1].parent = Some(999);
        save_document(&doc, dir.path()).unwrap();
        let err = load_document(dir.path()).unwrap_err();
        match err {
            PersistenceError::Manifest(msg) => {
                assert!(
                    msg.contains("999"),
                    "message should name the parent id: {msg}"
                );
            }
            other => panic!("expected Manifest error, got {other:?}"),
        }
    }

    #[test]
    fn manifest_with_parent_cycle_is_rejected() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers[0].parent = Some(2);
        doc.layers[1].parent = Some(1);
        save_document(&doc, dir.path()).unwrap();
        let err = load_document(dir.path()).unwrap_err();
        assert!(matches!(err, PersistenceError::Manifest(_)));
    }

    #[test]
    fn recovery_journal_roundtrip() {
        let dir = TempDir::new();
        let journal = RecoveryJournal {
            format_version: FORMAT_VERSION,
            project_dir: Some(PathBuf::from("/tmp/real/project")),
            autosave_dir: PathBuf::from("/tmp/real/project/autosave"),
            project_name: "project".to_string(),
            saved_at: 1_700_000_000,
        };
        write_recovery_journal(dir.path(), &journal).unwrap();
        assert_eq!(read_recovery_journal(dir.path()).unwrap(), journal);
    }

    #[test]
    fn recovery_journal_clear_removes_file() {
        let dir = TempDir::new();
        let journal = RecoveryJournal {
            format_version: FORMAT_VERSION,
            project_dir: None,
            autosave_dir: PathBuf::from("/tmp/x"),
            project_name: "x".to_string(),
            saved_at: 1,
        };
        write_recovery_journal(dir.path(), &journal).unwrap();
        assert!(dir.path().join("recovery.json").exists());
        clear_recovery_journal(dir.path());
        assert!(!dir.path().join("recovery.json").exists());
        // Clearing a missing journal is a no-op.
        clear_recovery_journal(dir.path());
    }

    #[test]
    fn read_recovery_journal_missing_returns_none() {
        let dir = TempDir::new();
        assert!(read_recovery_journal(dir.path()).is_none());
    }

    #[test]
    fn autosave_dir_derivation_and_sanitization() {
        let base = Path::new("/tmp/pyxross");
        assert_eq!(
            autosave_dir_for(base, "my project"),
            PathBuf::from("/tmp/pyxross/autosave/my_project")
        );
        assert_eq!(
            autosave_dir_for(base, "a/b\\c"),
            PathBuf::from("/tmp/pyxross/autosave/a_b_c")
        );
        assert_eq!(
            autosave_dir_for(base, ".."),
            PathBuf::from("/tmp/pyxross/autosave/Untitled")
        );
        assert_eq!(
            autosave_dir_for(base, "  "),
            PathBuf::from("/tmp/pyxross/autosave/Untitled")
        );
        assert_eq!(
            autosave_dir_for(base, "a..b"),
            PathBuf::from("/tmp/pyxross/autosave/a..b")
        );
    }

    #[test]
    fn autosave_dir_roundtrips_via_save_load() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers.truncate(1);
        doc.regions.clear();
        doc.frames.clear();
        doc.sequences.clear();
        let autosave_dir = autosave_dir_for(dir.path(), "my project");
        save_document(&doc, &autosave_dir).unwrap();
        assert_eq!(load_document(&autosave_dir).unwrap(), doc);
    }

    #[test]
    fn encode_gif_roundtrip() {
        let frames = vec![
            GifFrameInput {
                width: 2,
                height: 2,
                rgba: rgba_fixture(2, 2, 1),
                delay_ms: 100,
            },
            GifFrameInput {
                width: 2,
                height: 2,
                rgba: rgba_fixture(2, 2, 2),
                delay_ms: 150,
            },
        ];
        let bytes = encode_gif(&frames).unwrap();
        assert!(bytes.starts_with(b"GIF89a"));
        let mut decoder = gif::Decoder::new(Cursor::new(&bytes)).unwrap();
        let mut count = 0;
        let mut delays = Vec::new();
        while let Some(frame) = decoder.read_next_frame().unwrap() {
            count += 1;
            delays.push(frame.delay);
        }
        assert_eq!(count, 2);
        assert_eq!(delays, vec![10, 15]);
    }

    #[test]
    fn encode_gif_rejects_bad_input() {
        assert!(matches!(encode_gif(&[]), Err(GifError::FrameMismatch(_))));
        assert!(matches!(
            encode_gif(&[GifFrameInput {
                width: 0,
                height: 0,
                rgba: vec![],
                delay_ms: 100,
            }]),
            Err(GifError::FrameMismatch(_))
        ));
        assert!(matches!(
            encode_gif(&[GifFrameInput {
                width: 2,
                height: 2,
                rgba: vec![0u8; 15],
                delay_ms: 100,
            }]),
            Err(GifError::FrameMismatch(_))
        ));
        assert!(matches!(
            encode_gif(&[
                GifFrameInput {
                    width: 2,
                    height: 2,
                    rgba: vec![0u8; 16],
                    delay_ms: 100,
                },
                GifFrameInput {
                    width: 3,
                    height: 2,
                    rgba: vec![0u8; 24],
                    delay_ms: 100,
                },
            ]),
            Err(GifError::FrameMismatch(_))
        ));
    }

    #[test]
    fn sprite_sheet_meta_json_roundtrip() {
        let doc = sample_document();
        let json = sprite_sheet_meta_json(&doc).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["canvas_width"], serde_json::json!(4));
        assert_eq!(value["canvas_height"], serde_json::json!(4));
        assert_eq!(value["tile_size"], serde_json::json!(16));
        let regions = value["regions"].as_array().unwrap();
        assert_eq!(regions.len(), 2);
        assert_eq!(regions[0]["id"], serde_json::json!(10));
        assert_eq!(regions[0]["x"], serde_json::json!(0));
        assert_eq!(regions[0]["y"], serde_json::json!(0));
        assert_eq!(regions[0]["width"], serde_json::json!(2));
        assert_eq!(regions[0]["height"], serde_json::json!(2));
        let frames = value["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["id"], serde_json::json!(0));
        assert_eq!(frames[0]["region_id"], serde_json::json!(10));
        assert_eq!(frames[0]["delay_ms"], serde_json::json!(100));
        assert_eq!(frames[1]["id"], serde_json::json!(1));
        assert_eq!(frames[1]["region_id"], serde_json::json!(11));
        assert_eq!(frames[1]["delay_ms"], serde_json::json!(200));
        let sequences = value["sequences"].as_array().unwrap();
        assert_eq!(sequences.len(), 1);
        assert_eq!(sequences[0]["name"], serde_json::json!("walk"));
        assert_eq!(sequences[0]["frame_ids"], serde_json::json!([0, 1]));
        assert_eq!(sequences[0]["loop_flag"], serde_json::json!(true));
        assert_eq!(sequences[0]["tags"], serde_json::json!(["loop"]));
    }

    #[test]
    fn read_png_file_roundtrip() {
        let dir = TempDir::new();
        let path = dir.path().join("test.png");
        let rgba = rgba_fixture(4, 4, 9);
        fs::write(&path, encode_png(4, 4, &rgba).unwrap()).unwrap();
        let img = read_png_file(&path).unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(img.rgba, rgba);
        assert!(matches!(
            read_png_file(&dir.path().join("missing.png")),
            Err(PngError::Io(_))
        ));
    }

    #[test]
    fn palette_loading_is_sorted_and_skips_malformed_files() {
        let dir = TempDir::new();
        let first = Palette::new("First", vec![[1, 2, 3, 255]]).unwrap();
        let second = Palette::new("Second", vec![[4, 5, 6, 255]]).unwrap();
        fs::write(
            dir.path().join("b.json"),
            serde_json::to_vec(&second).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.path().join("a.json"),
            serde_json::to_vec(&first).unwrap(),
        )
        .unwrap();
        fs::write(dir.path().join("bad.json"), b"not json").unwrap();
        let palettes = load_palettes_from_dirs(&[dir.path().to_path_buf()]);
        assert_eq!(
            palettes.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["First", "Second"]
        );
    }

    #[test]
    fn user_palette_wins_over_embedded_name() {
        let dir = TempDir::new();
        let user = Palette::new("DB16", vec![[9, 8, 7, 255]]).unwrap();
        fs::write(
            dir.path().join("override.json"),
            serde_json::to_vec(&user).unwrap(),
        )
        .unwrap();
        let palettes = load_palettes(Some(dir.path()));
        let selected = palettes
            .iter()
            .find(|palette| palette.name == "DB16")
            .unwrap();
        assert_eq!(selected.colors, user.colors);
    }

    #[test]
    fn keymap_override_roundtrips_and_malformed_input_falls_back() {
        let dir = TempDir::new();
        let path = dir.path().join("keybindings.json");
        let mut bindings = std::collections::BTreeMap::new();
        bindings.insert(
            crate::input::Action::Undo,
            crate::input::KeyBinding {
                key: crate::input::LogicalKey::G,
                modifiers: crate::input::Modifiers::default(),
            },
        );
        save_keymap(&crate::input::Keymap { bindings }, &path).unwrap();
        let loaded = load_keymap(Some(&path));
        assert_eq!(
            loaded.binding(crate::input::Action::Undo).unwrap().key,
            crate::input::LogicalKey::G
        );
        fs::write(&path, b"broken").unwrap();
        assert_eq!(load_keymap(Some(&path)), crate::input::Keymap::defaults());
    }

    #[test]
    fn project_palette_loader_reads_project_palette_directory() {
        let dir = TempDir::new();
        let palette_dir = project_palette_dir(dir.path());
        let palette = Palette::new("Project", vec![[7, 8, 9, 255]]).unwrap();
        fs::create_dir_all(&palette_dir).unwrap();
        fs::write(
            palette_dir.join("project.json"),
            serde_json::to_vec(&palette).unwrap(),
        )
        .unwrap();
        assert_eq!(load_project_palettes(dir.path()), vec![palette]);
    }

    #[test]
    fn embedded_keymap_asset_matches_typed_defaults() {
        let asset: crate::input::Keymap =
            serde_json::from_str(include_str!("../assets/keybindings.json")).unwrap();
        assert_eq!(crate::input::Keymap::defaults(), asset);
    }
}
