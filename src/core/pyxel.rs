//! Pyxel Edit (`.pyxel`) import and export codec.
//!
//! `.pyxel` files are standard ZIP archives (typically DEFLATE-compressed,
//! method 8) containing:
//! - `docData.json` — canvas dimensions, tile size, layers, tileset metadata,
//!   animations, and palette.
//! - `layer{i}.png` — RGBA pixel data for layer `i` (top-to-bottom order in
//!   Pyxel Edit).
//! - `tile{i}.png` — RGBA pixel data for tileset tile `i`.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};

use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};

use crate::core::buffer::PixelBuffer;
use crate::core::color::Color;
use crate::core::math::Rect2i;
use crate::core::model::sequence::AnimationSequence;
use crate::core::model::{AnimationSet, BlendMode, Frame, Layer, LayerId, LayerStack, Region};
use crate::core::palette::Palette;
use crate::core::png_codec::{decode_png, encode_png};
use crate::core::tilemap::{Tile, TileCell, TileId, TileMap, TilePalette, TileSize};

/// Everything extracted from a `.pyxel` archive, ready to inspect in the
/// Import Pyxel preview panel or convert into a `ProjectSession`.
pub struct PyxelImport {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub tile_size: TileSize,
    pub layers: LayerStack,
    pub tile_palette: TilePalette,
    pub animations: AnimationSet,
    pub palette: Option<Palette>,
    /// Composited RGBA8 preview (`width * height * 4` bytes) for the UI panel.
    pub preview_rgba: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyxelDocData {
    #[serde(default)]
    name: String,
    #[serde(default = "default_version")]
    version: String,
    canvas: PyxelCanvas,
    #[serde(default)]
    tileset: PyxelTileset,
    #[serde(default)]
    animations: BTreeMap<String, PyxelAnimation>,
    #[serde(default)]
    palette: PyxelPalette,
    #[serde(default)]
    settings: serde_json::Value,
}

fn default_version() -> String {
    "0.4.95".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyxelCanvas {
    width: usize,
    height: usize,
    #[serde(default = "default_tile_dim")]
    tile_width: u32,
    #[serde(default = "default_tile_dim")]
    tile_height: u32,
    #[serde(default)]
    num_layers: usize,
    #[serde(default)]
    current_layer_index: usize,
    #[serde(default)]
    layers: BTreeMap<String, PyxelLayer>,
    #[serde(default)]
    frame_duration: Option<u32>,
}

fn default_tile_dim() -> u32 {
    16
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyxelLayer {
    #[serde(default)]
    name: String,
    #[serde(default = "default_blend_mode")]
    blend_mode: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    collapsed: bool,
    #[serde(default)]
    soloed: bool,
    #[serde(default = "default_alpha")]
    alpha: u8,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    tile_refs: BTreeMap<String, PyxelTileRef>,
    #[serde(rename = "type", default = "default_layer_type")]
    layer_type: String,
    #[serde(default = "default_parent_index")]
    parent_index: i32,
}

fn default_blend_mode() -> String {
    "normal".to_owned()
}

fn default_alpha() -> u8 {
    255
}

fn default_layer_type() -> String {
    "tile_layer".to_owned()
}

fn default_parent_index() -> i32 {
    -1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyxelTileRef {
    index: usize,
    #[serde(default)]
    flip_x: bool,
    #[serde(default)]
    rot: u8,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyxelTileset {
    #[serde(default = "default_tile_dim")]
    tile_width: u32,
    #[serde(default = "default_tile_dim")]
    tile_height: u32,
    #[serde(default)]
    num_tiles: usize,
    #[serde(default = "default_tiles_wide")]
    tiles_wide: usize,
    #[serde(default = "default_fixed_width")]
    fixed_width: bool,
}

fn default_tiles_wide() -> usize {
    8
}

fn default_fixed_width() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyxelAnimation {
    #[serde(default)]
    name: String,
    #[serde(default)]
    base_tile: usize,
    #[serde(default)]
    length: usize,
    #[serde(default)]
    frame_duration: Option<u32>,
    #[serde(default)]
    frame_duration_multipliers: Vec<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PyxelPalette {
    #[serde(default)]
    num_colors: usize,
    #[serde(default = "default_palette_width")]
    width: usize,
    #[serde(default = "default_palette_height")]
    height: usize,
    #[serde(default)]
    colors: BTreeMap<String, Option<String>>,
}

fn default_palette_width() -> usize {
    12
}

fn default_palette_height() -> usize {
    5
}

/// Imports a `.pyxel` archive from raw bytes into [`PyxelImport`].
pub fn import_pyxel_bytes(bytes: &[u8], fallback_name: &str) -> Result<PyxelImport, String> {
    let entries = unpack_zip(bytes)?;
    let entry_map: HashMap<String, Vec<u8>> = entries.into_iter().collect();

    let doc_bytes = entry_map
        .get("docData.json")
        .ok_or_else(|| "Invalid .pyxel archive: missing docData.json".to_owned())?;
    let doc: PyxelDocData = serde_json::from_slice(doc_bytes)
        .map_err(|e| format!("Failed to parse docData.json: {e}"))?;

    let width = doc.canvas.width;
    let height = doc.canvas.height;
    if width == 0 || height == 0 {
        return Err("Invalid .pyxel canvas dimensions".to_owned());
    }
    let tile_w = doc.canvas.tile_width.max(1);
    let tile_h = doc.canvas.tile_height.max(1);
    let tile_size = TileSize::new(tile_w, tile_h);

    // 1. Tileset: tile0.png .. tile{numTiles-1}.png
    let mut tile_palette = TilePalette::new();
    let mut pyxel_tile_to_id: HashMap<usize, TileId> = HashMap::new();
    let num_tiles = doc.tileset.num_tiles;
    for idx in 0..num_tiles {
        let key = format!("tile{idx}.png");
        let pixels = if let Some(png_bytes) = entry_map.get(&key) {
            let decoded =
                decode_png(png_bytes).map_err(|e| format!("Failed to decode {key}: {e}"))?;
            if decoded.width == tile_w as usize && decoded.height == tile_h as usize {
                decoded.rgba
            } else {
                blit_to_size(
                    &decoded.rgba,
                    decoded.width,
                    decoded.height,
                    tile_w as usize,
                    tile_h as usize,
                )
            }
        } else {
            vec![0u8; (tile_w as usize) * (tile_h as usize) * 4]
        };
        let id = tile_palette.add(Tile {
            id: TileId(0),
            w: tile_w as u16,
            h: tile_h as u16,
            pixels,
        });
        pyxel_tile_to_id.insert(idx, id);
    }

    // 2. Layers: Pyxel Edit stores layers top-to-bottom ("0" = top).
    // Pyxross `LayerStack` is bottom-to-top, so we iterate in reverse.
    let mut sorted_layers: Vec<(usize, &PyxelLayer)> = doc
        .canvas
        .layers
        .iter()
        .filter_map(|(k, v)| k.parse::<usize>().ok().map(|idx| (idx, v)))
        .collect();
    sorted_layers.sort_by_key(|(idx, _)| *idx);

    let cols = ((width as u32) / tile_w).max(1);
    let rows = ((height as u32) / tile_h).max(1);

    let mut layers = LayerStack::new(width, height);
    let initial_id = layers.active_layer_id();
    let mut pyxel_idx_to_layer_id: HashMap<usize, LayerId> = HashMap::new();
    let mut built_layers: Vec<(usize, Layer)> = Vec::with_capacity(sorted_layers.len());
    let mut next_layer_id: u64 = 2;

    for (pyx_idx, pyx_layer) in sorted_layers.iter().rev() {
        let id = LayerId::new(next_layer_id);
        next_layer_id += 1;
        pyxel_idx_to_layer_id.insert(*pyx_idx, id);

        let is_group = pyx_layer.layer_type == "group_layer";
        let mut tilemap = if !is_group && !pyx_layer.tile_refs.is_empty() {
            let mut tm = TileMap::new(tile_size, cols, rows);
            for (cell_key, tref) in &pyx_layer.tile_refs {
                let Ok(linear) = cell_key.parse::<u32>() else {
                    continue;
                };
                let cx = linear % cols;
                let cy = linear / cols;
                if let Some(&tile_id) = pyxel_tile_to_id.get(&tref.index) {
                    let mut cell = TileCell::new(tile_id);
                    cell.flip_x = tref.flip_x;
                    cell.set_rotation(tref.rot);
                    tm.set_cell((cx, cy), Some(cell));
                }
            }
            Some(tm)
        } else {
            None
        };

        let mut buffer = PixelBuffer::new(width, height);
        if !is_group {
            let key = format!("layer{pyx_idx}.png");
            if let Some(png_bytes) = entry_map.get(&key) {
                let decoded =
                    decode_png(png_bytes).map_err(|e| format!("Failed to decode {key}: {e}"))?;
                let rgba = if decoded.width == width && decoded.height == height {
                    decoded.rgba
                } else {
                    blit_to_size(&decoded.rgba, decoded.width, decoded.height, width, height)
                };
                // If this layer has tileRefs, `layer{i}.png` in Pyxel Edit holds
                // the baked tile pixels; clear the occupied tile cells from
                // `buffer` so `tilemap` remains the single source of truth
                // without double-compositing semi-transparent pixels.
                if let Some(tm) = &mut tilemap {
                    let mut filtered = rgba;
                    for cy in 0..rows {
                        for cx in 0..cols {
                            if tm.cell((cx, cy)).is_some() {
                                clear_rect(&mut filtered, width, height, cx, cy, tile_w, tile_h);
                            }
                        }
                    }
                    buffer.as_bytes_mut().copy_from_slice(&filtered);
                } else {
                    buffer.as_bytes_mut().copy_from_slice(&rgba);
                }
            }
        }

        let name = if pyx_layer.name.trim().is_empty() {
            format!("Layer {}", pyx_idx + 1)
        } else {
            pyx_layer.name.clone()
        };

        built_layers.push((
            *pyx_idx,
            Layer {
                id,
                name,
                visible: !pyx_layer.hidden,
                opacity: (pyx_layer.alpha as f32 / 255.0).clamp(0.0, 1.0),
                blend: parse_blend_mode(&pyx_layer.blend_mode),
                buffer,
                parent: None,
                is_group,
                tilemap,
                locked: false,
                tilemap_cache: std::cell::RefCell::new(None),
            },
        ));
    }

    // Resolve parent group IDs after all IDs are assigned.
    let parent_lookup: HashMap<usize, i32> = sorted_layers
        .iter()
        .map(|(idx, l)| (*idx, l.parent_index))
        .collect();
    for (pyx_idx, layer) in &mut built_layers {
        if let Some(&p_idx) = parent_lookup.get(pyx_idx) {
            if p_idx >= 0 {
                layer.parent = pyxel_idx_to_layer_id.get(&(p_idx as usize)).copied();
            }
        }
    }

    if !built_layers.is_empty() {
        for (i, (_, layer)) in built_layers.into_iter().enumerate() {
            layers.insert_layer(layer, i);
        }
        layers.remove_layer(initial_id);
        let top_id = layers
            .iter()
            .nth(doc.canvas.current_layer_index.min(layers.len().saturating_sub(1)))
            .map(|l| l.id)
            .unwrap_or_else(|| layers.active_layer_id());
        let _ = layers.set_active(top_id);
    }

    // 3. Animations
    let mut animations = AnimationSet::new();
    let mut sorted_anims: Vec<(usize, &PyxelAnimation)> = doc
        .animations
        .iter()
        .filter_map(|(k, v)| k.parse::<usize>().ok().map(|idx| (idx, v)))
        .collect();
    sorted_anims.sort_by_key(|(idx, _)| *idx);

    let mut first_anim = true;
    let default_base_ms = doc.canvas.frame_duration.unwrap_or(100).max(1);
    for (_, anim) in sorted_anims {
        if anim.length == 0 {
            continue;
        }
        let seq_name = if anim.name.trim().is_empty() {
            "Animation".to_owned()
        } else {
            anim.name.clone()
        };
        let mut seq = AnimationSequence::new(seq_name);
        let base_ms = anim.frame_duration.unwrap_or(default_base_ms).max(1);
        for f in 0..anim.length {
            let tile_idx = anim.base_tile + f;
            let cx = (tile_idx as u32) % cols;
            let cy = (tile_idx as u32) / cols;
            let rect = Rect2i::new(
                (cx * tile_w) as i32,
                (cy * tile_h) as i32,
                tile_w as i32,
                tile_h as i32,
            );
            let mult = anim
                .frame_duration_multipliers
                .get(f)
                .copied()
                .unwrap_or(100)
                .max(1);
            let delay_ms = (base_ms.saturating_mul(mult) / 100).max(10);
            seq.push(Frame::new(Region::new(rect, format!("Frame {}", f + 1)), delay_ms));
        }
        if first_anim {
            if let Some(slot) = animations.get_mut(0) {
                *slot = seq;
            }
            first_anim = false;
        } else {
            let _ = animations.add(seq.name().to_owned());
            let last = animations.len().saturating_sub(1);
            if let Some(slot) = animations.get_mut(last) {
                *slot = seq;
            }
        }
    }

    // 4. Palette
    let mut pal_entries: Vec<(usize, Color)> = doc
        .palette
        .colors
        .iter()
        .filter_map(|(k, v)| {
            let idx = k.parse::<usize>().ok()?;
            let hex = v.as_deref()?;
            parse_argb_hex(hex).map(|c| (idx, c))
        })
        .collect();
    pal_entries.sort_by_key(|(idx, _)| *idx);
    let palette = if pal_entries.is_empty() {
        None
    } else {
        let colors: Vec<[u8; 4]> = pal_entries
            .into_iter()
            .map(|(_, c)| [c.r, c.g, c.b, c.a])
            .collect();
        Palette::new("Pyxel Palette", colors).ok()
    };

    let preview_rgba = layers.composite_layers(&tile_palette).as_bytes().to_vec();
    let name = if doc.name.trim().is_empty() {
        fallback_name.to_owned()
    } else {
        doc.name
    };

    Ok(PyxelImport {
        name,
        width,
        height,
        tile_size,
        layers,
        tile_palette,
        animations,
        palette,
        preview_rgba,
    })
}

/// Exports a Pyxross project into `.pyxel` ZIP archive bytes.
pub fn export_pyxel_bytes(
    name: &str,
    tile_size: TileSize,
    layers: &LayerStack,
    tile_palette: &TilePalette,
    animations: &AnimationSet,
    palette: Option<&Palette>,
) -> Result<Vec<u8>, String> {
    let width = layers.width();
    let height = layers.height();
    let tile_w = tile_size.w.max(1);
    let tile_h = tile_size.h.max(1);
    let cols = ((width as u32) / tile_w).max(1);
    let rows = ((height as u32) / tile_h).max(1);

    // 1. Map TileId -> Pyxel tile index (0..num_tiles) and encode `tile{i}.png`
    let mut tile_id_to_idx: HashMap<TileId, usize> = HashMap::new();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for (idx, tile) in tile_palette.tiles.iter().enumerate() {
        tile_id_to_idx.insert(tile.id, idx);
        let rgba = if tile.w as u32 == tile_w && tile.h as u32 == tile_h {
            tile.pixels.clone()
        } else {
            blit_to_size(
                &tile.pixels,
                tile.w as usize,
                tile.h as usize,
                tile_w as usize,
                tile_h as usize,
            )
        };
        let png = encode_png(tile_w as usize, tile_h as usize, &rgba)
            .map_err(|e| format!("Failed to encode tile{idx}.png: {e}"))?;
        entries.push((format!("tile{idx}.png"), png));
    }

    // 2. Layers: reverse Pyxross bottom-to-top into Pyxel Edit top-to-bottom.
    let mut layer_vec: Vec<&Layer> = layers.iter().collect();
    layer_vec.reverse();
    let id_to_pyxel_idx: HashMap<LayerId, usize> = layer_vec
        .iter()
        .enumerate()
        .map(|(idx, l)| (l.id, idx))
        .collect();

    let mut pyx_layers: BTreeMap<String, PyxelLayer> = BTreeMap::new();
    let mut current_layer_index = 0usize;
    let active_id = layers.active_layer_id();

    for (idx, layer) in layer_vec.iter().enumerate() {
        if layer.id == active_id {
            current_layer_index = idx;
        }
        let mut tile_refs = BTreeMap::new();
        let mut combined_rgba = layer.buffer.as_bytes().to_vec();
        if let Some(tm) = &layer.tilemap {
            // Composite tilemap pixels into `layer{idx}.png` so Pyxel Edit and
            // previews display the full layer even before tile re-bake.
            let mut tm_buf = PixelBuffer::new(width, height);
            tm.rasterize(tile_palette, &mut tm_buf);
            let tm_rgba = tm_buf.as_bytes();
            for p in (0..combined_rgba.len()).step_by(4) {
                if p + 3 < combined_rgba.len() && combined_rgba[p + 3] == 0 && tm_rgba[p + 3] > 0 {
                    combined_rgba[p..p + 4].copy_from_slice(&tm_rgba[p..p + 4]);
                }
            }
            for cy in 0..rows.min(tm.rows) {
                for cx in 0..cols.min(tm.cols) {
                    if let Some(cell) = tm.cell((cx, cy)) {
                        if let Some(&tile_idx) = tile_id_to_idx.get(&cell.tile_id) {
                            let linear = cy * cols + cx;
                            tile_refs.insert(
                                linear.to_string(),
                                PyxelTileRef {
                                    index: tile_idx,
                                    flip_x: cell.flip_x,
                                    rot: cell.rotation % 4,
                                },
                            );
                        }
                    }
                }
            }
        }
        let png = encode_png(width, height, &combined_rgba)
            .map_err(|e| format!("Failed to encode layer{idx}.png: {e}"))?;
        entries.push((format!("layer{idx}.png"), png));

        let parent_index = layer
            .parent
            .and_then(|pid| id_to_pyxel_idx.get(&pid))
            .map(|&p| p as i32)
            .unwrap_or(-1);

        pyx_layers.insert(
            idx.to_string(),
            PyxelLayer {
                name: layer.name.clone(),
                blend_mode: blend_mode_name(layer.blend).to_owned(),
                hidden: !layer.visible,
                collapsed: false,
                soloed: false,
                alpha: (layer.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
                muted: false,
                tile_refs,
                layer_type: if layer.is_group {
                    "group_layer".to_owned()
                } else {
                    "tile_layer".to_owned()
                },
                parent_index,
            },
        );
    }

    // 3. Animations
    let mut pyx_anims: BTreeMap<String, PyxelAnimation> = BTreeMap::new();
    for (idx, seq) in animations.iter().enumerate() {
        if seq.is_empty() {
            continue;
        }
        let first_rect = seq.frame(0).map(|f| f.region().rect());
        let base_tile = first_rect
            .map(|r| {
                let cx = (r.x.max(0) as u32) / tile_w;
                let cy = (r.y.max(0) as u32) / tile_h;
                (cy * cols + cx) as usize
            })
            .unwrap_or(0);
        let base_delay = seq.frame(0).map(|f| f.delay_ms().max(1)).unwrap_or(100);
        let mults: Vec<u32> = (0..seq.len())
            .filter_map(|i| seq.frame(i))
            .map(|f| ((f.delay_ms().max(1) * 100) / base_delay).max(1))
            .collect();
        pyx_anims.insert(
            idx.to_string(),
            PyxelAnimation {
                name: seq.name().to_owned(),
                base_tile,
                length: seq.len(),
                frame_duration: Some(base_delay),
                frame_duration_multipliers: mults,
            },
        );
    }

    // 4. Palette
    let mut pal_colors = BTreeMap::new();
    let num_colors = if let Some(pal) = palette {
        for (i, c) in pal.colors.iter().enumerate() {
            pal_colors.insert(
                i.to_string(),
                Some(format!("{:02x}{:02x}{:02x}{:02x}", c[3], c[0], c[1], c[2])),
            );
        }
        pal.colors.len()
    } else {
        0
    };

    let doc = PyxelDocData {
        name: name.to_owned(),
        version: default_version(),
        canvas: PyxelCanvas {
            width,
            height,
            tile_width: tile_w,
            tile_height: tile_h,
            num_layers: layer_vec.len(),
            current_layer_index,
            layers: pyx_layers,
            frame_duration: Some(100),
        },
        tileset: PyxelTileset {
            tile_width: tile_w,
            tile_height: tile_h,
            num_tiles: tile_palette.tiles.len(),
            tiles_wide: 8,
            fixed_width: true,
        },
        animations: pyx_anims,
        palette: PyxelPalette {
            num_colors,
            width: 12,
            height: 5.max(num_colors.div_ceil(12)),
            colors: pal_colors,
        },
        settings: serde_json::json!({}),
    };

    let doc_json =
        serde_json::to_vec_pretty(&doc).map_err(|e| format!("Failed to serialize docData.json: {e}"))?;
    entries.insert(0, ("docData.json".to_owned(), doc_json));

    let refs: Vec<(&str, &[u8])> = entries.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
    pack_zip(&refs)
}

fn parse_blend_mode(mode: &str) -> BlendMode {
    match mode.to_ascii_lowercase().as_str() {
        "multiply" => BlendMode::Multiply,
        "screen" => BlendMode::Screen,
        "add" | "linear dodge" | "lineardodge" => BlendMode::Add,
        _ => BlendMode::Normal,
    }
}

fn blend_mode_name(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "normal",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Add => "add",
    }
}

fn parse_argb_hex(s: &str) -> Option<Color> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() == 8 {
        let a = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let r = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let g = u8::from_str_radix(&hex[4..6], 16).ok()?;
        let b = u8::from_str_radix(&hex[6..8], 16).ok()?;
        Some(Color::rgba(r, g, b, a))
    } else if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Color::rgba(r, g, b, 255))
    } else {
        None
    }
}

fn blit_to_size(src: &[u8], src_w: usize, src_h: usize, dst_w: usize, dst_h: usize) -> Vec<u8> {
    let mut dst = vec![0u8; dst_w * dst_h * 4];
    let copy_w = src_w.min(dst_w);
    let copy_h = src_h.min(dst_h);
    for y in 0..copy_h {
        let s_off = y * src_w * 4;
        let d_off = y * dst_w * 4;
        let row_bytes = copy_w * 4;
        if s_off + row_bytes <= src.len() && d_off + row_bytes <= dst.len() {
            dst[d_off..d_off + row_bytes].copy_from_slice(&src[s_off..s_off + row_bytes]);
        }
    }
    dst
}

fn clear_rect(
    rgba: &mut [u8],
    canvas_w: usize,
    canvas_h: usize,
    cx: u32,
    cy: u32,
    tile_w: u32,
    tile_h: u32,
) {
    let x0 = (cx * tile_w) as usize;
    let y0 = (cy * tile_h) as usize;
    let x1 = (x0 + tile_w as usize).min(canvas_w);
    let y1 = (y0 + tile_h as usize).min(canvas_h);
    for y in y0..y1 {
        let start = (y * canvas_w + x0) * 4;
        let end = (y * canvas_w + x1) * 4;
        if end <= rgba.len() {
            rgba[start..end].fill(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal ZIP reader/writer supporting STORED (0) and DEFLATE (8).
// ---------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut hasher = flate2::Crc::new();
    hasher.update(data);
    hasher.sum()
}

fn read_u16_le(b: &[u8], off: usize) -> Result<u16, String> {
    let slice = b.get(off..off + 2).ok_or("Truncated ZIP")?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(b: &[u8], off: usize) -> Result<u32, String> {
    let slice = b.get(off..off + 4).ok_or("Truncated ZIP")?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn unpack_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    if bytes.len() < 22 {
        return Err("Invalid ZIP: file too short".to_owned());
    }
    let search_start = bytes.len().saturating_sub(22 + 65535);
    let mut eocd_off = None;
    for i in (search_start..=bytes.len() - 22).rev() {
        if bytes[i..i + 4] == [0x50, 0x4b, 0x05, 0x06] {
            let comment_len = u16::from_le_bytes([bytes[i + 20], bytes[i + 21]]) as usize;
            if i + 22 + comment_len == bytes.len() {
                eocd_off = Some(i);
                break;
            }
        }
    }
    let eocd = eocd_off.ok_or_else(|| "Invalid ZIP: EOCD not found".to_owned())?;
    let entry_count = read_u16_le(bytes, eocd + 10)? as usize;
    let cd_size = read_u32_le(bytes, eocd + 12)? as usize;
    let cd_offset = read_u32_le(bytes, eocd + 16)? as usize;
    if cd_offset + cd_size > eocd {
        return Err("Invalid ZIP: central directory out of bounds".to_owned());
    }

    let mut out = Vec::with_capacity(entry_count);
    let mut cursor = cd_offset;
    for _ in 0..entry_count {
        if bytes.get(cursor..cursor + 4) != Some(&[0x50, 0x4b, 0x01, 0x02]) {
            return Err("Invalid ZIP: bad central directory header".to_owned());
        }
        let method = read_u16_le(bytes, cursor + 10)?;
        let expected_crc = read_u32_le(bytes, cursor + 16)?;
        let comp_size = read_u32_le(bytes, cursor + 20)? as usize;
        let uncomp_size = read_u32_le(bytes, cursor + 24)? as usize;
        let name_len = read_u16_le(bytes, cursor + 28)? as usize;
        let extra_len = read_u16_le(bytes, cursor + 30)? as usize;
        let comment_len = read_u16_le(bytes, cursor + 32)? as usize;
        let local_off = read_u32_le(bytes, cursor + 42)? as usize;

        let name_bytes = bytes
            .get(cursor + 46..cursor + 46 + name_len)
            .ok_or("Truncated ZIP entry name")?;
        let name = std::str::from_utf8(name_bytes)
            .map_err(|_| "Non-UTF-8 ZIP entry name")?
            .to_owned();
        if name.starts_with('/') || name.starts_with('\\') || name.contains("..") {
            return Err(format!("Unsafe ZIP entry name: {name}"));
        }
        cursor += 46 + name_len + extra_len + comment_len;

        if bytes.get(local_off..local_off + 4) != Some(&[0x50, 0x4b, 0x03, 0x04]) {
            return Err("Invalid ZIP: bad local file header".to_owned());
        }
        let l_name_len = read_u16_le(bytes, local_off + 26)? as usize;
        let l_extra_len = read_u16_le(bytes, local_off + 28)? as usize;
        let data_start = local_off + 30 + l_name_len + l_extra_len;
        let comp_slice = bytes
            .get(data_start..data_start + comp_size)
            .ok_or("Truncated ZIP entry payload")?;

        let payload = match method {
            0 => comp_slice.to_vec(),
            8 => {
                let mut decoder = DeflateDecoder::new(comp_slice);
                let mut buf = Vec::with_capacity(uncomp_size);
                decoder
                    .read_to_end(&mut buf)
                    .map_err(|e| format!("Deflate decode failed for {name}: {e}"))?;
                buf
            }
            other => return Err(format!("Unsupported ZIP compression method {other}")),
        };
        if crc32(&payload) != expected_crc {
            return Err(format!("CRC-32 mismatch for {name}"));
        }
        out.push((name, payload));
    }
    Ok(out)
}

fn pack_zip(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    struct CdRecord<'a> {
        name: &'a [u8],
        crc: u32,
        comp_size: u32,
        uncomp_size: u32,
        local_offset: u32,
    }
    let mut cd = Vec::with_capacity(entries.len());

    for &(name, data) in entries {
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(data)
            .map_err(|e| format!("Deflate encode failed for {name}: {e}"))?;
        let comp = encoder
            .finish()
            .map_err(|e| format!("Deflate finish failed for {name}: {e}"))?;

        let local_offset = u32::try_from(out.len()).map_err(|_| "ZIP too large")?;
        let comp_size = u32::try_from(comp.len()).map_err(|_| "Entry too large")?;
        let uncomp_size = u32::try_from(data.len()).map_err(|_| "Entry too large")?;
        let name_len = u16::try_from(name_bytes.len()).map_err(|_| "Name too long")?;

        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&8u16.to_le_bytes()); // method: DEFLATE
        out.extend_from_slice(&0u16.to_le_bytes()); // mtime
        out.extend_from_slice(&0u16.to_le_bytes()); // mdate
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&comp_size.to_le_bytes());
        out.extend_from_slice(&uncomp_size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&comp);

        cd.push(CdRecord {
            name: name_bytes,
            crc,
            comp_size,
            uncomp_size,
            local_offset,
        });
    }

    let cd_offset = u32::try_from(out.len()).map_err(|_| "ZIP too large")?;
    for rec in &cd {
        let name_len = rec.name.len() as u16;
        out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&8u16.to_le_bytes()); // method: DEFLATE
        out.extend_from_slice(&0u16.to_le_bytes()); // mtime
        out.extend_from_slice(&0u16.to_le_bytes()); // mdate
        out.extend_from_slice(&rec.crc.to_le_bytes());
        out.extend_from_slice(&rec.comp_size.to_le_bytes());
        out.extend_from_slice(&rec.uncomp_size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out.extend_from_slice(&0u16.to_le_bytes()); // disk start
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&rec.local_offset.to_le_bytes());
        out.extend_from_slice(rec.name);
    }
    let cd_size = u32::try_from(out.len() - cd_offset as usize).map_err(|_| "CD too large")?;
    let count = u16::try_from(cd.len()).map_err(|_| "Too many entries")?;

    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_example_pyxel_with_tiles_layers_and_palette() {
        let bytes = include_bytes!("../../example.pyxel");
        let imp = import_pyxel_bytes(bytes, "fallback").expect("example.pyxel should import");
        assert_eq!(imp.name, "dungeon tileset1");
        assert_eq!(imp.width, 256);
        assert_eq!(imp.height, 256);
        assert_eq!(imp.tile_size, TileSize::new(16, 16));
        assert_eq!(imp.layers.len(), 3);
        assert_eq!(imp.tile_palette.tiles.len(), 3);
        assert!(imp.palette.is_some());
        assert_eq!(imp.preview_rgba.len(), 256 * 256 * 4);
        // Layer 0 in example.pyxel had 15 tileRefs.
        let layer0 = imp
            .layers
            .iter()
            .find(|l| l.name == "Layer 0")
            .expect("Layer 0 should exist");
        let tm = layer0.tilemap.as_ref().expect("Layer 0 should have tilemap");
        assert!(tm.cell((2, 0)).is_some());
        assert!(tm.cell((2, 2)).is_some());
        assert!(tm.cell((4, 5)).is_some());
    }

    #[test]
    fn export_and_reimport_round_trip_preserves_document() {
        let bytes = include_bytes!("../../example.pyxel");
        let imp = import_pyxel_bytes(bytes, "fallback").unwrap();
        let exported = export_pyxel_bytes(
            &imp.name,
            imp.tile_size,
            &imp.layers,
            &imp.tile_palette,
            &imp.animations,
            imp.palette.as_ref(),
        )
        .expect("export should succeed");
        let reimported = import_pyxel_bytes(&exported, "reimport").expect("reimport should succeed");
        assert_eq!(reimported.name, imp.name);
        assert_eq!(reimported.width, imp.width);
        assert_eq!(reimported.height, imp.height);
        assert_eq!(reimported.tile_size, imp.tile_size);
        assert_eq!(reimported.layers.len(), imp.layers.len());
        assert_eq!(reimported.tile_palette.tiles.len(), imp.tile_palette.tiles.len());
        assert_eq!(
            reimported.palette.as_ref().map(|p| p.colors.len()),
            imp.palette.as_ref().map(|p| p.colors.len())
        );
    }
}
