//! Aseprite (`.ase` / `.aseprite`) binary import and export codec.
//!
//! Imports Aseprite timelines into Pyxross region sheets:
//! - Each Aseprite frame's canvas size becomes one Pyxross region size.
//! - Frames are arranged into a sheet using `RowLength`, `SingleRow`, or
//!   `SplitByTags` (where `row_length = max_tag_len` and each tag occupies its
//!   own row left-to-right, leaving trailing cells empty).
//! - Each Aseprite tag becomes a Pyxross `AnimationSequence` with frame delays.
//!
//! Exports Pyxross animations back into a valid 32-bit RGBA `.ase` file with
//! layers, zlib-compressed cels, frame delays, tags, and palette.

use std::collections::HashMap;
use std::io::{Read, Write};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::core::buffer::PixelBuffer;
use crate::core::math::Rect2i;
use crate::core::model::sequence::AnimationSequence;
use crate::core::model::{AnimationSet, BlendMode, Frame, Layer, LayerId, LayerStack, Region};
use crate::core::palette::Palette;
use crate::core::tilemap::{TilePalette, TileSize};

/// Upper bound on palette entries read from a `.ase` file to guard against
/// malformed/malicious headers causing OOM.
const MAX_PALETTE_COLORS: usize = 65_536;
/// Upper bound on decompressed zlib payload bytes for a single cel or tileset.
const MAX_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsepriteLayoutMode {
    RowLength(u32),
    SingleRow,
    SplitByTags,
    SingleCanvas { px_w: u32, px_h: u32 },
}

#[derive(Clone, Debug)]
pub struct AsepriteLayerInfo {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub is_background: bool,
    pub is_group: bool,
    pub is_tilemap: bool,
    pub tileset_index: Option<u32>,
    pub opacity: u8,
    pub blend: BlendMode,
    pub child_level: u16,
}

#[derive(Clone, Debug)]
pub struct AsepriteTag {
    pub name: String,
    pub from_frame: usize,
    pub to_frame: usize,
    /// Aseprite loop direction: 0 = Forward, 1 = Reverse, 2 = Ping-Pong, 3 = Ping-Pong Reverse.
    pub direction: u8,
    pub repeat: u16,
}

