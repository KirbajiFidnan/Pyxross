//! .pyxross format, PNG import/export, GIF export, sprite-sheet metadata.
//! R6/R7 milestones.
//!
//! [`save_document`] / [`load_document`] persist a [`Document`] as a project
//! directory: `manifest.json` (metadata) plus one full-canvas RGBA PNG per
//! layer under `layers/`. D80 adds a single-file form: [`save_document_file`]
//! packs the very same entries (layers + tiles + manifest) into a STORED ZIP
//! via [`crate::core::archive`], and [`load_document`] accepts both forms — a
//! directory and a `.pyxross` archive file. The PNG codec ([`encode_png`] /
//! [`decode_png`] / [`PngImage`]) is defined in [`crate::core::png_codec`]
//! (pure core, D51) and re-exported here for the persistence and
//! import/export paths. R7 adds [`encode_gif`] (animation export),
//! [`sprite_sheet_meta_json`] (atlas metadata), and [`read_png_file`] (PNG
//! import).

use std::fmt;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{AnimationDecoder, ImageDecoder};
use serde::{Deserialize, Serialize};

use crate::core::document::{Document, FORMAT_VERSION};
use crate::core::palette::Palette;
use crate::core::tilemap::{TilePalette, TileSize};
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
    let mut palettes = vec![crate::core::palette::builtin_palette(
        crate::core::palette::BuiltinPaletteKind::Ross,
    )];
    for json in EMBEDDED_PALETTE_JSON {
        if let Ok(palette) = serde_json::from_str::<Palette>(json) {
            if palette.validate().is_ok()
                && !palettes.iter().any(|existing| existing.name == palette.name)
            {
                palettes.push(palette);
            }
        }
    }
    palettes
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
    /// A layer's or tile's PNG file is missing.
    MissingLayer(String),
    /// A pixel buffer or PNG dimension does not match the canvas.
    SizeMismatch(String),
    /// The manifest was saved by a newer Pyxross version.
    UnsupportedVersion { found: u32, supported: u32 },
    /// The manifest is structurally invalid.
    Manifest(String),
    /// The single-file archive (D80) layer failed: truncated, CRC-corrupt or
    /// otherwise malformed `.pyxross` archive, or a pack-side limit hit.
    Archive(String),
    /// The file is not a `.pyxross` archive at all (bad ZIP magic or a valid
    /// archive without a `manifest.json` entry).
    NotPyxrossFile(String),
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
            PersistenceError::Archive(msg) => write!(f, "archive error: {msg}"),
            PersistenceError::NotPyxrossFile(msg) => write!(f, "not a pyxross file: {msg}"),
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

