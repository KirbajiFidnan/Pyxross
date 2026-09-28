//! Pure PNG codec (D51): encode/decode 8-bit RGBA8 with no UI dependencies.
//!
//! Extracted from `crate::io` so D51's internal clipboard region + PNG
//! round-trip can live entirely in `src/core` without pulling in
//! `crate::input`/`egui`. `crate::io` re-exports these items to preserve its
//! existing public API.

use std::fmt;
use std::io::Cursor;

/// Errors produced by the PNG codec.
#[derive(Debug)]
pub enum PngError {
    Io(std::io::Error),
    Decode(String),
    Encode(String),
}

impl fmt::Display for PngError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PngError::Io(e) => write!(f, "png io error: {e}"),
            PngError::Decode(msg) => write!(f, "png decode error: {msg}"),
            PngError::Encode(msg) => write!(f, "png encode error: {msg}"),
        }
    }
}

impl std::error::Error for PngError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PngError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// Decoded 8-bit RGBA image, row-major, stride = `width * 4`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PngImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Encodes 8-bit RGBA pixels (row-major, stride `width * 4`) as PNG bytes.
///
/// Returns `Err` when `rgba.len() != width * height * 4` or the dimensions
/// do not fit the PNG format.
pub fn encode_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, PngError> {
    let expected = match width.checked_mul(height).and_then(|n| n.checked_mul(4)) {
        Some(n) => n,
        None => {
            return Err(PngError::Encode(format!(
                "dimensions {width}x{height} overflow the byte count"
            )));
        }
    };
    if rgba.len() != expected {
        return Err(PngError::Encode(format!(
            "rgba buffer length {} does not match width*height*4 = {expected}",
            rgba.len()
        )));
    }
    let w = u32::try_from(width)
        .map_err(|_| PngError::Encode(format!("width {width} does not fit the PNG format")))?;
    let h = u32::try_from(height)
        .map_err(|_| PngError::Encode(format!("height {height} does not fit the PNG format")))?;

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(Cursor::new(&mut out), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|e| PngError::Encode(e.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|e| PngError::Encode(e.to_string()))?;
        writer
            .finish()
            .map_err(|e| PngError::Encode(e.to_string()))?;
    }
    Ok(out)
}

