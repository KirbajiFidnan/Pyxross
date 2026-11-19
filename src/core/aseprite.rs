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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsepriteLayoutMode {
    RowLength(u32),
    SingleRow,
    SplitByTags,
}

#[derive(Clone, Debug)]
pub struct AsepriteLayerInfo {
    pub name: String,
    pub visible: bool,
    pub is_group: bool,
    pub opacity: u8,
    pub blend: BlendMode,
    pub child_level: u16,
}

#[derive(Clone, Debug)]
pub struct AsepriteTag {
    pub name: String,
    pub from_frame: usize,
    pub to_frame: usize,
}

#[derive(Clone, Debug)]
pub struct AsepriteCel {
    pub layer_index: usize,
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    pub opacity: u8,
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
    let _flags = cur.read_u32()?;
    let default_speed = cur.read_u16()?.max(1) as u32;
    cur.skip(8)?; // reserved
    let transparent_idx = cur.read_u8()?;
    cur.skip(3)?;
    let _num_colors = cur.read_u16()?;
    cur.skip(94)?; // pixel ratio, grid, reserved to 128

    let mut layers: Vec<AsepriteLayerInfo> = Vec::new();
    let mut frames: Vec<AsepriteFrame> = Vec::with_capacity(frame_count);
    let mut tags: Vec<AsepriteTag> = Vec::new();
    let mut pal_rgba: Vec<[u8; 4]> = vec![[0, 0, 0, 255]; 256];
    let mut has_palette = false;

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