impl From<crate::core::archive::ArchiveError> for PersistenceError {
    fn from(e: crate::core::archive::ArchiveError) -> Self {
        match e {
            // Not a ZIP at all — the user (or a dialog filter) picked a file
            // that is not a `.pyxross` archive.
            crate::core::archive::ArchiveError::BadMagic => {
                PersistenceError::NotPyxrossFile("not a ZIP archive (bad magic)".to_string())
            }
            // A checksum failure is the definitive "corrupt file" signal.
            crate::core::archive::ArchiveError::CrcMismatch(name) => PersistenceError::Archive(
                format!("entry {name:?} failed its CRC-32 check (corrupt file)"),
            ),
            // Everything else (truncated, too large, too many entries, unsafe
            // name, unsupported method) is an archive-level error.
            other => PersistenceError::Archive(other.to_string()),
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

/// `w * h * 4` with overflow checking, for a tile's pixel buffer.
fn tile_byte_len(w: u16, h: u16) -> Result<usize, PersistenceError> {
    (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| {
            PersistenceError::SizeMismatch(format!("tile {w}x{h} overflows the pixel byte count"))
        })
}

/// The canonical on-disk file map of a document, in write order.
///
/// Shared by the directory form ([`save_document`]) and the single-file form
/// ([`save_document_file`], D80): layer PNGs (`layers/<id>.png`, groups
/// skipped), tile PNGs (`tiles/<id>.png`), and `manifest.json` LAST — a reader
/// never observes a new manifest over old PNGs. Every pixel buffer is validated
/// against its size before anything is produced, so a wrong-length buffer fails
/// without touching disk or producing an archive.
fn document_entries(doc: &Document) -> Result<Vec<(String, Vec<u8>)>, PersistenceError> {
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
    // Validate every tile's buffer before writing anything (mirrors layers).
    for tile in &doc.tile_palette.tiles {
        let expected_tile = tile_byte_len(tile.w, tile.h)?;
        if tile.pixels.len() != expected_tile {
            return Err(PersistenceError::SizeMismatch(format!(
                "tile {} pixel buffer length {} does not match {}x{} ({} bytes)",
                tile.id.0,
                tile.pixels.len(),
                tile.w,
                tile.h,
                expected_tile
            )));
        }
    }

    let mut entries: Vec<(String, Vec<u8>)> =
        Vec::with_capacity(doc.layers.len() + doc.tile_palette.tiles.len() + 1);
    for layer in &doc.layers {
        if layer.is_group {
            continue;
        }
        let png = encode_png(
            doc.canvas_width as usize,
            doc.canvas_height as usize,
            &layer.pixels,
        )?;
        entries.push((format!("layers/{}.png", layer.id), png));
    }
    // Tile pixels go to `tiles/<tile_id>.png`; the manifest carries metadata
    // only (`Tile.pixels` is `skip_serializing`).
    for tile in &doc.tile_palette.tiles {
        let png = encode_png(tile.w as usize, tile.h as usize, &tile.pixels)?;
        entries.push((format!("tiles/{}.png", tile.id.0), png));
    }

    let mut value = serde_json::to_value(doc).map_err(|e| PersistenceError::Json(e.to_string()))?;
    value["format_version"] = serde_json::json!(FORMAT_VERSION);
    let json =
        serde_json::to_string_pretty(&value).map_err(|e| PersistenceError::Json(e.to_string()))?;
    entries.push(("manifest.json".to_string(), json.into_bytes()));
    Ok(entries)
}

/// Saves `doc` as a `.pyxross` project directory.
///
/// Layout:
/// ```text
/// <dir>/
/// ├── manifest.json   — metadata (layer and tile pixels skipped)
/// ├── layers/
/// │   └── <layer_id>.png
/// └── tiles/
///     └── <tile_id>.png
/// ```
///
/// All layer and tile PNGs are written first and `manifest.json` last, so a
/// reader never observes a new manifest over old PNGs. On error the partially
/// written directory is left in place and reported via the returned error.
///
/// Pixel buffers (layer and tile) are validated against their sizes before
/// anything is written, so a wrong-length buffer fails without touching the
/// directory. Group layers (`is_group == true`) carry no pixels and no PNG.
pub fn save_document(doc: &Document, dir: &Path) -> Result<(), PersistenceError> {
    let entries = document_entries(doc)?;
    fs::create_dir_all(dir.join("layers")).map_err(PersistenceError::Io)?;
    fs::create_dir_all(dir.join("tiles")).map_err(PersistenceError::Io)?;
    for (name, bytes) in &entries {
        fs::write(dir.join(name), bytes).map_err(PersistenceError::Io)?;
    }
    Ok(())
}

/// Saves `doc` as a single `.pyxross` file (D80) — the directory form's exact
/// entry set packed into a STORED ZIP archive via [`crate::core::archive`].
///
/// The write is atomic: the archive bytes land in a sibling `*.tmp` file first,
/// then `rename` moves it over `path`, so a crash never leaves a truncated
/// `.pyxross` file. On Windows the target is removed first (`rename` cannot
/// overwrite an existing file there). A validation failure or pack error leaves
/// `path` untouched.
pub fn save_document_file(doc: &Document, path: &Path) -> Result<(), PersistenceError> {
    let entries = document_entries(doc)?;
    let slice: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    let bytes = crate::core::archive::pack(&slice)?;

    let parent = path.parent().map_or_else(|| Path::new("."), |value| value);
    fs::create_dir_all(parent).map_err(PersistenceError::Io)?;
    let tmp = sibling_temp_path(path, "tmp");
    fs::write(&tmp, bytes).map_err(PersistenceError::Io)?;

    #[cfg(windows)]
    {
        if let Err(e) = replace_file_with_backup(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(PersistenceError::Io(e));
        }
    }

    #[cfg(not(windows))]
    {
        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(PersistenceError::Io(e));
        }
    }
    Ok(())
}

fn sibling_temp_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("project"));
    name.push(format!(".{suffix}"));
    path.with_file_name(name)
}

/// Safe two-phase file replacement for platforms where `rename` cannot
/// overwrite an existing destination (Windows): moves an existing `target`
/// aside to a sibling `.bak`, renames `tmp` into place, and restores `.bak`
/// if the final rename fails so the previous saved file is never lost.
#[cfg_attr(not(windows), allow(dead_code))]
fn replace_file_with_backup(tmp: &Path, target: &Path) -> std::io::Result<()> {
    let backup = sibling_temp_path(target, "bak");
    let had_target = target.exists();
    if had_target {
        let _ = fs::remove_file(&backup);
        fs::rename(target, &backup)?;
    }
    match fs::rename(tmp, target) {
        Ok(()) => {
            if had_target {
                let _ = fs::remove_file(&backup);
            }
            Ok(())
        }
        Err(err) => {
            if had_target {
                let _ = fs::rename(&backup, target);
            }
            Err(err)
        }
    }
}

/// Loads a `.pyxross` project saved by [`save_document`] (directory) or
/// [`save_document_file`] (single-file archive, D80).
///
/// A path that is a directory loads as the native directory form; any other
/// path is read as an archive file (bad ZIP magic or a missing
/// `manifest.json` entry yields [`PersistenceError::NotPyxrossFile`]).
///
/// Both forms share the same semantics: `manifest.json` is parsed and migrated
/// forward if needed (manifests from a newer Pyxross are rejected), the layer
/// parent graph is validated, then each layer's `pixels` is filled from
/// `layers/<id>.png` and each tile's from `tiles/<id>.png`.
pub fn load_document(path: &Path) -> Result<Document, PersistenceError> {
    if path.is_dir() {
        load_document_dir(path)
    } else {
        load_document_file(path)
    }
}