#[derive(Clone, Debug)]
pub struct AsepriteCel {
    pub layer_index: usize,
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    pub opacity: u8,
    pub z_index: i16,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AsepriteFrame {
    pub duration_ms: u32,
    pub cels: Vec<AsepriteCel>,
}

#[derive(Clone, Debug)]
pub struct ParsedAseprite {
    pub frame_w: u32,
    pub frame_h: u32,
    pub layers: Vec<AsepriteLayerInfo>,
    pub frames: Vec<AsepriteFrame>,
    pub tags: Vec<AsepriteTag>,
    pub palette: Option<Palette>,
}

pub struct AsepriteImportResult {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub frame_w: u32,
    pub frame_h: u32,
    pub layers: LayerStack,
    pub animations: AnimationSet,
    pub palette: Option<Palette>,
    pub preview_rgba: Vec<u8>,
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    fn skip(&mut self, n: usize) -> Result<(), String> {
        if self.remaining() < n {
            return Err("Unexpected EOF in .ase file".to_owned());
        }
        self.pos += n;
        Ok(())
    }
    fn read_u8(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.pos).ok_or("Unexpected EOF in .ase")?;
        self.pos += 1;
        Ok(b)
    }
    fn read_u16(&mut self) -> Result<u16, String> {
        let s = self
            .data
            .get(self.pos..self.pos + 2)
            .ok_or("Unexpected EOF in .ase")?;
        self.pos += 2;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn read_i16(&mut self) -> Result<i16, String> {
        Ok(self.read_u16()? as i16)
    }
    fn read_u32(&mut self) -> Result<u32, String> {
        let s = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or("Unexpected EOF in .ase")?;
        self.pos += 4;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or("Unexpected EOF in .ase")?;
        self.pos += n;
        Ok(s)
    }
    fn read_string(&mut self) -> Result<String, String> {
        let len = self.read_u16()? as usize;
        let bytes = self.read_bytes(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

enum RawCelKind {
    Image {
        w: usize,
        h: usize,
        raw: Vec<u8>,
    },
    Linked {
        linked_frame: usize,
    },
    Tilemap {
        cols: usize,
        rows: usize,
        bits_per_tile: u16,
        mask_id: u32,
        mask_x: u32,
        mask_y: u32,
        mask_diag: u32,
        raw: Vec<u8>,
    },
}

struct RawCel {
    layer_index: usize,
    x: i32,
    y: i32,
    opacity: u8,
    z_index: i16,
    kind: RawCelKind,
}

struct RawFrame {
    duration_ms: u32,
    cels: Vec<RawCel>,
}

struct RawTileset {
    tile_count: usize,
    tile_w: usize,
    tile_h: usize,
    raw_pixels: Vec<u8>,
}

struct DecodedTileset {
    tile_count: usize,
    tile_w: usize,
    tile_h: usize,
    rgba: Vec<u8>,
}

fn read_zlib_bounded(compressed: &[u8], context: &str) -> Result<Vec<u8>, String> {
    let mut dec = ZlibDecoder::new(compressed).take((MAX_DECOMPRESSED_BYTES as u64) + 1);
    let mut raw = Vec::new();
    dec.read_to_end(&mut raw)
        .map_err(|e| format!("Failed to decompress {context}: {e}"))?;
    if raw.len() > MAX_DECOMPRESSED_BYTES {
        return Err(format!(
            "Decompressed {context} exceeds {} byte limit",
            MAX_DECOMPRESSED_BYTES
        ));
    }
    Ok(raw)
}

/// Parses raw `.ase` / `.aseprite` bytes into frames, layers, tags, and palette.
pub fn parse_aseprite_bytes(bytes: &[u8]) -> Result<ParsedAseprite, String> {
    if bytes.len() < 128 {
        return Err("File too short to be an Aseprite document".to_owned());
    }
    let mut cur = Cursor::new(bytes);
    let _file_size = cur.read_u32()?;
    let magic = cur.read_u16()?;
    if magic != 0xA5E0 {
        return Err(format!("Invalid .ase header magic: 0x{magic:04x}"));
    }
    let frame_count = cur.read_u16()? as usize;
    let width = cur.read_u16()? as u32;
    let height = cur.read_u16()? as u32;
    if width == 0 || height == 0 {
        return Err("Aseprite canvas dimensions must be non-zero".to_owned());
    }
    let color_depth = cur.read_u16()?;
    if !matches!(color_depth, 8 | 16 | 32) {
        return Err(format!("Unsupported Aseprite color depth: {color_depth}"));
    }
    let header_flags = cur.read_u32()?;
    let layer_opacity_valid = (header_flags & 1) != 0;
    let default_speed = cur.read_u16()?.max(1) as u32;
    cur.skip(8)?; // reserved
    let transparent_idx = cur.read_u8()?;
    cur.skip(3)?;
    let _num_colors = cur.read_u16()?;
    cur.skip(94)?; // pixel ratio, grid, reserved to 128

    let mut layers: Vec<AsepriteLayerInfo> = Vec::new();
    let mut raw_frames: Vec<RawFrame> = Vec::with_capacity(frame_count);
    let mut tags: Vec<AsepriteTag> = Vec::new();
    let mut raw_tilesets: HashMap<u32, RawTileset> = HashMap::new();
    let mut pal_rgba: Vec<[u8; 4]> = Vec::new();
    let mut has_new_palette = false;
    let mut has_old_palette = false;

    for frame_idx in 0..frame_count {
        if cur.remaining() < 16 {
            break;
        }
        let frame_start = cur.pos;
        let frame_bytes = cur.read_u32()? as usize;
        let frame_magic = cur.read_u16()?;
        if frame_magic != 0xF1FA {
            return Err(format!("Invalid frame {frame_idx} magic: 0x{frame_magic:04x}"));
        }
        let old_chunks = cur.read_u16()? as u32;
        let duration_ms = cur.read_u16()?;
        let duration_ms = if duration_ms == 0 {
            default_speed
        } else {
            duration_ms as u32
        };
        cur.skip(2)?;
        let new_chunks = cur.read_u32()?;
        let chunk_count = if new_chunks != 0 {
            new_chunks
        } else {
            old_chunks
        };

        let mut cels: Vec<RawCel> = Vec::new();
        for _ in 0..chunk_count {
            if cur.remaining() < 6 {
                break;
            }
            let chunk_start = cur.pos;
            let chunk_size = cur.read_u32()? as usize;
            let chunk_type = cur.read_u16()?;
            let chunk_end = chunk_start.saturating_add(chunk_size).min(bytes.len());
            let payload_len = chunk_end.saturating_sub(cur.pos);
            let payload = cur.read_bytes(payload_len)?;
            let mut cc = Cursor::new(payload);

            match chunk_type {
                0x2004 => {
                    // Layer chunk
                    let l_flags = cc.read_u16()?;
                    let l_type = cc.read_u16()?;
                    let child_level = cc.read_u16()?;
                    cc.skip(4)?; // default w/h
                    let blend_raw = cc.read_u16()?;
                    let raw_opacity = cc.read_u8()?;
                    let opacity = if layer_opacity_valid { raw_opacity } else { 255 };
                    cc.skip(3)?;
                    let name = cc.read_string()?;
                    let tileset_index = if l_type == 2 && cc.remaining() >= 4 {
                        Some(cc.read_u32()?)
                    } else {
                        None
                    };
                    layers.push(AsepriteLayerInfo {
                        name,
                        visible: (l_flags & 1) != 0,
                        locked: (l_flags & 2) == 0,
                        is_background: (l_flags & 8) != 0,
                        is_group: l_type == 1,
                        is_tilemap: l_type == 2,
                        tileset_index,
                        opacity,
                        blend: match blend_raw {
                            0 => BlendMode::Normal,
                            1 => BlendMode::Multiply,
                            2 => BlendMode::Screen,
                            3 => BlendMode::Overlay,
                            4 => BlendMode::Darken,
                            5 => BlendMode::Lighten,
                            6 => BlendMode::ColorDodge,
                            7 => BlendMode::ColorBurn,
                            8 => BlendMode::HardLight,
                            9 => BlendMode::SoftLight,
                            10 => BlendMode::Difference,
                            11 => BlendMode::Exclusion,
                            12 => BlendMode::Hue,
                            13 => BlendMode::Saturation,
                            14 => BlendMode::Color,
                            15 => BlendMode::Luminosity,
                            16 => BlendMode::Add,
                            17 => BlendMode::Subtract,
                            18 => BlendMode::Divide,
                            _ => BlendMode::Normal,
                        },
                        child_level,
                    });
                }
                0x2005 => {
                    // Cel chunk
                    let layer_index = cc.read_u16()? as usize;
                    let x = cc.read_i16()? as i32;
                    let y = cc.read_i16()? as i32;
                    let opacity = cc.read_u8()?;
                    let cel_type = cc.read_u16()?;
                    let z_index = cc.read_i16()?;
                    cc.skip(5)?;
                    match cel_type {
                        0 => {
                            let cw = cc.read_u16()? as usize;
                            let ch = cc.read_u16()? as usize;
                            let raw = cc.read_bytes(cc.remaining())?.to_vec();
                            cels.push(RawCel {
                                layer_index,
                                x,
                                y,
                                opacity,
                                z_index,
                                kind: RawCelKind::Image { w: cw, h: ch, raw },
                            });
                        }
                        1 => {
                            // Linked cel
                            let linked_frame = cc.read_u16()? as usize;
                            cels.push(RawCel {
                                layer_index,
                                x,
                                y,
                                opacity,
                                z_index,
                                kind: RawCelKind::Linked { linked_frame },
                            });
                        }
                        2 => {
                            let cw = cc.read_u16()? as usize;
                            let ch = cc.read_u16()? as usize;
                            let compressed = cc.read_bytes(cc.remaining())?;
                            let raw = read_zlib_bounded(compressed, "cel")?;
                            cels.push(RawCel {
                                layer_index,
                                x,
                                y,
                                opacity,
                                z_index,
                                kind: RawCelKind::Image { w: cw, h: ch, raw },
                            });
                        }
                        3 => {
                            // Compressed Tilemap (Aseprite 1.3+)
                            let cols = cc.read_u16()? as usize;
                            let rows = cc.read_u16()? as usize;
                            let bits_per_tile = cc.read_u16()?;
                            let mask_id = cc.read_u32()?;
                            let mask_x = cc.read_u32()?;
                            let mask_y = cc.read_u32()?;
                            let mask_diag = cc.read_u32()?;
                            cc.skip(10)?; // reserved
                            let compressed = cc.read_bytes(cc.remaining())?;
                            let raw = read_zlib_bounded(compressed, "tilemap cel")?;
                            cels.push(RawCel {
                                layer_index,
                                x,
                                y,
                                opacity,
                                z_index,
                                kind: RawCelKind::Tilemap {
                                    cols,
                                    rows,
                                    bits_per_tile,
                                    mask_id,
                                    mask_x,
                                    mask_y,
                                    mask_diag,
                                    raw,
                                },
                            });
                        }
                        _ => {}
                    }
                }
                0x2018 => {
                    // Tags chunk
                    let num_tags = cc.read_u16()? as usize;
                    cc.skip(8)?;
                    for _ in 0..num_tags {
                        if cc.remaining() < 17 {
                            break;
                        }
                        let from_frame = cc.read_u16()? as usize;
                        let to_frame = cc.read_u16()? as usize;
                        let direction = cc.read_u8()?;
                        let repeat = cc.read_u16()?;
                        cc.skip(10)?; // reserved(6) + rgb(3) + extra(1)
                        let name = cc.read_string()?;
                        tags.push(AsepriteTag {
                            name,
                            from_frame,
                            to_frame,
                            direction,
                            repeat,
                        });
                    }
                }
                0x2019 => {
                    // Palette chunk (authoritative)
                    let new_size = cc.read_u32()? as usize;
                    if new_size > MAX_PALETTE_COLORS {
                        return Err(format!(
                            "Aseprite palette size {new_size} exceeds limit ({MAX_PALETTE_COLORS})"
                        ));
                    }
                    let first = cc.read_u32()? as usize;
                    let last = cc.read_u32()? as usize;
                    cc.skip(8)?;
                    if new_size > 0 {
                        pal_rgba.resize(new_size, [0, 0, 0, 255]);
                    }
                    if first <= last {
                        for idx in first..=last {
                            if cc.remaining() < 6 {
                                break;
                            }
                            let flags = cc.read_u16()?;
                            let r = cc.read_u8()?;
                            let g = cc.read_u8()?;
                            let b = cc.read_u8()?;
                            let a = cc.read_u8()?;
                            if (flags & 1) != 0 {
                                let _ = cc.read_string()?;
                            }
                            if idx < pal_rgba.len() {
                                pal_rgba[idx] = [r, g, b, a];
                            }
                        }
                    }
                    has_new_palette = true;
                }
                0x0004 if !has_new_palette => {
                    // Old palette chunk (0..255 RGB)
                    let packets = cc.read_u16()? as usize;
                    let mut idx = 0usize;
                    for _ in 0..packets {
                        if cc.remaining() < 2 {
                            break;
                        }
                        idx = idx.saturating_add(cc.read_u8()? as usize);
                        let cnt_raw = cc.read_u8()? as usize;
                        let cnt = if cnt_raw == 0 { 256 } else { cnt_raw };
                        for _ in 0..cnt {
                            if cc.remaining() < 3 {
                                break;
                            }
                            let r = cc.read_u8()?;
                            let g = cc.read_u8()?;
                            let b = cc.read_u8()?;
                            if idx < MAX_PALETTE_COLORS {
                                if idx >= pal_rgba.len() {
                                    pal_rgba.resize(idx + 1, [0, 0, 0, 255]);
                                }
                                pal_rgba[idx] = [r, g, b, 255];
                            }
                            idx = idx.saturating_add(1);
                        }
                    }
                    has_old_palette = true;
                }
                0x0011 if !has_new_palette && !has_old_palette => {
                    // Old palette chunk 2 (0..63 RGB)
                    let packets = cc.read_u16()? as usize;
                    let mut idx = 0usize;
                    for _ in 0..packets {
                        if cc.remaining() < 2 {
                            break;
                        }
                        idx = idx.saturating_add(cc.read_u8()? as usize);
                        let cnt_raw = cc.read_u8()? as usize;
                        let cnt = if cnt_raw == 0 { 256 } else { cnt_raw };
                        for _ in 0..cnt {
                            if cc.remaining() < 3 {
                                break;
                            }
                            let r = ((cc.read_u8()? as u16).min(63) * 255 / 63) as u8;
                            let g = ((cc.read_u8()? as u16).min(63) * 255 / 63) as u8;
                            let b = ((cc.read_u8()? as u16).min(63) * 255 / 63) as u8;
                            if idx < MAX_PALETTE_COLORS {
                                if idx >= pal_rgba.len() {
                                    pal_rgba.resize(idx + 1, [0, 0, 0, 255]);
                                }
                                pal_rgba[idx] = [r, g, b, 255];
                            }
                            idx = idx.saturating_add(1);
                        }
                    }
                    has_old_palette = true;
                }
                0x2023 => {
                    // Tileset chunk (Aseprite 1.3+)
                    if cc.remaining() >= 28 {
                        let id = cc.read_u32()?;
                        let ts_flags = cc.read_u32()?;
                        let tile_count = cc.read_u32()? as usize;
                        let tile_w = cc.read_u16()? as usize;
                        let tile_h = cc.read_u16()? as usize;
                        let _base_index = cc.read_i16()?;
                        cc.skip(14)?; // reserved
                        let _name = cc.read_string()?;
                        if (ts_flags & 1) != 0 {
                            // External file link: file_id (u32) + tileset_id (u32)
                            cc.skip(8)?;
                        }
                        if (ts_flags & 2) != 0 && cc.remaining() >= 4 {
                            let comp_len = cc.read_u32()? as usize;
                            let comp = cc.read_bytes(comp_len.min(cc.remaining()))?;
                            let raw_pixels = read_zlib_bounded(comp, "tileset")?;
                            raw_tilesets.insert(
                                id,
                                RawTileset {
                                    tile_count,
                                    tile_w,
                                    tile_h,
                                    raw_pixels,
                                },
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        raw_frames.push(RawFrame { duration_ms, cels });
        let next_frame = frame_start.saturating_add(frame_bytes);
        if next_frame > cur.pos && next_frame <= bytes.len() {
            cur.pos = next_frame;
        }
    }

    if layers.is_empty() {
        layers.push(AsepriteLayerInfo {
            name: "Layer 1".to_owned(),
            visible: true,
            locked: false,
            is_background: false,
            is_group: false,
            is_tilemap: false,
            tileset_index: None,
            opacity: 255,
            blend: BlendMode::Normal,
            child_level: 0,
        });
    }

    // Pass 2: decode tilesets and cels using the final resolved palette and layer metadata.
    let mut decoded_tilesets: HashMap<u32, DecodedTileset> = HashMap::new();
    for (id, rts) in raw_tilesets {
        let total_h = rts.tile_h.saturating_mul(rts.tile_count);
        let rgba = decode_cel_pixels(
            &rts.raw_pixels,
            rts.tile_w,
            total_h,
            color_depth,
            transparent_idx,
            false,
            &pal_rgba,
        )?;
        decoded_tilesets.insert(
            id,
            DecodedTileset {
                tile_count: rts.tile_count,
                tile_w: rts.tile_w,
                tile_h: rts.tile_h,
                rgba,
            },
        );
    }

    let mut frames: Vec<AsepriteFrame> = Vec::with_capacity(raw_frames.len());
    for rf in raw_frames {
        let mut cels: Vec<AsepriteCel> = Vec::with_capacity(rf.cels.len());
        for rc in rf.cels {
            let is_bg = layers
                .get(rc.layer_index)
                .is_some_and(|l| l.is_background);
            match rc.kind {
                RawCelKind::Image { w, h, raw } => {
                    let rgba = decode_cel_pixels(
                        &raw,
                        w,
                        h,
                        color_depth,
                        transparent_idx,
                        is_bg,
                        &pal_rgba,
                    )?;
                    cels.push(AsepriteCel {
                        layer_index: rc.layer_index,
                        x: rc.x,
                        y: rc.y,
                        w,
                        h,
                        opacity: rc.opacity,
                        z_index: rc.z_index,
                        rgba,
                    });
                }
                RawCelKind::Linked { linked_frame } => {
                    if let Some(src_cel) = frames
                        .get(linked_frame)
                        .and_then(|f| f.cels.iter().find(|c| c.layer_index == rc.layer_index))
                    {
                        let mut cloned = src_cel.clone();
                        cloned.opacity = rc.opacity;
                        cloned.z_index = rc.z_index;
                        cels.push(cloned);
                    }
                }
                RawCelKind::Tilemap {
                    cols,
                    rows,
                    bits_per_tile,
                    mask_id,
                    mask_x,
                    mask_y,
                    mask_diag,
                    raw,
                } => {
                    let ts_id = layers
                        .get(rc.layer_index)
                        .and_then(|l| l.tileset_index);
                    if let Some(ts) = ts_id.and_then(|id| decoded_tilesets.get(&id)) {
                        if let Some((pw, ph, rgba)) = decode_tilemap_cel(
                            &raw,
                            cols,
                            rows,
                            bits_per_tile,
                            mask_id,
                            mask_x,
                            mask_y,
                            mask_diag,
                            ts,
                        ) {
                            cels.push(AsepriteCel {
                                layer_index: rc.layer_index,
                                x: rc.x,
                                y: rc.y,
                                w: pw,
                                h: ph,
                                opacity: rc.opacity,
                                z_index: rc.z_index,
                                rgba,
                            });
                        }
                    }
                }
            }
        }
        frames.push(AsepriteFrame {
            duration_ms: rf.duration_ms,
            cels,
        });
    }

    let palette = if (has_new_palette || has_old_palette) && !pal_rgba.is_empty() {
        Palette::new("Aseprite Palette", pal_rgba).ok()
    } else {
        None
    };

    Ok(ParsedAseprite {
        frame_w: width,
        frame_h: height,
        layers,
        frames,
        tags,
        palette,
    })
}

fn decode_cel_pixels(
    raw: &[u8],
    w: usize,
    h: usize,
    depth: u16,
    transparent_idx: u8,
    is_background: bool,
    pal: &[[u8; 4]],
) -> Result<Vec<u8>, String> {
    let px_count = w
        .checked_mul(h)
        .ok_or_else(|| "Aseprite cel dimensions overflow".to_owned())?;
    let byte_len = px_count
        .checked_mul(4)
        .ok_or_else(|| "Aseprite cel byte count overflow".to_owned())?;
    if byte_len > MAX_DECOMPRESSED_BYTES * 4 {
        return Err("Aseprite cel RGBA buffer exceeds safety limit".to_owned());
    }
    let mut out = vec![0u8; byte_len];
    match depth {
        32 => {
            let copy = raw.len().min(byte_len);
            out[..copy].copy_from_slice(&raw[..copy]);
        }
        16 => {
            for i in 0..px_count {
                let off = i * 2;
                if off + 1 >= raw.len() {
                    break;
                }
                let v = raw[off];
                let a = raw[off + 1];
                out[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, a]);
            }
        }
        8 => {
            for i in 0..px_count {
                if i >= raw.len() {
                    break;
                }
                let idx = raw[i];
                if !is_background && idx == transparent_idx {
                    out[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 0]);
                } else {
                    let c = pal.get(idx as usize).copied().unwrap_or([0, 0, 0, 255]);
                    out[i * 4..i * 4 + 4].copy_from_slice(&c);
                }
            }
        }
        _ => {}
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn decode_tilemap_cel(
    raw: &[u8],
    cols: usize,
    rows: usize,
    bits_per_tile: u16,
    mask_id: u32,
    mask_x: u32,
    mask_y: u32,
    mask_diag: u32,
    ts: &DecodedTileset,
) -> Option<(usize, usize, Vec<u8>)> {
    if cols == 0 || rows == 0 || ts.tile_w == 0 || ts.tile_h == 0 {
        return None;
    }
    let bytes_per_tile = match bits_per_tile {
        8 => 1usize,
        16 => 2usize,
        32 => 4usize,
        _ => return None,
    };
    let pw = cols.checked_mul(ts.tile_w)?;
    let ph = rows.checked_mul(ts.tile_h)?;
    let byte_len = pw.checked_mul(ph)?.checked_mul(4)?;
    if byte_len > MAX_DECOMPRESSED_BYTES * 4 {
        return None;
    }
    let mut out = vec![0u8; byte_len];
    let tile_stride = ts.tile_w.checked_mul(ts.tile_h)?.checked_mul(4)?;

    for ty in 0..rows {
        for tx in 0..cols {
            let off = (ty * cols + tx) * bytes_per_tile;
            let raw_val = match bytes_per_tile {
                1 => *raw.get(off)? as u32,
                2 => u16::from_le_bytes([*raw.get(off)?, *raw.get(off + 1)?]) as u32,
                4 => u32::from_le_bytes([
                    *raw.get(off)?,
                    *raw.get(off + 1)?,
                    *raw.get(off + 2)?,
                    *raw.get(off + 3)?,
                ]),
                _ => return None,
            };
            let tile_id = (raw_val & mask_id) as usize;
            if tile_id == 0 || tile_id >= ts.tile_count {
                continue;
            }
            let flip_x = mask_x != 0 && (raw_val & mask_x) != 0;
            let flip_y = mask_y != 0 && (raw_val & mask_y) != 0;
            let flip_diag = mask_diag != 0 && (raw_val & mask_diag) != 0;
            let tile_base = tile_id * tile_stride;
            if tile_base + tile_stride > ts.rgba.len() {
                continue;
            }

            for py in 0..ts.tile_h {
                for px in 0..ts.tile_w {
                    let (mut sx, mut sy) = (px, py);
                    if flip_diag && ts.tile_w == ts.tile_h {
                        std::mem::swap(&mut sx, &mut sy);
                    }
                    if flip_x {
                        sx = ts.tile_w - 1 - sx;
                    }
                    if flip_y {
                        sy = ts.tile_h - 1 - sy;
                    }
                    let src_off = tile_base + (sy * ts.tile_w + sx) * 4;
                    let dst_off = ((ty * ts.tile_h + py) * pw + (tx * ts.tile_w + px)) * 4;
                    out[dst_off..dst_off + 4].copy_from_slice(&ts.rgba[src_off..src_off + 4]);
                }
            }
        }
    }
    Some((pw, ph, out))
}

/// Returns the ordered list of source frame indices for an Aseprite tag,
/// respecting its loop direction (Forward, Reverse, Ping-Pong, Ping-Pong Reverse).
fn tag_frame_indices(tag: &AsepriteTag, frame_count: usize) -> Vec<usize> {
    if frame_count == 0 {
        return Vec::new();
    }
    let to = tag.to_frame.min(frame_count - 1);
    let from = tag.from_frame.min(to);
    match tag.direction {
        1 => (from..=to).rev().collect(),
        2 => {
            let mut out: Vec<usize> = (from..=to).collect();
            if to > from + 1 {
                out.extend(((from + 1)..to).rev());
            }
            out
        }
        3 => {
            let mut out: Vec<usize> = (from..=to).rev().collect();
            if to > from + 1 {
                out.extend((from + 1)..to);
            }
            out
        }
        _ => (from..=to).collect(),
    }
}

/// Returns frame indices not covered by any tag in `parsed.tags`.
fn untagged_frame_indices(parsed: &ParsedAseprite) -> Vec<usize> {
    let n = parsed.frames.len();
    if n == 0 || parsed.tags.is_empty() {
        return Vec::new();
    }
    let mut covered = vec![false; n];
    for tag in &parsed.tags {
        let to = tag.to_frame.min(n - 1);
        let from = tag.from_frame.min(to);
        for slot in &mut covered[from..=to] {
            *slot = true;
        }
    }
    covered
        .into_iter()
        .enumerate()
        .filter_map(|(i, c)| (!c).then_some(i))
        .collect()
}

/// Computes `(cols, rows)` for a given [`AsepriteLayoutMode`].
pub fn layout_grid(parsed: &ParsedAseprite, mode: AsepriteLayoutMode) -> (usize, usize) {
    let n = parsed.frames.len().max(1);
    match mode {
        AsepriteLayoutMode::SingleRow => (n, 1),
        AsepriteLayoutMode::RowLength(cols) => {
            let c = (cols as usize).max(1);
            let r = n.div_ceil(c);
            (c, r)
        }
        AsepriteLayoutMode::SplitByTags => {
            if parsed.tags.is_empty() {
                (n, 1)
            } else {
                let mut max_tag = parsed
                    .tags
                    .iter()
                    .map(|t| {
                        let to = t.to_frame.min(n.saturating_sub(1));
                        let from = t.from_frame.min(to);
                        to - from + 1
                    })
                    .max()
                    .unwrap_or(1)
                    .max(1);
                let untagged = untagged_frame_indices(parsed);
                let mut rows = parsed.tags.len();
                if !untagged.is_empty() {
                    max_tag = max_tag.max(untagged.len());
                    rows += 1;
                }
                (max_tag, rows)
            }
        }
        AsepriteLayoutMode::SingleCanvas { px_w, px_h } => {
            let (rw, rh) = crate::core::import::regions_from_pixels(
                parsed.frame_w.max(1),
                parsed.frame_h.max(1),
                px_w.max(1),
                px_h.max(1),
            );
            (rw.max(1) as usize, rh.max(1) as usize)
        }
    }
}

/// Converts a [`ParsedAseprite`] into a Pyxross sprite-sheet project according to
/// `mode`.
pub fn build_aseprite_import(
    parsed: &ParsedAseprite,
    name: &str,
    mode: AsepriteLayoutMode,
) -> Result<AsepriteImportResult, String> {
    if parsed.frames.is_empty() {
        return Err("Aseprite file contains no frames".to_owned());
    }
    let (cols, rows) = layout_grid(parsed, mode);
    let fw = parsed.frame_w as usize;
    let fh = parsed.frame_h as usize;
    let (cell_w, cell_h) = match mode {
        AsepriteLayoutMode::SingleCanvas { px_w, px_h } => (px_w.max(1), px_h.max(1)),
        _ => (parsed.frame_w.max(1), parsed.frame_h.max(1)),
    };
    let width = cols.saturating_mul(cell_w as usize).max(fw).max(1);
    let height = rows.saturating_mul(cell_h as usize).max(fh).max(1);
    let buf_bytes = width
        .checked_mul(height)
        .and_then(|p| p.checked_mul(4))
        .ok_or_else(|| "Imported Aseprite canvas dimensions overflow memory".to_owned())?;

    let mut layer_stack = LayerStack::try_new(width, height)
        .ok_or_else(|| "Imported Aseprite canvas exceeds addressable memory".to_owned())?;
    let initial_id = layer_stack.active_layer_id();

    // Build Pyxross layers matching Aseprite's bottom-to-top layer order.
    let mut layer_buffers: Vec<Vec<u8>> = (0..parsed.layers.len())
        .map(|i| {
            if parsed.layers[i].is_group {
                Vec::new()
            } else {
                vec![0u8; buf_bytes]
            }
        })
        .collect();

    let mut animations = AnimationSet::new();
    let mut first_anim = true;

    let mut blit_frame_at = |frame_idx: usize, gx: usize, gy: usize| {
        let Some(frame) = parsed.frames.get(frame_idx) else {
            return;
        };
        let dst_ox = gx * fw;
        let dst_oy = gy * fh;
        let mut ordered: Vec<&AsepriteCel> = frame.cels.iter().collect();
        ordered.sort_by_key(|c| (c.layer_index as i32 + c.z_index as i32, c.z_index));
        for cel in ordered {
            let Some(buf) = layer_buffers.get_mut(cel.layer_index) else {
                continue;
            };
            if buf.is_empty() {
                continue;
            }
            for cy in 0..cel.h {
                let py = cel.y + cy as i32;
                if py < 0 || py >= fh as i32 {
                    continue;
                }
                for cx in 0..cel.w {
                    let px = cel.x + cx as i32;
                    if px < 0 || px >= fw as i32 {
                        continue;
                    }
                    let src_off = (cy * cel.w + cx) * 4;
                    let dst_off = ((dst_oy + py as usize) * width + (dst_ox + px as usize)) * 4;
                    if src_off + 3 < cel.rgba.len() && dst_off + 3 < buf.len() {
                        let mut px_rgba = [
                            cel.rgba[src_off],
                            cel.rgba[src_off + 1],
                            cel.rgba[src_off + 2],
                            cel.rgba[src_off + 3],
                        ];
                        if cel.opacity < 255 {
                            px_rgba[3] =
                                ((px_rgba[3] as u16 * cel.opacity as u16 + 127) / 255) as u8;
                        }
                        buf[dst_off..dst_off + 4].copy_from_slice(&px_rgba);
                    }
                }
            }
        }
    };

    let mut push_animation = |seq: AnimationSequence| {
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
    };

    match mode {
        AsepriteLayoutMode::SingleCanvas { .. } => {
            blit_frame_at(0, 0, 0);
        }
        AsepriteLayoutMode::SplitByTags if !parsed.tags.is_empty() => {
            for (row_idx, tag) in parsed.tags.iter().enumerate() {
                let to = tag.to_frame.min(parsed.frames.len() - 1);
                let from = tag.from_frame.min(to);
                // Blit each unique frame of the tag left-to-right on its row.
                for (col_idx, frame_idx) in (from..=to).enumerate() {
                    blit_frame_at(frame_idx, col_idx, row_idx);
                }
                let mut seq = AnimationSequence::new(tag.name.clone());
                for frame_idx in tag_frame_indices(tag, parsed.frames.len()) {
                    let col_idx = frame_idx.saturating_sub(from);
                    let rect = Rect2i::new(
                        (col_idx * fw) as i32,
                        (row_idx * fh) as i32,
                        fw as i32,
                        fh as i32,
                    );
                    let delay = parsed.frames[frame_idx].duration_ms.max(10);
                    seq.push(Frame::new(
                        Region::new(rect, format!("Frame {}", frame_idx + 1)),
                        delay,
                    ));
                }
                push_animation(seq);
            }
            // Preserve any frames not covered by tags on an extra "Untagged" row.
            let untagged = untagged_frame_indices(parsed);
            if !untagged.is_empty() {
                let row_idx = parsed.tags.len();
                let mut seq = AnimationSequence::new("Untagged");
                for (col_idx, frame_idx) in untagged.into_iter().enumerate() {
                    blit_frame_at(frame_idx, col_idx, row_idx);
                    let rect = Rect2i::new(
                        (col_idx * fw) as i32,
                        (row_idx * fh) as i32,
                        fw as i32,
                        fh as i32,
                    );
                    let delay = parsed.frames[frame_idx].duration_ms.max(10);
                    seq.push(Frame::new(
                        Region::new(rect, format!("Frame {}", frame_idx + 1)),
                        delay,
                    ));
                }
                push_animation(seq);
            }
        }
        _ => {
            for frame_idx in 0..parsed.frames.len() {
                let gx = frame_idx % cols;
                let gy = frame_idx / cols;
                blit_frame_at(frame_idx, gx, gy);
            }
            if parsed.tags.is_empty() {
                let mut seq = AnimationSequence::new("Animation 1");
                for frame_idx in 0..parsed.frames.len() {
                    let gx = frame_idx % cols;
                    let gy = frame_idx / cols;
                    let rect = Rect2i::new(
                        (gx * fw) as i32,
                        (gy * fh) as i32,
                        fw as i32,
                        fh as i32,
                    );
                    let delay = parsed.frames[frame_idx].duration_ms.max(10);
                    seq.push(Frame::new(
                        Region::new(rect, format!("Frame {}", frame_idx + 1)),
                        delay,
                    ));
                }
                push_animation(seq);
            } else {
                for tag in &parsed.tags {
                    let mut seq = AnimationSequence::new(tag.name.clone());
                    for frame_idx in tag_frame_indices(tag, parsed.frames.len()) {
                        let gx = frame_idx % cols;
                        let gy = frame_idx / cols;
                        let rect = Rect2i::new(
                            (gx * fw) as i32,
                            (gy * fh) as i32,
                            fw as i32,
                            fh as i32,
                        );
                        let delay = parsed.frames[frame_idx].duration_ms.max(10);
                        seq.push(Frame::new(
                            Region::new(rect, format!("Frame {}", frame_idx + 1)),
                            delay,
                        ));
                    }
                    push_animation(seq);
                }
            }
        }
    }

    // Populate LayerStack with layers.
    let mut parent_stack: Vec<(u16, LayerId)> = Vec::new();
    for (idx, info) in parsed.layers.iter().enumerate() {
        let id = LayerId::new((idx as u64) + 2);
        while parent_stack
            .last()
            .is_some_and(|(lvl, _)| *lvl >= info.child_level)
        {
            parent_stack.pop();
        }
        let parent = parent_stack.last().map(|(_, pid)| *pid);
        let mut buffer = PixelBuffer::new(width, height);
        if !info.is_group {
            if let Some(raw) = layer_buffers.get(idx) {
                if raw.len() == buffer.as_bytes().len() {
                    buffer.as_bytes_mut().copy_from_slice(raw);
                }
            }
        }
        let layer = Layer {
            id,
            name: if info.name.trim().is_empty() {
                format!("Layer {}", idx + 1)
            } else {
                info.name.clone()
            },
            visible: info.visible,
            opacity: (info.opacity as f32 / 255.0).clamp(0.0, 1.0),
            blend: info.blend,
            buffer,
            parent,
            is_group: info.is_group,
            tilemap: None,
            locked: info.locked,
            tilemap_cache: std::cell::RefCell::new(None),
        };
        layer_stack.insert_layer(layer, idx);
        if info.is_group {
            parent_stack.push((info.child_level, id));
        }
    }
    layer_stack.remove_layer(initial_id);
    if layer_stack.active_layer().is_group {
        if let Some(drawable) = layer_stack.iter().filter(|l| !l.is_group).last() {
            let id = drawable.id;
            let _ = layer_stack.set_active(id);
        }
    }

    let empty_tiles = TilePalette::new();
    let preview_rgba = layer_stack.composite_layers(&empty_tiles).as_bytes().to_vec();

    Ok(AsepriteImportResult {
        name: name.to_owned(),
        width,
        height,
        frame_w: cell_w,
        frame_h: cell_h,
        layers: layer_stack,
        animations,
        palette: parsed.palette.clone(),
        preview_rgba,
    })
}

/// Precomputed preview frames for the Export `.ase` panel.
#[derive(Clone, Debug)]
pub struct AsepriteExportPreview {
    pub unified_w: usize,
    pub unified_h: usize,
    pub unified_rgba: Vec<u8>,
    pub frames: Vec<(usize, usize, Vec<u8>)>,
    pub total_timeline_frames: usize,
    pub frame_w: usize,
    pub frame_h: usize,
}

fn aseprite_timeline_rects(
    layers: &LayerStack,
    animations: &AnimationSet,
    tile_size: TileSize,
    unified_frames: bool,
) -> (Vec<(Rect2i, u32)>, Vec<AsepriteTag>) {
    let cw = layers.width();
    let ch = layers.height();
    let mut timeline: Vec<(Rect2i, u32)> = Vec::new();
    let mut tags: Vec<AsepriteTag> = Vec::new();

    if unified_frames {
        timeline.push((Rect2i::new(0, 0, cw as i32, ch as i32), 100));
        return (timeline, tags);
    }

    for seq in animations.iter() {
        if seq.is_empty() {
            continue;
        }
        let from_frame = timeline.len();
        for i in 0..seq.len() {
            if let Some(f) = seq.frame(i) {
                timeline.push((f.region().rect(), f.delay_ms().max(1)));
            }
        }
        let to_frame = timeline.len().saturating_sub(1);
        if to_frame >= from_frame {
            tags.push(AsepriteTag {
                name: seq.name().to_owned(),
                from_frame,
                to_frame,
                direction: 0,
                repeat: 0,
            });
        }
    }

    if timeline.is_empty() {
        let tw = (tile_size.w.max(1) as usize).min(cw.max(1));
        let th = (tile_size.h.max(1) as usize).min(ch.max(1));
        let cols = (cw / tw).max(1);
        let rows = (ch / th).max(1);
        for cy in 0..rows {
            for cx in 0..cols {
                timeline.push((
                    Rect2i::new(
                        (cx * tw) as i32,
                        (cy * th) as i32,
                        tw as i32,
                        th as i32,
                    ),
                    100,
                ));
            }
        }
    }

    (timeline, tags)
}

/// Builds both the unified canvas preview and up to `max_frames` timeline
/// frame previews for the Export `.ase` panel.
pub fn aseprite_export_preview(
    layers: &LayerStack,
    tile_palette: &TilePalette,
    animations: &AnimationSet,
    tile_size: TileSize,
    max_frames: usize,
) -> AsepriteExportPreview {
    let composited = layers.composite_layers(tile_palette);
    let unified_w = composited.width();
    let unified_h = composited.height();
    let unified_rgba = composited.as_bytes().to_vec();

    let (timeline, _) = aseprite_timeline_rects(layers, animations, tile_size, false);
    let total_timeline_frames = timeline.len();
    let frame_w = timeline
        .iter()
        .map(|(r, _)| r.w.max(1) as usize)
        .max()
        .unwrap_or(unified_w.max(1));
    let frame_h = timeline
        .iter()
        .map(|(r, _)| r.h.max(1) as usize)
        .max()
        .unwrap_or(unified_h.max(1));

    let frames = timeline
        .iter()
        .take(max_frames)
        .map(|(rect, _)| {
            let rgba = crop_rgba(
                &unified_rgba,
                unified_w,
                unified_h,
                *rect,
                frame_w,
                frame_h,
            );
            (frame_w, frame_h, rgba)
        })
        .collect();

    AsepriteExportPreview {
        unified_w,
        unified_h,
        unified_rgba,
        frames,
        total_timeline_frames,
        frame_w,
        frame_h,
    }
}

/// Computes the nesting depth (`child_level`) of `layer` in `layers`.
fn layer_child_level(layers: &LayerStack, layer: &Layer) -> u16 {
    let mut depth = 0u16;
    let mut cur = layer.parent;
    let mut hops = 0usize;
    while let Some(pid) = cur {
        depth = depth.saturating_add(1);
        hops += 1;
        if hops > layers.len() {
            break;
        }
        cur = layers.layer(pid).and_then(|p| p.parent);
    }
    depth
}

/// Exports Pyxross animations (or region grid / unified canvas) and layers into
/// a 32-bit RGBA `.ase` binary.
pub fn export_aseprite_bytes(
    layers: &LayerStack,
    tile_palette: &TilePalette,
    animations: &AnimationSet,
    palette: Option<&Palette>,
    tile_size: TileSize,
    unified_frames: bool,
) -> Result<Vec<u8>, String> {
    let cw = layers.width();
    let ch = layers.height();
    let (timeline, tags) = aseprite_timeline_rects(layers, animations, tile_size, unified_frames);

    if timeline.is_empty() {
        return Err("No frames to export".to_owned());
    }
    let frame_count_u16 = u16::try_from(timeline.len())
        .map_err(|_| format!("Too many frames for .ase format: {} (max 65535)", timeline.len()))?;
    let max_fw = timeline
        .iter()
        .map(|(r, _)| r.w.max(1) as usize)
        .max()
        .unwrap_or(1);
    let max_fh = timeline
        .iter()
        .map(|(r, _)| r.h.max(1) as usize)
        .max()
        .unwrap_or(1);
    let frame_w = u16::try_from(max_fw)
        .map_err(|_| format!("Frame width {max_fw} exceeds .ase limit (65535)"))?;
    let frame_h = u16::try_from(max_fh)
        .map_err(|_| format!("Frame height {max_fh} exceeds .ase limit (65535)"))?;

    let all_layers: Vec<&Layer> = layers.iter().collect();
    if !all_layers.iter().any(|l| !l.is_group) {
        return Err("No drawable layers to export".to_owned());
    }
    let _ = u16::try_from(all_layers.len())
        .map_err(|_| format!("Too many layers for .ase format: {}", all_layers.len()))?;

    // Pre-rasterize each drawable layer using Pyxross's canonical effective buffer
    // (tilemap composited OVER pixel buffer, matching composite_layer_over).
    // Group layers are preserved in the layer list with their child_level and carry no cels.
    let mut export_entries: Vec<(&Layer, u16, Option<Vec<u8>>)> =
        Vec::with_capacity(all_layers.len());
    for l in all_layers {
        let child_level = layer_child_level(layers, l);
        let pixels = if l.is_group {
            None
        } else {
            let eff = l.effective_pixel_buffer(tile_palette);
            Some(eff.as_bytes().to_vec())
        };
        export_entries.push((l, child_level, pixels));
    }

    let mut body = Vec::new();

    for (frame_idx, (rect, delay_ms)) in timeline.iter().enumerate() {
        let mut chunks: Vec<(u16, Vec<u8>)> = Vec::new();
        if frame_idx == 0 {
            for (l, child_level, _) in &export_entries {
                chunks.push((0x2004, encode_layer_chunk(l, *child_level)));
            }
            if let Some(pal) = palette {
                if !pal.colors.is_empty() {
                    chunks.push((0x2019, encode_palette_chunk(pal)));
                }
            }
            if !tags.is_empty() {
                chunks.push((0x2018, encode_tags_chunk(&tags)?));
            }
        }
        for (layer_idx, (_, _, maybe_buf)) in export_entries.iter().enumerate() {
            let Some(buf) = maybe_buf else {
                continue;
            };
            let cel_rgba = crop_rgba(buf, cw, ch, *rect, frame_w as usize, frame_h as usize);
            // Skip completely transparent cels (valid in Aseprite and reduces file size).
            if cel_rgba.chunks_exact(4).all(|px| px[3] == 0) {
                continue;
            }
            chunks.push((
                0x2005,
                encode_cel_chunk(layer_idx as u16, frame_w, frame_h, &cel_rgba)?,
            ));
        }

        let mut frame_buf = Vec::new();
        let count_u32 = u32::try_from(chunks.len())
            .map_err(|_| "Too many chunks in frame".to_owned())?;
        let old_count = count_u32.min(0xFFFF) as u16;
        let mut chunk_stream = Vec::new();
        for (ctype, payload) in chunks {
            let csize = u32::try_from(6usize.saturating_add(payload.len()))
                .map_err(|_| "Chunk payload exceeds .ase 32-bit size limit".to_owned())?;
            chunk_stream.extend_from_slice(&csize.to_le_bytes());
            chunk_stream.extend_from_slice(&ctype.to_le_bytes());
            chunk_stream.extend_from_slice(&payload);
        }
        let total_frame_bytes = u32::try_from(16usize.saturating_add(chunk_stream.len()))
            .map_err(|_| "Frame exceeds .ase 32-bit size limit".to_owned())?;
        frame_buf.extend_from_slice(&total_frame_bytes.to_le_bytes());
        frame_buf.extend_from_slice(&0xF1FAu16.to_le_bytes());
        frame_buf.extend_from_slice(&old_count.to_le_bytes());
        frame_buf.extend_from_slice(&((*delay_ms).min(u16::MAX as u32) as u16).to_le_bytes());
        frame_buf.extend_from_slice(&0u16.to_le_bytes());
        frame_buf.extend_from_slice(&count_u32.to_le_bytes());
        frame_buf.extend_from_slice(&chunk_stream);
        body.extend_from_slice(&frame_buf);
    }

    let file_size = u32::try_from(128usize.saturating_add(body.len()))
        .map_err(|_| "Exported .ase file exceeds 4 GiB format limit".to_owned())?;
    let mut header = Vec::with_capacity(128);
    header.extend_from_slice(&file_size.to_le_bytes());
    header.extend_from_slice(&0xA5E0u16.to_le_bytes());
    header.extend_from_slice(&frame_count_u16.to_le_bytes());
    header.extend_from_slice(&frame_w.to_le_bytes());
    header.extend_from_slice(&frame_h.to_le_bytes());
    header.extend_from_slice(&32u16.to_le_bytes()); // 32-bit RGBA
    header.extend_from_slice(&1u32.to_le_bytes()); // layer opacity valid
    header.extend_from_slice(&100u16.to_le_bytes()); // speed
    header.extend_from_slice(&[0u8; 8]);
    header.push(0); // transparent index
    header.extend_from_slice(&[0u8; 3]);
    let pal_count = palette.map_or(0, |p| p.colors.len().min(u16::MAX as usize)) as u16;
    header.extend_from_slice(&pal_count.to_le_bytes());
    header.push(1);
    header.push(1);
    header.extend_from_slice(&0i16.to_le_bytes());
    header.extend_from_slice(&0i16.to_le_bytes());
    header.extend_from_slice(&frame_w.to_le_bytes());
    header.extend_from_slice(&frame_h.to_le_bytes());
    header.resize(128, 0);

    header.extend_from_slice(&body);
    Ok(header)
}

fn encode_ase_string(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    let len = b.len().min(u16::MAX as usize) as u16;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&b[..len as usize]);
}

fn encode_layer_chunk(l: &Layer, child_level: u16) -> Vec<u8> {
    let mut out = Vec::new();
    let mut flags: u16 = 0;
    if l.visible {
        flags |= 1; // Visible
    }
    if !l.locked {
        flags |= 2; // Editable
    }
    let ltype: u16 = if l.is_group { 1 } else { 0 };
    let blend: u16 = match l.blend {
        BlendMode::Normal => 0,
        BlendMode::Multiply => 1,
        BlendMode::Screen => 2,
        BlendMode::Overlay => 3,
        BlendMode::Darken => 4,
        BlendMode::Lighten => 5,
        BlendMode::ColorDodge => 6,
        BlendMode::ColorBurn => 7,
        BlendMode::HardLight => 8,
        BlendMode::SoftLight => 9,
        BlendMode::Difference => 10,
        BlendMode::Exclusion => 11,
        BlendMode::Hue => 12,
        BlendMode::Saturation => 13,
        BlendMode::Color => 14,
        BlendMode::Luminosity => 15,
        BlendMode::Add => 16,
        BlendMode::Subtract => 17,
        BlendMode::Divide => 18,
        // Modes without a native Aseprite ID map to their closest Aseprite counterpart.
        BlendMode::LinearBurn => 7,
        BlendMode::DarkerColor => 4,
        BlendMode::LighterColor => 5,
        BlendMode::VividLight | BlendMode::LinearLight | BlendMode::PinLight | BlendMode::HardMix => 8,
        BlendMode::Invert => 10,
    };
    let opacity = (l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&ltype.to_le_bytes());
    out.extend_from_slice(&child_level.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&blend.to_le_bytes());
    out.push(opacity);
    out.extend_from_slice(&[0u8; 3]);
    encode_ase_string(&mut out, &l.name);
    out
}

fn encode_palette_chunk(pal: &Palette) -> Vec<u8> {
    let mut out = Vec::new();
    let count = pal.colors.len() as u32;
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&count.saturating_sub(1).to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    for c in &pal.colors {
        out.extend_from_slice(&0u16.to_le_bytes()); // no name
        out.extend_from_slice(c);
    }
    out
}

fn encode_tags_chunk(tags: &[AsepriteTag]) -> Result<Vec<u8>, String> {
    let tag_count = u16::try_from(tags.len())
        .map_err(|_| format!("Too many animation tags for .ase format: {}", tags.len()))?;
    let mut out = Vec::new();
    out.extend_from_slice(&tag_count.to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    for t in tags {
        let from = u16::try_from(t.from_frame)
            .map_err(|_| "Tag from_frame exceeds .ase limit (65535)".to_owned())?;
        let to = u16::try_from(t.to_frame)
            .map_err(|_| "Tag to_frame exceeds .ase limit (65535)".to_owned())?;
        out.extend_from_slice(&from.to_le_bytes());
        out.extend_from_slice(&to.to_le_bytes());
        out.push(t.direction);
        out.extend_from_slice(&t.repeat.to_le_bytes());
        out.extend_from_slice(&[0u8; 6]);
        out.extend_from_slice(&[255, 255, 255]); // tag color
        out.push(0);
        encode_ase_string(&mut out, &t.name);
    }
    Ok(out)
}

fn encode_cel_chunk(layer_idx: u16, w: u16, h: u16, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(rgba)
        .map_err(|e| format!("Failed to compress cel: {e}"))?;
    let comp = enc
        .finish()
        .map_err(|e| format!("Failed to finish cel compression: {e}"))?;
    let mut out = Vec::with_capacity(20 + comp.len());
    out.extend_from_slice(&layer_idx.to_le_bytes());
    out.extend_from_slice(&0i16.to_le_bytes()); // x
    out.extend_from_slice(&0i16.to_le_bytes()); // y
    out.push(255); // opacity
    out.extend_from_slice(&2u16.to_le_bytes()); // compressed image
    out.extend_from_slice(&0i16.to_le_bytes()); // z_index
    out.extend_from_slice(&[0u8; 5]);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&comp);
    Ok(out)
}

fn crop_rgba(
    src: &[u8],
    sw: usize,
    sh: usize,
    rect: Rect2i,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; dst_w * dst_h * 4];
    let copy_w = dst_w.min(rect.w.max(0) as usize);
    let copy_h = dst_h.min(rect.h.max(0) as usize);
    for y in 0..copy_h {
        let sy = rect.y + y as i32;
        if sy < 0 || sy >= sh as i32 {
            continue;
        }
        for x in 0..copy_w {
            let sx = rect.x + x as i32;
            if sx < 0 || sx >= sw as i32 {
                continue;
            }
            let s_off = (sy as usize * sw + sx as usize) * 4;
            let d_off = (y * dst_w + x) * 4;
            if s_off + 3 < src.len() {
                out[d_off..d_off + 4].copy_from_slice(&src[s_off..s_off + 4]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_layer_info(name: &str) -> AsepriteLayerInfo {
        AsepriteLayerInfo {
            name: name.to_owned(),
            visible: true,
            locked: false,
            is_background: false,
            is_group: false,
            is_tilemap: false,
            tileset_index: None,
            opacity: 255,
            blend: BlendMode::Normal,
            child_level: 0,
        }
    }

    #[test]
    fn split_by_tags_matches_user_example_24_frames_8_12_4() {
        let parsed = ParsedAseprite {
            frame_w: 16,
            frame_h: 16,
            layers: vec![test_layer_info("Layer 1")],
            frames: (0..24)
                .map(|i| AsepriteFrame {
                    duration_ms: 80 + i as u32,
                    cels: Vec::new(),
                })
                .collect(),
            tags: vec![
                AsepriteTag {
                    name: "Tag 1".to_owned(),
                    from_frame: 0,
                    to_frame: 7,
                    direction: 0,
                    repeat: 0,
                },
                AsepriteTag {
                    name: "Tag 2".to_owned(),
                    from_frame: 8,
                    to_frame: 19,
                    direction: 0,
                    repeat: 0,
                },
                AsepriteTag {
                    name: "Tag 3".to_owned(),
                    from_frame: 20,
                    to_frame: 23,
                    direction: 0,
                    repeat: 0,
                },
            ],
            palette: None,
        };
        let (cols, rows) = layout_grid(&parsed, AsepriteLayoutMode::SplitByTags);
        assert_eq!(cols, 12);
        assert_eq!(rows, 3);

        let imp = build_aseprite_import(&parsed, "Test", AsepriteLayoutMode::SplitByTags).unwrap();
        assert_eq!(imp.width, 12 * 16);
        assert_eq!(imp.height, 3 * 16);
        assert_eq!(imp.animations.len(), 3);
        assert_eq!(imp.animations.get(0).unwrap().len(), 8);
        assert_eq!(imp.animations.get(1).unwrap().len(), 12);
        assert_eq!(imp.animations.get(2).unwrap().len(), 4);
        // Row 0 -> y=0, Row 1 -> y=16, Row 2 -> y=32
        assert_eq!(
            imp.animations.get(0).unwrap().frame(0).unwrap().region().rect(),
            Rect2i::new(0, 0, 16, 16)
        );
        assert_eq!(
            imp.animations.get(1).unwrap().frame(0).unwrap().region().rect(),
            Rect2i::new(0, 16, 16, 16)
        );
        assert_eq!(
            imp.animations.get(2).unwrap().frame(0).unwrap().region().rect(),
            Rect2i::new(0, 32, 16, 16)
        );
    }

    #[test]
    fn export_and_reimport_ase_round_trip_preserves_frames_tags_and_palette() {
        let layers = LayerStack::new(32, 16);
        let mut anims = AnimationSet::new();
        {
            let s = anims.get_mut(0).unwrap();
            *s = AnimationSequence::new("Idle");
            s.push(Frame::new(Region::new(Rect2i::new(0, 0, 16, 16), "F1"), 120));
            s.push(Frame::new(Region::new(Rect2i::new(16, 0, 16, 16), "F2"), 150));
        }
        let tiles = TilePalette::new();
        let pal = Palette::new("Test", vec![[255, 0, 0, 255], [0, 255, 0, 255]]).unwrap();
        let bytes = export_aseprite_bytes(
            &layers,
            &tiles,
            &anims,
            Some(&pal),
            TileSize::new(16, 16),
            false,
        )
        .unwrap();
        let parsed = parse_aseprite_bytes(&bytes).unwrap();
        assert_eq!(parsed.frame_w, 16);
        assert_eq!(parsed.frame_h, 16);
        assert_eq!(parsed.frames.len(), 2);
        assert_eq!(parsed.frames[0].duration_ms, 120);
        assert_eq!(parsed.frames[1].duration_ms, 150);
        assert_eq!(parsed.tags.len(), 1);
        assert_eq!(parsed.tags[0].name, "Idle");
        assert_eq!(parsed.palette.as_ref().unwrap().colors.len(), 2);
        // Exported unlocked layer must be editable (not locked) in Aseprite.
        assert!(!parsed.layers[0].locked);
    }

    #[test]
    fn export_ase_unified_frames_vs_region_grid_without_animations() {
        let layers = LayerStack::new(32, 32);
        let tiles = TilePalette::new();
        let anims = AnimationSet::new(); // 0 frames

        let unified = export_aseprite_bytes(
            &layers,
            &tiles,
            &anims,
            None,
            TileSize::new(16, 16),
            true,
        )
        .unwrap();
        let p_uni = parse_aseprite_bytes(&unified).unwrap();
        assert_eq!(p_uni.frame_w, 32);
        assert_eq!(p_uni.frame_h, 32);
        assert_eq!(p_uni.frames.len(), 1);
        assert!(p_uni.tags.is_empty());

        let grid = export_aseprite_bytes(
            &layers,
            &tiles,
            &anims,
            None,
            TileSize::new(16, 16),
            false,
        )
        .unwrap();
        let p_grid = parse_aseprite_bytes(&grid).unwrap();
        assert_eq!(p_grid.frame_w, 16);
        assert_eq!(p_grid.frame_h, 16);
        assert_eq!(p_grid.frames.len(), 4); // 2x2 grid of 16x16 regions
    }

    #[test]
    fn single_canvas_import_uses_first_frame_only_and_splits_regions() {
        let parsed = ParsedAseprite {
            frame_w: 32,
            frame_h: 16,
            layers: vec![test_layer_info("Layer 1")],
            frames: vec![
                AsepriteFrame {
                    duration_ms: 100,
                    cels: vec![AsepriteCel {
                        layer_index: 0,
                        x: 0,
                        y: 0,
                        w: 1,
                        h: 1,
                        opacity: 255,
                        z_index: 0,
                        rgba: vec![200, 10, 10, 255],
                    }],
                },
                AsepriteFrame {
                    duration_ms: 100,
                    cels: vec![AsepriteCel {
                        layer_index: 0,
                        x: 0,
                        y: 0,
                        w: 1,
                        h: 1,
                        opacity: 255,
                        z_index: 0,
                        rgba: vec![10, 200, 10, 255],
                    }],
                },
            ],
            tags: vec![AsepriteTag {
                name: "Run".to_owned(),
                from_frame: 0,
                to_frame: 1,
                direction: 0,
                repeat: 0,
            }],
            palette: None,
        };
        let imp = build_aseprite_import(
            &parsed,
            "Single",
            AsepriteLayoutMode::SingleCanvas { px_w: 16, px_h: 16 },
        )
        .unwrap();
        assert_eq!((imp.width, imp.height), (32, 16));
        assert_eq!((imp.frame_w, imp.frame_h), (16, 16));
        assert_eq!(imp.animations.len(), 1);
        assert!(imp.animations.get(0).unwrap().is_empty());
        let px = imp.layers.active_layer().buffer.get_pixel(0, 0).unwrap();
        assert_eq!((px.r, px.g, px.b, px.a), (200, 10, 10, 255));
    }

    #[test]
    fn aseprite_export_preview_caps_timeline_at_four_and_includes_unified() {
        let layers = LayerStack::new(96, 16);
        let tiles = TilePalette::new();
        let mut anims = AnimationSet::new();
        {
            let s = anims.get_mut(0).unwrap();
            *s = AnimationSequence::new("Walk");
            for i in 0..6 {
                s.push(Frame::new(
                    Region::new(Rect2i::new(i * 16, 0, 16, 16), format!("F{i}")),
                    100,
                ));
            }
        }
        let preview = aseprite_export_preview(&layers, &tiles, &anims, TileSize::new(16, 16), 4);
        assert_eq!((preview.unified_w, preview.unified_h), (96, 16));
        assert_eq!(preview.total_timeline_frames, 6);
        assert_eq!(preview.frames.len(), 4);
        assert_eq!((preview.frame_w, preview.frame_h), (16, 16));
    }

    #[test]
    fn export_and_reimport_preserves_groups_child_level_and_locked_flag() {
        let mut layers = LayerStack::new(16, 16);
        let group_id = LayerId::new(10);
        let child_id = LayerId::new(11);
        let initial_id = layers.active_layer_id();

        let group = Layer {
            id: group_id,
            name: "Characters".to_owned(),
            visible: false,
            opacity: 0.5,
            blend: BlendMode::Multiply,
            buffer: PixelBuffer::new(16, 16),
            parent: None,
            is_group: true,
            tilemap: None,
            locked: false,
            tilemap_cache: std::cell::RefCell::new(None),
        };
        let mut child_buf = PixelBuffer::new(16, 16);
        child_buf.set_pixel(1, 1, crate::core::color::Color::rgba(255, 10, 20, 255));
        let child = Layer {
            id: child_id,
            name: "Hero".to_owned(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            buffer: child_buf,
            parent: Some(group_id),
            is_group: false,
            tilemap: None,
            locked: true,
            tilemap_cache: std::cell::RefCell::new(None),
        };
        layers.insert_layer(group, 0);
        layers.insert_layer(child, 1);
        layers.remove_layer(initial_id);

        let bytes = export_aseprite_bytes(
            &layers,
            &TilePalette::new(),
            &AnimationSet::new(),
            None,
            TileSize::new(16, 16),
            true,
        )
        .unwrap();
        let parsed = parse_aseprite_bytes(&bytes).unwrap();
        assert_eq!(parsed.layers.len(), 2);
        assert!(parsed.layers[0].is_group);
        assert_eq!(parsed.layers[0].name, "Characters");
        assert!(!parsed.layers[0].visible);
        assert_eq!(parsed.layers[0].child_level, 0);
        assert!(!parsed.layers[1].is_group);
        assert_eq!(parsed.layers[1].name, "Hero");
        assert_eq!(parsed.layers[1].child_level, 1);
        assert!(parsed.layers[1].locked);

        let imp = build_aseprite_import(&parsed, "G", AsepriteLayoutMode::SingleRow).unwrap();
        assert_eq!(imp.layers.len(), 2);
        let re_group = imp.layers.iter().find(|l| l.name == "Characters").unwrap();
        let re_child = imp.layers.iter().find(|l| l.name == "Hero").unwrap();
        assert!(re_group.is_group);
        assert_eq!(re_child.parent, Some(re_group.id));
        assert!(re_child.locked);
        assert!(!imp.layers.active_layer().is_group);
        let px = re_child.buffer.get_pixel(1, 1).unwrap();
        assert_eq!((px.r, px.g, px.b, px.a), (255, 10, 20, 255));
    }

    #[test]
    fn export_heterogeneous_frame_sizes_does_not_bleed_neighbor_pixels() {
        let mut layers = LayerStack::new(48, 32);
        // Frame 0: 32x32 at (0,0), Frame 1: 16x16 at (0,0) with neighbor pixel at (20, 0).
        layers
            .active_layer_mut()
            .buffer
            .set_pixel(2, 2, crate::core::color::Color::rgba(10, 20, 30, 255));
        layers
            .active_layer_mut()
            .buffer
            .set_pixel(20, 0, crate::core::color::Color::rgba(255, 0, 0, 255));
        let mut anims = AnimationSet::new();
        {
            let s = anims.get_mut(0).unwrap();
            *s = AnimationSequence::new("Mix");
            s.push(Frame::new(Region::new(Rect2i::new(0, 0, 32, 32), "Big"), 100));
            s.push(Frame::new(Region::new(Rect2i::new(0, 0, 16, 16), "Small"), 100));
        }
        let bytes = export_aseprite_bytes(
            &layers,
            &TilePalette::new(),
            &anims,
            None,
            TileSize::new(16, 16),
            false,
        )
        .unwrap();
        let parsed = parse_aseprite_bytes(&bytes).unwrap();
        assert_eq!((parsed.frame_w, parsed.frame_h), (32, 32));
        // In frame 1 (16x16 region), pixel (20, 0) outside rect.w must NOT bleed in.
        let cel1 = &parsed.frames[1].cels[0];
        let off = 20 * 4;
        assert_eq!(&cel1.rgba[off..off + 4], &[0, 0, 0, 0]);
    }

    #[test]
    fn parse_indexed_ase_with_palette_after_cel_and_legacy_opacity_flag() {
        // Construct a minimal 8-bit indexed .ase where:
        // - header_flags = 0 (legacy file: layer opacity 0 means 255)
        // - Cel chunk (0x2005) appears BEFORE Palette chunk (0x2019)
        // - Background layer flag (8) keeps transparent_idx opaque.
        fn push_chunk(dst: &mut Vec<u8>, ctype: u16, payload: &[u8]) {
            let csize = (6 + payload.len()) as u32;
            dst.extend_from_slice(&csize.to_le_bytes());
            dst.extend_from_slice(&ctype.to_le_bytes());
            dst.extend_from_slice(payload);
        }

        let mut layer_pl = Vec::new();
        layer_pl.extend_from_slice(&(1u16 | 2u16 | 8u16).to_le_bytes()); // visible | editable | background
        layer_pl.extend_from_slice(&0u16.to_le_bytes()); // normal
        layer_pl.extend_from_slice(&0u16.to_le_bytes()); // child_level
        layer_pl.extend_from_slice(&0u32.to_le_bytes()); // default w/h
        layer_pl.extend_from_slice(&0u16.to_le_bytes()); // normal blend
        layer_pl.push(0); // legacy opacity=0 (must become 255 because header_flags=0)
        layer_pl.extend_from_slice(&[0u8; 3]);
        encode_ase_string(&mut layer_pl, "BG");

        let mut cel_pl = Vec::new();
        cel_pl.extend_from_slice(&0u16.to_le_bytes()); // layer 0
        cel_pl.extend_from_slice(&0i16.to_le_bytes());
        cel_pl.extend_from_slice(&0i16.to_le_bytes());
        cel_pl.push(255);
        cel_pl.extend_from_slice(&0u16.to_le_bytes()); // raw image
        cel_pl.extend_from_slice(&0i16.to_le_bytes());
        cel_pl.extend_from_slice(&[0u8; 5]);
        cel_pl.extend_from_slice(&2u16.to_le_bytes()); // w=2
        cel_pl.extend_from_slice(&1u16.to_le_bytes()); // h=1
        cel_pl.extend_from_slice(&[0u8, 1u8]); // pixel 0 = idx 0 (transparent_idx), pixel 1 = idx 1

        let pal = Palette::new("P", vec![[11, 22, 33, 255], [44, 55, 66, 255]]).unwrap();
        let pal_pl = encode_palette_chunk(&pal);

        let mut chunks = Vec::new();
        push_chunk(&mut chunks, 0x2004, &layer_pl);
        push_chunk(&mut chunks, 0x2005, &cel_pl);
        push_chunk(&mut chunks, 0x2019, &pal_pl);

        let mut frame = Vec::new();
        frame.extend_from_slice(&((16 + chunks.len()) as u32).to_le_bytes());
        frame.extend_from_slice(&0xF1FAu16.to_le_bytes());
        frame.extend_from_slice(&3u16.to_le_bytes());
        frame.extend_from_slice(&100u16.to_le_bytes());
        frame.extend_from_slice(&0u16.to_le_bytes());
        frame.extend_from_slice(&3u32.to_le_bytes());
        frame.extend_from_slice(&chunks);

        let mut file = Vec::with_capacity(128 + frame.len());
        file.extend_from_slice(&((128 + frame.len()) as u32).to_le_bytes());
        file.extend_from_slice(&0xA5E0u16.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes()); // 1 frame
        file.extend_from_slice(&2u16.to_le_bytes()); // w=2
        file.extend_from_slice(&1u16.to_le_bytes()); // h=1
        file.extend_from_slice(&8u16.to_le_bytes()); // 8-bit indexed
        file.extend_from_slice(&0u32.to_le_bytes()); // header_flags = 0 (opacity NOT valid)
        file.extend_from_slice(&100u16.to_le_bytes());
        file.extend_from_slice(&[0u8; 8]);
        file.push(0); // transparent_idx = 0
        file.resize(128, 0);
        file.extend_from_slice(&frame);

        let parsed = parse_aseprite_bytes(&file).unwrap();
        assert_eq!(parsed.layers[0].opacity, 255);
        assert!(parsed.layers[0].is_background);
        assert_eq!(
            &parsed.frames[0].cels[0].rgba,
            &vec![11, 22, 33, 255, 44, 55, 66, 255]
        );
    }

    #[test]
    fn split_by_tags_preserves_untagged_frames_and_ping_pong_direction() {
        let parsed = ParsedAseprite {
            frame_w: 8,
            frame_h: 8,
            layers: vec![test_layer_info("L1")],
            frames: (0..5)
                .map(|_| AsepriteFrame {
                    duration_ms: 100,
                    cels: Vec::new(),
                })
                .collect(),
            tags: vec![AsepriteTag {
                name: "Bounce".to_owned(),
                from_frame: 0,
                to_frame: 2,
                direction: 2, // Ping-Pong: 0, 1, 2, 1
                repeat: 0,
            }],
            palette: None,
        };
        let (cols, rows) = layout_grid(&parsed, AsepriteLayoutMode::SplitByTags);
        assert_eq!((cols, rows), (3, 2)); // Row 0: Bounce (3 frames), Row 1: Untagged (2 frames)
        let imp = build_aseprite_import(&parsed, "T", AsepriteLayoutMode::SplitByTags).unwrap();
        assert_eq!(imp.animations.len(), 2);
        let bounce = imp.animations.get(0).unwrap();
        assert_eq!(bounce.len(), 4); // 0, 1, 2, 1
        assert_eq!(bounce.frame(3).unwrap().region().rect(), Rect2i::new(8, 0, 8, 8));
        let untagged = imp.animations.get(1).unwrap();
        assert_eq!(untagged.name(), "Untagged");
        assert_eq!(untagged.len(), 2);
    }

    #[test]
    fn parse_rejects_oversized_palette_without_oom() {
        let mut pal_pl = Vec::new();
        pal_pl.extend_from_slice(&u32::MAX.to_le_bytes()); // malicious new_size
        pal_pl.extend_from_slice(&0u32.to_le_bytes());
        pal_pl.extend_from_slice(&0u32.to_le_bytes());
        pal_pl.extend_from_slice(&[0u8; 8]);

        let mut chunks = Vec::new();
        chunks.extend_from_slice(&((6 + pal_pl.len()) as u32).to_le_bytes());
        chunks.extend_from_slice(&0x2019u16.to_le_bytes());
        chunks.extend_from_slice(&pal_pl);

        let mut frame = Vec::new();
        frame.extend_from_slice(&((16 + chunks.len()) as u32).to_le_bytes());
        frame.extend_from_slice(&0xF1FAu16.to_le_bytes());
        frame.extend_from_slice(&1u16.to_le_bytes());
        frame.extend_from_slice(&100u16.to_le_bytes());
        frame.extend_from_slice(&0u16.to_le_bytes());
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&chunks);

        let mut file = Vec::with_capacity(128 + frame.len());
        file.extend_from_slice(&((128 + frame.len()) as u32).to_le_bytes());
        file.extend_from_slice(&0xA5E0u16.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&8u16.to_le_bytes());
        file.extend_from_slice(&8u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.resize(128, 0);
        file.extend_from_slice(&frame);

        assert!(parse_aseprite_bytes(&file).is_err());
    }
}