/// Decodes any valid PNG (palette/indexed, grayscale, grayscale+alpha, RGB,
/// RGBA; 1/2/4/8/16-bit) into 8-bit RGBA pixels.
///
/// Malformed or truncated input yields `Err`, never a panic.
pub fn decode_png(data: &[u8]) -> Result<PngImage, PngError> {
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|e| PngError::Decode(e.to_string()))?;

    let buf_len = reader
        .output_buffer_size()
        .ok_or_else(|| PngError::Decode("output buffer size overflow".to_string()))?;
    let mut buf = vec![0u8; buf_len];
    let frame = reader
        .next_frame(&mut buf)
        .map_err(|e| PngError::Decode(e.to_string()))?;
    // Validate the stream is complete (IEND present); truncated input errors.
    reader
        .finish()
        .map_err(|e| PngError::Decode(e.to_string()))?;

    let (color_type, bit_depth) = reader.output_color_type();
    if bit_depth != png::BitDepth::Eight {
        return Err(PngError::Decode(format!(
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
            return Err(PngError::Decode(
                "indexed color type was not expanded".to_string(),
            ));
        }
    }
    Ok(PngImage {
        width,
        height,
        rgba,
    })
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

    fn assert_roundtrip(width: usize, height: usize, rgba: &[u8]) {
        let png = encode_png(width, height, rgba).unwrap();
        let img = decode_png(&png).unwrap();
        assert_eq!((img.width, img.height), (width, height));
        assert_eq!(img.rgba, rgba);
    }

    #[test]
    fn encode_decode_roundtrip_1x1() {
        assert_roundtrip(1, 1, &[255, 0, 0, 255]);
        assert_roundtrip(1, 1, &[0, 0, 0, 0]);
    }

    #[test]
    fn encode_decode_roundtrip_3x5() {
        assert_roundtrip(3, 5, &rgba_fixture(3, 5, 7));
    }

    #[test]
    fn encode_decode_roundtrip_64x16() {
        assert_roundtrip(64, 16, &rgba_fixture(64, 16, 13));
    }

    #[test]
    fn encode_decode_roundtrip_fully_transparent() {
        assert_roundtrip(8, 8, &vec![0u8; 8 * 8 * 4]);
    }

    #[test]
    fn encode_decode_roundtrip_fully_opaque() {
        assert_roundtrip(8, 8, &vec![200u8; 8 * 8 * 4]);
    }

    #[test]
    fn decode_palette_indexed_normalizes_to_rgba() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut png), 2, 2);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![255, 0, 0, 0, 255, 0, 0, 0, 255]);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 1, 2, 0]).unwrap();
            writer.finish().unwrap();
        }
        let img = decode_png(&png).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(
            img.rgba,
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 0, 0, 255,]
        );
    }

    #[test]
    fn decode_grayscale_normalizes_to_rgba() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut png), 2, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[10, 200]).unwrap();
            writer.finish().unwrap();
        }
        let img = decode_png(&png).unwrap();
        assert_eq!(img.rgba, vec![10, 10, 10, 255, 200, 200, 200, 255]);
    }

    #[test]
    fn decode_grayscale_alpha_normalizes_to_rgba() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut png), 1, 2);
            encoder.set_color(png::ColorType::GrayscaleAlpha);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[10, 128, 200, 0]).unwrap();
            writer.finish().unwrap();
        }
        let img = decode_png(&png).unwrap();
        assert_eq!(img.rgba, vec![10, 10, 10, 128, 200, 200, 200, 0]);
    }

    #[test]
    fn decode_16bit_normalizes_to_8bit() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut png), 1, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            let mut writer = encoder.write_header().unwrap();
            // Big-endian 16-bit samples: 0x1234 -> 0x12, 0x00FF -> 0x00.
            writer
                .write_image_data(&[
                    0x12, 0x34, 0xab, 0xcd, 0x00, 0xff, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff,
                    0xff, 0xff, 0xff,
                ])
                .unwrap();
            writer.finish().unwrap();
        }
        let img = decode_png(&png).unwrap();
        assert_eq!(
            img.rgba,
            vec![0x12, 0xab, 0x00, 0x80, 0x00, 0x00, 0xff, 0xff]
        );
    }

    #[test]
    fn decode_1bit_grayscale_normalizes_to_rgba() {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut png), 8, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::One);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0b1010_0101]).unwrap();
            writer.finish().unwrap();
        }
        let img = decode_png(&png).unwrap();
        assert_eq!(
            img.rgba,
            vec![
                255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255,
                255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ]
        );
    }

    #[test]
    fn decode_truncated_returns_err() {
        let png = encode_png(8, 8, &rgba_fixture(8, 8, 3)).unwrap();
        for cut in [0, 1, 8, 20, png.len() / 2, png.len() - 1] {
            assert!(decode_png(&png[..cut]).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn decode_corrupt_returns_err() {
        let mut png = encode_png(4, 4, &rgba_fixture(4, 4, 5)).unwrap();
        let mid = png.len() / 2;
        png[mid] ^= 0xff;
        assert!(decode_png(&png).is_err());
        assert!(decode_png(b"not a png at all").is_err());
    }

    #[test]
    fn decode_empty_returns_err() {
        assert!(decode_png(&[]).is_err());
    }

    #[test]
    fn encode_mismatched_length_returns_err() {
        assert!(encode_png(2, 2, &[0u8; 15]).is_err());
        assert!(encode_png(2, 2, &[0u8; 17]).is_err());
        assert!(encode_png(2, 2, &[]).is_err());
        assert!(encode_png(0, 0, &[0u8; 1]).is_err());
    }
}