/// Loads the native directory form. Reads `manifest.json` and fills pixels
/// from the directory's `layers/` and `tiles/` PNG files; a missing PNG is a
/// [`PersistenceError::MissingLayer`] naming the expected file path. Group
/// layers carry no PNG; their `pixels` stay empty. Tile pixels that arrived
/// already populated (a pre-v7 manifest embedding them as JSON) are left
/// untouched, so old files load unchanged.
fn load_document_dir(dir: &Path) -> Result<Document, PersistenceError> {
    let manifest_path = dir.join("manifest.json");
    let bytes = fs::read(&manifest_path).map_err(PersistenceError::Io)?;
    let value = parse_manifest(&bytes)?;
    let doc = document_from_manifest(value)?;
    fill_pixels(doc, |name| {
        let path = dir.join(name);
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(PersistenceError::MissingLayer(format!(
                    "{}: PNG file not found at {}",
                    entry_label(name),
                    path.display()
                )))
            }
            Err(e) => Err(PersistenceError::Io(e)),
        }
    })
}

/// Loads the single-file form (D80): unpacks the STORED ZIP archive, requires
/// a `manifest.json` entry, and fills pixels from the in-archive entries.
/// Entries other than the expected `layers/*.png` / `tiles/*.png` /
/// `manifest.json` set are ignored.
fn load_document_file(path: &Path) -> Result<Document, PersistenceError> {
    let bytes = fs::read(path).map_err(PersistenceError::Io)?;
    let entries = crate::core::archive::unpack(&bytes)?;
    let manifest = find_entry(&entries, "manifest.json").ok_or_else(|| {
        PersistenceError::NotPyxrossFile("missing manifest.json entry".to_string())
    })?;
    let value = parse_manifest(manifest)?;
    let doc = document_from_manifest(value)?;
    fill_pixels(doc, |name| match find_entry(&entries, name) {
        Some(bytes) => Ok(bytes.to_vec()),
        None => Err(PersistenceError::MissingLayer(format!(
            "{}: PNG entry {name:?} not found in the archive",
            entry_label(name)
        ))),
    })
}

/// Parses the manifest bytes with the historical `manifest.json` error text
/// (shared by the directory and archive forms).
fn parse_manifest(bytes: &[u8]) -> Result<serde_json::Value, PersistenceError> {
    serde_json::from_slice(bytes)
        .map_err(|e| PersistenceError::Json(format!("manifest.json is not valid JSON: {e}")))
}

/// Version-gates, migrates and deserializes a parsed manifest, then validates
/// the layer parent graph (shared by both load forms).
fn document_from_manifest(value: serde_json::Value) -> Result<Document, PersistenceError> {
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

    let doc: Document = serde_json::from_value(value).map_err(|e| {
        PersistenceError::Manifest(format!(
            "manifest.json does not match format version {FORMAT_VERSION}: {e}"
        ))
    })?;
    validate_layer_tree(&doc)?;
    Ok(doc)
}

/// Fills layer and tile `pixels` from their PNG byte sources.
///
/// `read_entry(name)` fetches the bytes for one entry
/// (`layers/<id>.png` / `tiles/<id>.png`) and must already produce the final
/// error for a missing entry (the two load forms phrase it differently).
/// Mirrors the historical directory loader's validation and messages exactly:
/// layer PNGs must be exactly the canvas size; tile PNGs exactly `tile.w ×
/// tile.h`, with a pre-populated (inline, pre-v7) tile skipped.
fn fill_pixels(
    mut doc: Document,
    mut read_entry: impl FnMut(&str) -> Result<Vec<u8>, PersistenceError>,
) -> Result<Document, PersistenceError> {
    for layer in &mut doc.layers {
        if layer.is_group {
            continue;
        }
        let name = format!("layers/{}.png", layer.id);
        let png_bytes = read_entry(&name)?;
        let img = decode_png(&png_bytes)?;
        if img.width != doc.canvas_width as usize || img.height != doc.canvas_height as usize {
            return Err(PersistenceError::SizeMismatch(format!(
                "layer {} PNG is {}x{} but the canvas is {}x{}",
                layer.id, img.width, img.height, doc.canvas_width, doc.canvas_height
            )));
        }
        layer.pixels = img.rgba;
    }

    // Fill each tile's pixels from `tiles/<id>.png`, unless the manifest
    // already carried them (a pre-v7 manifest embeds `pixels` as JSON; those
    // arrive non-empty and are used as-is, preserving old files losslessly).
    for tile in &mut doc.tile_palette.tiles {
        if !tile.pixels.is_empty() {
            continue;
        }
        let expected = tile_byte_len(tile.w, tile.h)?;
        // A zero-area tile legitimately has no pixels and no PNG.
        if expected == 0 {
            continue;
        }
        let name = format!("tiles/{}.png", tile.id.0);
        let png_bytes = read_entry(&name)?;
        let img = decode_png(&png_bytes).map_err(|e| match e {
            PngError::Decode(msg) => PersistenceError::Manifest(format!(
                "tile {} PNG decode error: {msg}",
                tile.id.0
            )),
            other => PersistenceError::from(other),
        })?;
        if img.width != tile.w as usize || img.height != tile.h as usize {
            return Err(PersistenceError::SizeMismatch(format!(
                "tile {} PNG is {}x{} but the tile is {}x{}",
                tile.id.0, img.width, img.height, tile.w, tile.h
            )));
        }
        if img.rgba.len() != expected {
            return Err(PersistenceError::SizeMismatch(format!(
                "tile {} PNG has {} bytes but {}x{} needs {}",
                tile.id.0,
                img.rgba.len(),
                tile.w,
                tile.h,
                expected
            )));
        }
        tile.pixels = img.rgba;
    }
    Ok(doc)
}