        let mut cels: Vec<AsepriteCel> = Vec::new();
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
                    let opacity = cc.read_u8()?;
                    cc.skip(3)?;
                    let name = cc.read_string()?;
                    layers.push(AsepriteLayerInfo {
                        name,
                        visible: (l_flags & 1) != 0,
                        is_group: l_type == 1,
                        opacity,
                        blend: match blend_raw {
                            1 => BlendMode::Multiply,
                            2 => BlendMode::Screen,
                            16 => BlendMode::Add,
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
                    let _z_index = cc.read_i16()?;
                    cc.skip(5)?;
                    match cel_type {
                        0 => {
                            let cw = cc.read_u16()? as usize;
                            let ch = cc.read_u16()? as usize;
                            let raw = cc.read_bytes(cc.remaining())?;
                            let rgba = decode_cel_pixels(
                                raw,
                                cw,
                                ch,
                                color_depth,
                                transparent_idx,
                                &pal_rgba,
                            )?;
                            cels.push(AsepriteCel {
                                layer_index,
                                x,
                                y,
                                w: cw,
                                h: ch,
                                opacity,
                                rgba,
                            });
                        }
                        1 => {
                            // Linked cel
                            let linked_frame = cc.read_u16()? as usize;
                            if let Some(src_cel) = frames
                                .get(linked_frame)
                                .and_then(|f| f.cels.iter().find(|c| c.layer_index == layer_index))
                            {
                                let mut cloned = src_cel.clone();
                                cloned.opacity = opacity;
                                cels.push(cloned);
                            }
                        }
                        2 => {
                            let cw = cc.read_u16()? as usize;
                            let ch = cc.read_u16()? as usize;
                            let compressed = cc.read_bytes(cc.remaining())?;
                            let mut dec = ZlibDecoder::new(compressed);
                            let mut raw = Vec::new();
                            dec.read_to_end(&mut raw)
                                .map_err(|e| format!("Failed to decompress cel: {e}"))?;
                            let rgba = decode_cel_pixels(
                                &raw,
                                cw,
                                ch,
                                color_depth,
                                transparent_idx,
                                &pal_rgba,
                            )?;
                            cels.push(AsepriteCel {
                                layer_index,
                                x,
                                y,
                                w: cw,
                                h: ch,
                                opacity,
                                rgba,
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
                        cc.skip(13)?; // dir(1) + repeat(2) + reserved(6) + rgb(3) + extra(1)
                        let name = cc.read_string()?;
                        tags.push(AsepriteTag {
                            name,
                            from_frame,
                            to_frame,
                        });
                    }
                }
                0x2019 => {
                    // Palette chunk
                    let new_size = cc.read_u32()? as usize;
                    let first = cc.read_u32()? as usize;
                    let last = cc.read_u32()? as usize;
                    cc.skip(8)?;
                    if new_size > 0 {
                        pal_rgba.resize(new_size, [0, 0, 0, 255]);
                    }
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
                    has_palette = true;
                }
                0x0004 if !has_palette => {
                    // Old palette chunk
                    let packets = cc.read_u16()? as usize;
                    let mut idx = 0usize;
                    for _ in 0..packets {
                        if cc.remaining() < 2 {
                            break;
                        }
                        idx += cc.read_u8()? as usize;
                        let cnt_raw = cc.read_u8()? as usize;
                        let cnt = if cnt_raw == 0 { 256 } else { cnt_raw };
                        for _ in 0..cnt {
                            if cc.remaining() < 3 {
                                break;
                            }
                            let r = cc.read_u8()?;
                            let g = cc.read_u8()?;
                            let b = cc.read_u8()?;
                            if idx < pal_rgba.len() {
                                pal_rgba[idx] = [r, g, b, 255];
                            }
                            idx += 1;
                        }
                    }
                    has_palette = true;
                }
                _ => {}
            }
        }

        frames.push(AsepriteFrame { duration_ms, cels });
        let next_frame = frame_start.saturating_add(frame_bytes);
        if next_frame > cur.pos && next_frame <= bytes.len() {
            cur.pos = next_frame;
        }
    }

    if layers.is_empty() {
        layers.push(AsepriteLayerInfo {
            name: "Layer 1".to_owned(),
            visible: true,
            is_group: false,
            opacity: 255,
            blend: BlendMode::Normal,
            child_level: 0,
        });
    }

    let palette = if has_palette {
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
    pal: &[[u8; 4]],
) -> Result<Vec<u8>, String> {
    let px_count = w.saturating_mul(h);
    let mut out = vec![0u8; px_count * 4];
    match depth {
        32 => {
            let need = px_count * 4;
            let copy = raw.len().min(need);
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
                if idx == transparent_idx {
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
                let max_tag = parsed
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
                (max_tag, parsed.tags.len())
            }
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
    let width = cols.saturating_mul(fw).max(1);
    let height = rows.saturating_mul(fh).max(1);

    let mut layer_stack = LayerStack::new(width, height);
    let initial_id = layer_stack.active_layer_id();

    // Build Pyxross layers matching Aseprite's bottom-to-top layer order.
    let mut layer_buffers: Vec<Vec<u8>> = vec![vec![0u8; width * height * 4]; parsed.layers.len()];

    let mut animations = AnimationSet::new();
    let mut first_anim = true;

    let mut blit_frame_at = |frame_idx: usize, gx: usize, gy: usize| {
        let Some(frame) = parsed.frames.get(frame_idx) else {
            return;
        };
        let dst_ox = gx * fw;
        let dst_oy = gy * fh;
        for cel in &frame.cels {
            let Some(buf) = layer_buffers.get_mut(cel.layer_index) else {
                continue;
            };
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
                                ((px_rgba[3] as u16 * cel.opacity as u16) / 255) as u8;
                        }
                        buf[dst_off..dst_off + 4].copy_from_slice(&px_rgba);
                    }
                }
            }
        }
    };

    match mode {
        AsepriteLayoutMode::SplitByTags if !parsed.tags.is_empty() => {
            for (row_idx, tag) in parsed.tags.iter().enumerate() {
                let to = tag.to_frame.min(parsed.frames.len() - 1);
                let from = tag.from_frame.min(to);
                let mut seq = AnimationSequence::new(tag.name.clone());
                for (col_idx, frame_idx) in (from..=to).enumerate() {
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
                if let Some(slot) = animations.get_mut(0) {
                    *slot = seq;
                }
            } else {
                for tag in &parsed.tags {
                    let to = tag.to_frame.min(parsed.frames.len() - 1);
                    let from = tag.from_frame.min(to);
                    let mut seq = AnimationSequence::new(tag.name.clone());
                    for frame_idx in from..=to {
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
                buffer.as_bytes_mut().copy_from_slice(raw);
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
            locked: false,
            tilemap_cache: std::cell::RefCell::new(None),
        };
        layer_stack.insert_layer(layer, idx);
        if info.is_group {
            parent_stack.push((info.child_level, id));
        }
    }
    layer_stack.remove_layer(initial_id);

    let empty_tiles = TilePalette::new();
    let preview_rgba = layer_stack.composite_layers(&empty_tiles).as_bytes().to_vec();

    Ok(AsepriteImportResult {
        name: name.to_owned(),
        width,
        height,
        frame_w: parsed.frame_w,
        frame_h: parsed.frame_h,
        layers: layer_stack,
        animations,
        palette: parsed.palette.clone(),
        preview_rgba,
    })
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
    let mut timeline: Vec<(Rect2i, u32)> = Vec::new();
    let mut tags: Vec<AsepriteTag> = Vec::new();

    if unified_frames {
        timeline.push((Rect2i::new(0, 0, cw as i32, ch as i32), 100));
    } else {
        // Flatten animations into sequential timeline frames + tags.
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
                });
            }
        }
        // If the project has no animation frames at all, lay out every region
        // cell from top-left to bottom-right as sequential timeline frames.
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
    }

    let frame_w = timeline[0].0.w.max(1) as u16;
    let frame_h = timeline[0].0.h.max(1) as u16;
    let export_layers: Vec<&Layer> = layers.iter().filter(|l| !l.is_group).collect();
    if export_layers.is_empty() {
        return Err("No drawable layers to export".to_owned());
    }

    // Pre-rasterize each layer (including tilemap if any).
    let mut rasterized_layers: Vec<(&Layer, Vec<u8>)> = Vec::with_capacity(export_layers.len());
    for l in export_layers {
        let mut buf = l.buffer.as_bytes().to_vec();
        if let Some(tm) = &l.tilemap {
            let mut tm_buf = PixelBuffer::new(cw, ch);
            tm.rasterize(tile_palette, &mut tm_buf);
            let tm_rgba = tm_buf.as_bytes();
            for p in (0..buf.len()).step_by(4) {
                if p + 3 < buf.len() && buf[p + 3] == 0 && tm_rgba[p + 3] > 0 {
                    buf[p..p + 4].copy_from_slice(&tm_rgba[p..p + 4]);
                }
            }
        }
        rasterized_layers.push((l, buf));
    }

    let mut body = Vec::new();

    for (frame_idx, (rect, delay_ms)) in timeline.iter().enumerate() {
        let mut chunks: Vec<(u16, Vec<u8>)> = Vec::new();
        if frame_idx == 0 {
            for (l, _) in &rasterized_layers {
                chunks.push((0x2004, encode_layer_chunk(l)));
            }
            if let Some(pal) = palette {
                if !pal.colors.is_empty() {
                    chunks.push((0x2019, encode_palette_chunk(pal)));
                }
            }
            if !tags.is_empty() {
                chunks.push((0x2018, encode_tags_chunk(&tags)));
            }
        }
        for (layer_idx, (_, buf)) in rasterized_layers.iter().enumerate() {
            let cel_rgba = crop_rgba(buf, cw, ch, *rect, frame_w as usize, frame_h as usize);
            chunks.push((
                0x2005,
                encode_cel_chunk(layer_idx as u16, frame_w, frame_h, &cel_rgba)?,
            ));
        }

        let mut frame_buf = Vec::new();
        let count_u32 = chunks.len() as u32;
        let old_count = count_u32.min(0xFFFF) as u16;
        let mut chunk_stream = Vec::new();
        for (ctype, payload) in chunks {
            let csize = (6 + payload.len()) as u32;
            chunk_stream.extend_from_slice(&csize.to_le_bytes());
            chunk_stream.extend_from_slice(&ctype.to_le_bytes());
            chunk_stream.extend_from_slice(&payload);
        }
        let total_frame_bytes = (16 + chunk_stream.len()) as u32;
        frame_buf.extend_from_slice(&total_frame_bytes.to_le_bytes());
        frame_buf.extend_from_slice(&0xF1FAu16.to_le_bytes());
        frame_buf.extend_from_slice(&old_count.to_le_bytes());
        frame_buf.extend_from_slice(&((*delay_ms).min(u16::MAX as u32) as u16).to_le_bytes());
        frame_buf.extend_from_slice(&0u16.to_le_bytes());
        frame_buf.extend_from_slice(&count_u32.to_le_bytes());
        frame_buf.extend_from_slice(&chunk_stream);
        body.extend_from_slice(&frame_buf);
    }

    let file_size = (128 + body.len()) as u32;
    let mut header = Vec::with_capacity(128);
    header.extend_from_slice(&file_size.to_le_bytes());
    header.extend_from_slice(&0xA5E0u16.to_le_bytes());
    header.extend_from_slice(&(timeline.len() as u16).to_le_bytes());
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

fn encode_layer_chunk(l: &Layer) -> Vec<u8> {
    let mut out = Vec::new();
    let flags: u16 = if l.visible { 1 } else { 0 };
    let ltype: u16 = if l.is_group { 1 } else { 0 };
    let blend: u16 = match l.blend {
        BlendMode::Normal => 0,
        BlendMode::Multiply => 1,
        BlendMode::Screen => 2,
        BlendMode::Add => 16,
    };
    let opacity = (l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&ltype.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // child_level
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

fn encode_tags_chunk(tags: &[AsepriteTag]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0u8; 8]);
    for t in tags {
        out.extend_from_slice(&(t.from_frame as u16).to_le_bytes());
        out.extend_from_slice(&(t.to_frame as u16).to_le_bytes());
        out.push(0); // forward
        out.extend_from_slice(&0u16.to_le_bytes()); // repeat
        out.extend_from_slice(&[0u8; 6]);
        out.extend_from_slice(&[255, 255, 255]); // tag color
        out.push(0);
        encode_ase_string(&mut out, &t.name);
    }
    out
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
    for y in 0..dst_h {
        let sy = rect.y + y as i32;
        if sy < 0 || sy >= sh as i32 {
            continue;
        }
        for x in 0..dst_w {
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

    #[test]
    fn split_by_tags_matches_user_example_24_frames_8_12_4() {
        let parsed = ParsedAseprite {
            frame_w: 16,
            frame_h: 16,
            layers: vec![AsepriteLayerInfo {
                name: "Layer 1".to_owned(),
                visible: true,
                is_group: false,
                opacity: 255,
                blend: BlendMode::Normal,
                child_level: 0,
            }],
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
                },
                AsepriteTag {
                    name: "Tag 2".to_owned(),
                    from_frame: 8,
                    to_frame: 19,
                },
                AsepriteTag {
                    name: "Tag 3".to_owned(),
                    from_frame: 20,
                    to_frame: 23,
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
        let mut layers = LayerStack::new(32, 16);
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
}