/// Looks up one unpacked archive entry by name.
fn find_entry<'a>(entries: &'a [(String, Vec<u8>)], name: &str) -> Option<&'a [u8]> {
    entries
        .iter()
        .find(|(entry_name, _)| entry_name == name)
        .map(|(_, data)| data.as_slice())
}

/// `layers/3.png` -> `layer 3`, `tiles/7.png` -> `tile 7` (for error
/// messages shared with the directory loader).
fn entry_label(name: &str) -> String {
    if let Some(rest) = name.strip_prefix("layers/") {
        format!("layer {}", rest.trim_end_matches(".png"))
    } else if let Some(rest) = name.strip_prefix("tiles/") {
        format!("tile {}", rest.trim_end_matches(".png"))
    } else {
        name.to_string()
    }
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
/// is incompatible with the tilemap model). v6 → v7 is an identity step: tile
/// pixels moved from inline JSON into `tiles/<id>.png` (a storage change with
/// no data transformation), and `SequenceDoc.delay_multiplier` became a
/// serde-defaulted field. v7 → v8 fans the numeric `tile_size` (document and
/// each layer's tilemap) out to the per-axis `{ "w": n, "h": n }` object.
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
            // v6 -> v7: tile pixels moved out of `manifest.json` into
            // `tiles/<id>.png`. This is a STORAGE change only — no data
            // transformation is needed: a v6 manifest still carries its tile
            // `pixels` inline (read back by `Tile.pixels`' `serde(default)`),
            // and a v7 manifest omits them and loads them from the PNGs.
            // (`SequenceDoc.delay_multiplier` also became a serde-defaulted
            // field, so older manifests without it still deserialize.)
            6 => {}
            // v7 -> v8: `tile_size` became PER-AXIS (`TileSize { w, h }`). A v7
            // manifest stores it as a single NUMBER on the document and on each
            // layer's tilemap; fan the number out to `{ "w": n, "h": n }` so old
            // square-tile files load losslessly.
            7 => {
                fn number_to_tile_size(v: &mut serde_json::Value) {
                    if let Some(n) = v.as_u64() {
                        let n = n.max(1);
                        *v = serde_json::json!({ "w": n, "h": n });
                    }
                }
                if let Some(obj) = value.as_object_mut() {
                    if let Some(ts) = obj.get_mut("tile_size") {
                        number_to_tile_size(ts);
                    }
                    if let Some(layers) = obj.get_mut("layers").and_then(|v| v.as_array_mut()) {
                        for layer in layers {
                            if let Some(ts) = layer.get_mut("tilemap").and_then(|tm| tm.get_mut("tile_size"))
                            {
                                number_to_tile_size(ts);
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
#[derive(Clone, Debug)]
pub struct GifFrameInput {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

/// Errors produced by the GIF encoder ([`encode_gif`]) and decoder
/// ([`decode_gif`]).
#[derive(Debug)]
pub enum GifError {
    Io(std::io::Error),
    Encode(String),
    /// GIF decoding failed (malformed data, empty animation, zero frame size).
    Decode(String),
    FrameMismatch(String),
}

impl fmt::Display for GifError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GifError::Io(e) => write!(f, "gif io error: {e}"),
            GifError::Encode(msg) => write!(f, "gif encode error: {msg}"),
            GifError::Decode(msg) => write!(f, "gif decode error: {msg}"),
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

/// One decoded GIF animation frame (Import GIF, P1): full-canvas RGBA8 pixels
/// plus the frame's display delay.
pub struct GifFrameData {
    pub rgba: Vec<u8>,
    pub delay_ms: u32,
}

/// A decoded GIF animation (Import GIF, P1): the shared canvas size and every
/// frame's composited pixels.
pub struct GifDecode {
    pub frame_w: u32,
    pub frame_h: u32,
    pub frames: Vec<GifFrameData>,
}

/// Decodes a GIF animation into full-canvas RGBA8 frames (Import GIF, P1).
///
/// Uses `image`'s GIF codec (`GifDecoder` + [`AnimationDecoder::into_frames`],
/// the same codec family as the `gif`-based [`encode_gif`]). The library
/// returns every frame fully COMPOSITED at the logical screen size — disposal
/// and transparency included — so `frame.left()/top()` are ignored: each
/// `rgba` is exactly `frame_w × frame_h × 4`. `delay` is converted from the
/// GIF centisecond time unit to milliseconds. An empty animation (and a
/// zero-sized canvas) is an error.
pub fn decode_gif(bytes: &[u8]) -> Result<GifDecode, GifError> {
    let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
        .map_err(|e| GifError::Decode(format!("gif decode error: {e}")))?;
    let (frame_w, frame_h) = decoder.dimensions();
    if frame_w == 0 || frame_h == 0 {
        return Err(GifError::Decode("gif frame size is zero".to_string()));
    }
    let frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|e| GifError::Decode(format!("gif decode error: {e}")))?;
    if frames.is_empty() {
        return Err(GifError::Decode("gif contains no frames".to_string()));
    }

    let mut out = Vec::with_capacity(frames.len());
    for frame in frames {
        let buffer = frame.buffer();
        debug_assert_eq!(buffer.width(), frame_w);
        debug_assert_eq!(buffer.height(), frame_h);
        debug_assert_eq!(buffer.len(), frame_w as usize * frame_h as usize * 4);
        let duration: std::time::Duration = frame.delay().into();
        out.push(GifFrameData {
            rgba: buffer.as_raw().clone(),
            delay_ms: duration.as_millis() as u32,
        });
    }
    Ok(GifDecode {
        frame_w,
        frame_h,
        frames: out,
    })
}

/// JSON metadata for a sprite-sheet export, mirroring the document's
/// regions/frames/sequences so a game engine can slice the atlas.
#[derive(Serialize)]
struct SpriteSheetMeta {
    canvas_width: u32,
    canvas_height: u32,
    tile_size: TileSize,
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
    /// Delay multiplier in hundredths (`100` = 1.00×). Apply it to a referenced
    /// frame's `delay_ms` to get the EFFECTIVE playback delay the editor and
    /// GIF export use, so the atlas metadata agrees with them (S6).
    delay_multiplier_centi: u32,
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
                delay_multiplier_centi: s.delay_multiplier,
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
    use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette, TileSize};

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
            tile_size: TileSize::square(16),
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
                delay_multiplier: 100,
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
        let mut map = TileMap::new(TileSize::square(4), 2, 2);
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
    fn save_writes_tile_pngs_and_keeps_manifest_small() {
        let dir = TempDir::new();
        let mut doc = sample_document();
        doc.layers.truncate(1);
        doc.regions.clear();
        doc.frames.clear();
        doc.sequences.clear();
        // 100 tiles of 16x16 = 100 * 1024 bytes of raw pixels.
        let mut palette = TilePalette::new();
        for i in 1..=100u64 {
            palette.add(Tile {
                id: TileId(i),
                w: 16,
                h: 16,
                pixels: vec![i as u8; 16 * 16 * 4],
            });
        }
        doc.tile_palette = palette;
        save_document(&doc, dir.path()).unwrap();

        // Every tile got its own PNG under tiles/.
        for i in 1..=100u64 {
            let png = dir.path().join("tiles").join(format!("{i}.png"));
            assert!(png.exists(), "tiles/{i}.png must be written");
        }
        // The manifest no longer embeds the tile pixel arrays and stays small.
        let manifest = fs::read_to_string(dir.path().join("manifest.json")).unwrap();
        assert!(
            !manifest.contains("\"pixels\""),
            "the manifest must not embed tile pixels"
        );
        let raw_tile_bytes = 100 * 16 * 16 * 4;
        assert!(
            manifest.len() < raw_tile_bytes / 10,
            "manifest ({} bytes) must be far below the raw tile bytes ({raw_tile_bytes})",
            manifest.len()
        );

        // Round-trip is byte-exact.
        let loaded = load_document(dir.path()).unwrap();
        assert_eq!(loaded.tile_palette, doc.tile_palette);
    }

    #[test]
    fn legacy_v6_manifest_with_inline_tile_pixels_still_loads() {
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
            h: 1,
            pixels: vec![7u8; 2 * 1 * 4],
        });
        doc.tile_palette = palette;

        // Simulate a PRE-v7 manifest: format_version 6 AND the inline `pixels`
        // array that `skip_serializing` now omits.
        let mut value = serde_json::to_value(&doc).unwrap();
        value["format_version"] = serde_json::json!(6);
        value["tile_palette"]["tiles"][0]["pixels"] = serde_json::json!(vec![7u8; 2 * 1 * 4]);

        fs::create_dir_all(dir.path().join("layers")).unwrap();
        fs::write(
            dir.path().join("manifest.json"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
        fs::write(
            dir.path().join("layers").join("1.png"),
            encode_png(4, 4, &doc.layers[0].pixels).unwrap(),
        )
        .unwrap();
        // NOTE: no `tiles/` directory — the pixels must come from the manifest.

        let loaded = load_document(dir.path()).unwrap();
        assert_eq!(
            loaded.tile_palette.tiles[0].pixels,
            vec![7u8; 2 * 1 * 4],
            "embedded v6 pixels must be used as-is"
        );
        assert_eq!(loaded.tile_palette, doc.tile_palette);
    }

    #[test]
    fn missing_tile_png_is_an_error() {
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
            pixels: vec![3u8; 2 * 2 * 4],
        });
        doc.tile_palette = palette;
        save_document(&doc, dir.path()).unwrap();

        fs::remove_file(dir.path().join("tiles").join("1.png")).unwrap();
        let err = load_document(dir.path()).unwrap_err();
        assert!(
            matches!(err, PersistenceError::MissingLayer(ref m) if m.contains("tile 1")),
            "a missing tile PNG must error, got {err:?}"
        );
    }

    #[test]
    fn tile_png_size_mismatch_is_an_error() {
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
            pixels: vec![3u8; 2 * 2 * 4],
        });
        doc.tile_palette = palette;
        save_document(&doc, dir.path()).unwrap();

        // Overwrite with a 3x3 PNG (wrong size for a 2x2 tile).
        fs::write(
            dir.path().join("tiles").join("1.png"),
            encode_png(3, 3, &[0u8; 3 * 3 * 4]).unwrap(),
        )
        .unwrap();
        let err = load_document(dir.path()).unwrap_err();
        assert!(
            matches!(err, PersistenceError::SizeMismatch(ref m) if m.contains("tile 1")),
            "a tile PNG size mismatch must error, got {err:?}"
        );
    }

    #[test]
    fn format_version_is_eight() {
        assert_eq!(FORMAT_VERSION, 8);
    }

    #[test]
    fn migrate_document_v7_fans_tile_size_to_per_axis() {
        // A v7 manifest stores `tile_size` as a single NUMBER, both on the
        // document and on each layer's tilemap. v7 -> v8 must fan each number
        // out to `{ "w": n, "h": n }` (lossless for the old square tiles).
        let value = serde_json::json!({
            "format_version": 7,
            "canvas_width": 32,
            "canvas_height": 32,
            "tile_size": 16,
            "layers": [
                {"id": 1, "name": "base", "visible": true, "opacity": 1.0,
                 "blend": "Normal", "is_group": false, "parent": null,
                 "tilemap": {"tile_size": 8, "cols": 4, "rows": 4,
                             "cells": [null, null, null, null, null, null, null, null,
                                       null, null, null, null, null, null, null, null]}},
                {"id": 2, "name": "no map", "visible": true, "opacity": 1.0,
                 "blend": "Normal", "is_group": false, "parent": null,
                 "tilemap": null}
            ],
            "regions": [], "frames": [], "sequences": [], "palette": [],
            "tile_palette": {"tiles": [], "selected": null},
            "editor": {"autosave_interval_min": 5}
        });
        let migrated = migrate_document(value, 7).unwrap();
        assert_eq!(
            migrated["tile_size"],
            serde_json::json!({"w": 16, "h": 16})
        );
        assert_eq!(
            migrated["layers"][0]["tilemap"]["tile_size"],
            serde_json::json!({"w": 8, "h": 8})
        );
        // A null tilemap stays null.
        assert!(migrated["layers"][1]["tilemap"].is_null());

        // The fanned-out document `tile_size` deserializes into the per-axis
        // type (the full disk-load path is covered by `v5_manifest_loads_end_to_end`,
        // which now runs the v7 -> v8 step too).
        let ts: TileSize = serde_json::from_value(migrated["tile_size"].clone()).unwrap();
        assert_eq!(ts, TileSize::square(16));
    }

    #[test]
    fn migrate_document_v6_is_identity() {
        let mut value = serde_json::to_value(sample_document()).unwrap();
        value["format_version"] = serde_json::json!(6);
        let before = value.clone();
        let migrated = migrate_document(value, 6).unwrap();
        assert_eq!(migrated, before, "v6 -> v7 must not transform the data");
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
    fn decode_gif_roundtrips_encoded_animation() {
        // Encode a 2x2, 3-frame animation with the in-house encoder, then
        // decode it back: the GIF centisecond delay unit converts to the
        // original millisecond values, and every frame is full-canvas RGBA8.
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
                delay_ms: 200,
            },
            GifFrameInput {
                width: 2,
                height: 2,
                rgba: rgba_fixture(2, 2, 3),
                delay_ms: 300,
            },
        ];
        let bytes = encode_gif(&frames).unwrap();
        let decoded = decode_gif(&bytes).unwrap();

        assert_eq!((decoded.frame_w, decoded.frame_h), (2, 2));
        assert_eq!(decoded.frames.len(), 3);
        for (index, frame) in decoded.frames.iter().enumerate() {
            assert_eq!(frame.rgba.len(), 2 * 2 * 4, "frame {index} must be full canvas");
            assert_eq!(frame.delay_ms, frames[index].delay_ms);
        }
    }

    #[test]
    fn decode_gif_rejects_garbage_and_empty_animations() {
        // Not a GIF at all.
        assert!(matches!(
            decode_gif(b"this is not a gif"),
            Err(GifError::Decode(_))
        ));
        // A single-frame GIF encodes, slices to zero frames -> empty decode
        // must still error for the empty-frames case.
        let gif = encode_gif(&[GifFrameInput {
            width: 2,
            height: 2,
            rgba: vec![0u8; 2 * 2 * 4],
            delay_ms: 100,
        }])
        .unwrap();
        assert!(decode_gif(&gif).is_ok());
        assert!(decode_gif(&[]).is_err());
    }

    #[test]
    fn sprite_sheet_meta_json_roundtrip() {
        let doc = sample_document();
        let json = sprite_sheet_meta_json(&doc).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["canvas_width"], serde_json::json!(4));
        assert_eq!(value["canvas_height"], serde_json::json!(4));
        assert_eq!(value["tile_size"], serde_json::json!({"w": 16, "h": 16}));
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
    fn sprite_sheet_meta_exports_multiplier_matching_effective_delays() {
        // S6: the atlas metadata must expose the sequence delay multiplier so a
        // consumer can reproduce the SAME effective playback delay the runtime
        // (`effective_delay_ms`) and GIF export use.
        use crate::core::model::{AnimationSequence, Frame, Region};

        let mut doc = sample_document();
        doc.sequences[0].delay_multiplier = 250; // 2.50×
        let json = sprite_sheet_meta_json(&doc).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();

        let sequences = value["sequences"].as_array().unwrap();
        let multiplier = sequences[0]["delay_multiplier_centi"].as_u64().unwrap();
        assert_eq!(multiplier, 250, "the multiplier is exported");

        // Rebuild the equivalent sequence from the exported raw delays and
        // assert the metadata-derived effective delay equals the runtime's.
        let raw: Vec<u32> = value["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|frame| frame["delay_ms"].as_u64().unwrap() as u32)
            .collect();
        let mut sequence = AnimationSequence::new("walk");
        sequence.set_delay_multiplier_centi(multiplier as u32);
        for (index, delay) in raw.iter().enumerate() {
            sequence.push(Frame::new(
                Region::new(Rect2i::new(0, 0, 1, 1), format!("f{index}")),
                *delay,
            ));
        }
        for (index, delay) in raw.iter().enumerate() {
            let from_metadata = (u64::from(*delay) * multiplier + 50) / 100;
            assert_eq!(
                from_metadata.min(60_000) as u32,
                sequence.effective_delay_ms(index),
                "exported metadata must agree with effective_delay_ms for frame {index}"
            );
        }
        assert_eq!(sequence.effective_delay_ms(0), 250); // 100 × 2.50
        assert_eq!(sequence.effective_delay_ms(1), 500); // 200 × 2.50
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

    // -----------------------------------------------------------------------
    // D80 single-file form
    // -----------------------------------------------------------------------

    #[test]
    fn single_file_roundtrip_equals_directory_roundtrip() {
        let dir = TempDir::new();
        let doc = sample_document();
        save_document(&doc, dir.path()).unwrap();
        let from_dir = load_document(dir.path()).unwrap();
        assert_eq!(from_dir, doc);

        let file = dir.path().join("project.pyxross");
        save_document_file(&doc, &file).unwrap();
        let from_file = load_document(&file).unwrap();
        assert_eq!(from_file, doc);
        assert_eq!(from_file, from_dir, "both forms must load identically");
    }

    #[test]
    fn single_file_empty_project_roundtrip() {
        let dir = TempDir::new();
        let doc = Document {
            canvas_width: 0,
            canvas_height: 0,
            ..Document::default()
        };
        let file = dir.path().join("empty.pyxross");
        save_document_file(&doc, &file).unwrap();
        assert_eq!(load_document(&file).unwrap(), doc);
    }

    #[test]
    fn single_file_group_layers_are_omitted() {
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
        let file = dir.path().join("groups.pyxross");
        save_document_file(&doc, &file).unwrap();
        let entries = crate::core::archive::unpack(&fs::read(&file).unwrap()).unwrap();
        assert!(
            entries.iter().all(|(name, _)| name != "layers/99.png"),
            "a group layer must not be packed"
        );
        assert_eq!(load_document(&file).unwrap(), doc);
    }

    #[test]
    fn single_file_overwrite_existing_and_leaves_no_tmp() {
        let dir = TempDir::new();
        let file = dir.path().join("overwrite.pyxross");
        fs::write(&file, b"old contents unrelated to pyxross").unwrap();
        save_document_file(&sample_document(), &file).unwrap();
        assert_eq!(load_document(&file).unwrap(), sample_document());
        // The temp sibling must be gone after a successful save.
        let leftovers: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no .tmp files may remain: {leftovers:?}");
    }

    #[test]
    fn single_file_garbage_is_rejected_as_not_pyxross() {
        let dir = TempDir::new();
        let garbage = dir.path().join("garbage.pyxross");
        fs::write(&garbage, b"this is not a zip file at all").unwrap();
        assert!(matches!(
            load_document(&garbage),
            Err(PersistenceError::NotPyxrossFile(_))
        ));

        let empty = dir.path().join("empty.pyxross");
        fs::write(&empty, b"").unwrap();
        assert!(matches!(
            load_document(&empty),
            Err(PersistenceError::NotPyxrossFile(_))
        ));
    }

    #[test]
    fn single_file_truncated_archive_is_an_error() {
        let dir = TempDir::new();
        let file = dir.path().join("cut.pyxross");
        save_document_file(&sample_document(), &file).unwrap();
        let bytes = fs::read(&file).unwrap();
        fs::write(&file, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(
            load_document(&file),
            Err(PersistenceError::Archive(_))
        ));
    }

    #[test]
    fn single_file_corrupt_manifest_reports_crc() {
        let dir = TempDir::new();
        let doc = sample_document();
        let file = dir.path().join("crc.pyxross");
        save_document_file(&doc, &file).unwrap();
        let mut bytes = fs::read(&file).unwrap();

        // Locate the manifest payload: entries are laid out back-to-back as
        // LFH(30) + name + data and the manifest is the LAST entry.
        let layout = document_entries(&doc).unwrap();
        let mut offset = 0usize;
        for (name, data) in &layout {
            if name == "manifest.json" {
                break; // `offset` now points at the manifest's local header
            }
            offset += 30 + name.len() + data.len();
        }
        let data_start = offset + 30 + "manifest.json".len();
        bytes[data_start] ^= 0x01;
        fs::write(&file, bytes).unwrap();

        let err = load_document(&file).unwrap_err();
        match err {
            PersistenceError::Archive(msg) => {
                assert!(msg.contains("CRC-32"), "message should identify the CRC: {msg}")
            }
            other => panic!("expected Archive error, got {other:?}"),
        }
    }

    #[test]
    fn single_file_missing_manifest_is_not_pyxross() {
        let dir = TempDir::new();
        let file = dir.path().join("nomanifest.pyxross");
        let png = encode_png(4, 4, &[0u8; 4 * 4 * 4]).unwrap();
        let zip = crate::core::archive::pack(&[("layers/1.png", png.as_slice())]).unwrap();
        fs::write(&file, zip).unwrap();
        let err = load_document(&file).unwrap_err();
        assert!(
            matches!(err, PersistenceError::NotPyxrossFile(_)),
            "a valid archive without a manifest is not a pyxross file, got {err:?}"
        );
    }

    #[test]
    fn single_file_ignores_unknown_entries() {
        let dir = TempDir::new();
        let doc = sample_document();
        let entries = document_entries(&doc).unwrap();
        let mut slice: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        slice.push(("readme.txt", b"hello extra"));
        slice.push(("thumbs/1.png", b"ignored"));
        let zip = crate::core::archive::pack(&slice).unwrap();
        let file = dir.path().join("extra.pyxross");
        fs::write(&file, zip).unwrap();
        assert_eq!(load_document(&file).unwrap(), doc);
    }

    #[test]
    fn single_file_missing_layer_png_is_an_error() {
        let dir = TempDir::new();
        let doc = sample_document();
        let mut entries = document_entries(&doc).unwrap();
        entries.retain(|(name, _)| name != "layers/1.png");
        let slice: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(name, data)| (name.as_str(), data.as_slice()))
            .collect();
        let zip = crate::core::archive::pack(&slice).unwrap();
        let file = dir.path().join("missing-layer.pyxross");
        fs::write(&file, zip).unwrap();
        let err = load_document(&file).unwrap_err();
        match err {
            PersistenceError::MissingLayer(msg) => {
                assert!(
                    msg.contains("layer 1") && msg.contains("not found"),
                    "message should name the layer and the missing entry: {msg}"
                );
            }
            other => panic!("expected MissingLayer, got {other:?}"),
        }
    }

    /// Hand-built single-entry STORED ZIP with the given raw name, bypassing
    /// `pack`'s name validation (CRC left 0 — name checks run first).
    fn raw_stored_zip(name: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        // Local file header (30 bytes).
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&0u32.to_le_bytes()); // crc
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name);
        out.extend_from_slice(data);
        let cd_offset = out.len() as u32;
        // Central directory header (46 bytes).
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // crc
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // local header offset
        out.extend_from_slice(name);
        let cd_size = out.len() as u32 - cd_offset;
        // End of central directory (22 bytes).
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn single_file_traversal_entry_names_are_rejected() {
        let dir = TempDir::new();
        let file = dir.path().join("evil.pyxross");
        fs::write(&file, raw_stored_zip(b"../evil.png", b"payload")).unwrap();
        let err = load_document(&file).unwrap_err();
        assert!(
            matches!(err, PersistenceError::Archive(_)),
            "an unsafe entry name must be rejected, got {err:?}"
        );
    }

    #[test]
    fn save_document_file_wrong_buffer_fails_without_creating_file() {
        let dir = TempDir::new();
        let file = dir.path().join("bad.pyxross");
        let mut doc = sample_document();
        doc.layers[0].pixels.truncate(4 * 4 * 4 - 1);
        let err = save_document_file(&doc, &file).unwrap_err();
        assert!(matches!(err, PersistenceError::SizeMismatch(_)));
        assert!(
            !file.exists(),
            "no file may be created when validation fails"
        );
    }

    #[test]
    fn replace_file_with_backup_creates_overwrites_and_rolls_back_on_error() {
        let dir = TempDir::new();
        let target = dir.path().join("project.pyxross");
        let tmp = sibling_temp_path(&target, "tmp");
        let bak = sibling_temp_path(&target, "bak");

        // 1. Fresh target (no pre-existing file)
        fs::write(&tmp, b"v1").unwrap();
        replace_file_with_backup(&tmp, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v1");
        assert!(!tmp.exists());
        assert!(!bak.exists());

        // 2. Overwrite existing target
        fs::write(&tmp, b"v2").unwrap();
        replace_file_with_backup(&tmp, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"v2");
        assert!(!tmp.exists());
        assert!(!bak.exists());

        // 3. Failure during rename (missing tmp) restores previous target
        let missing_tmp = dir.path().join("does_not_exist.tmp");
        assert!(replace_file_with_backup(&missing_tmp, &target).is_err());
        assert_eq!(
            fs::read(&target).unwrap(),
            b"v2",
            "existing file must be restored when replacement fails"
        );
        assert!(!bak.exists());
    }
}
