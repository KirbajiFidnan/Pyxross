//! Pure grid math for Import GIF / Import Image (P1).
//!
//! All functions are deterministic conversions between image sizes, region
//! grids, and canvas sizes — no state, no I/O, no UI imports (core purity).
//! "Region" here is the import-grid cell: the pixel-art region size chosen by
//! the user (GIF frame size, or the split pixel size for images).
//!
//! The conversions are STATELESS projections: re-deriving a region size from a
//! derived grid does not restore the original region size (ceil semantics are
//! not invertible). The UI keeps the user's chosen region size separately and
//! only calls these helpers for the derived quantities.

use crate::core::math::Rect2i;

/// `a` rounded up over `b`. Guards: `a == 0 -> 0`, `b == 0 -> 0`.
pub fn ceil_div(a: u32, b: u32) -> u32 {
    if a == 0 || b == 0 {
        return 0;
    }
    a.div_ceil(b)
}

/// Regions needed to cover an `img_w × img_h` image with `px_w × px_h` cells
/// (each axis separately): e.g. 64×64 with 16×16 → (4, 4); 64×64 with 20×20 →
/// (4, 4).
pub fn regions_from_pixels(img_w: u32, img_h: u32, px_w: u32, px_h: u32) -> (u32, u32) {
    (ceil_div(img_w, px_w), ceil_div(img_h, px_h))
}

/// Cell size derived from splitting an `img_w × img_h` image into a
/// `regions_w × regions_h` grid (each axis separately): e.g. 64×64 into 3×3 →
/// (22, 22).
pub fn pixels_from_regions(img_w: u32, img_h: u32, regions_w: u32, regions_h: u32) -> (u32, u32) {
    (ceil_div(img_w, regions_w), ceil_div(img_h, regions_h))
}

/// Canvas size covering a `regions_w × regions_h` grid of `px_w × px_h` cells
/// (each axis separately).
pub fn canvas_from_grid(regions_w: u32, regions_h: u32, px_w: u32, px_h: u32) -> (u32, u32) {
    (
        regions_w.saturating_mul(px_w),
        regions_h.saturating_mul(px_h),
    )
}

/// Layout of a GIF animation's frames on an import region sheet.
#[derive(Debug, PartialEq, Eq)]
pub struct GifSheet {
    /// Total cells on the sheet (frame count rounded up to a full line).
    pub cells: usize,
    /// Sheet rows (`cells / line_length`).
    pub rows: usize,
    /// Sheet canvas width (`line_length × frame_w`).
    pub canvas_w: u32,
    /// Sheet canvas height (`rows × frame_h`).
    pub canvas_h: u32,
}

/// Arranges `frame_count` GIF frames into a `line_length`-wide region sheet.
///
/// `cells` rounds `frame_count` UP to the nearest multiple of `line_length`
/// (equal to `frame_count` when already divisible): 22 frames / line 4 →
/// cells 24, rows 6; 20 / 4 → cells 20, rows 5; line 1 → cells 22, rows 22
/// (a vertical strip). `None` when `frame_count == 0` or `line_length == 0`.
pub fn gif_sheet(
    frame_count: usize,
    line_length: u32,
    frame_w: u32,
    frame_h: u32,
) -> Option<GifSheet> {
    if frame_count == 0 || line_length == 0 {
        return None;
    }
    let rows = (frame_count + line_length as usize - 1) / line_length as usize;
    let cells = rows * line_length as usize;
    Some(GifSheet {
        cells,
        rows,
        canvas_w: frame_w.saturating_mul(line_length),
        canvas_h: frame_h.saturating_mul(rows as u32),
    })
}

/// Row-major region rectangles covering a `regions_w × regions_h` grid of
/// `px_w × px_h` cells — left to right, top to bottom: for cell index `idx`,
/// `r = idx / regions_w`, `c = idx % regions_w`, rect
/// `(c * px_w, r * px_h, px_w, px_h)`.
pub fn region_rects(regions_w: u32, regions_h: u32, px_w: u32, px_h: u32) -> Vec<Rect2i> {
    let mut rects = Vec::with_capacity((regions_w as usize) * (regions_h as usize));
    for r in 0..regions_h {
        for c in 0..regions_w {
            rects.push(Rect2i::new(
                (c * px_w) as i32,
                (r * px_h) as i32,
                px_w as i32,
                px_h as i32,
            ));
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_div_guards_and_plain_division() {
        assert_eq!(ceil_div(16, 0), 0, "zero divisor guards to 0");
        assert_eq!(ceil_div(0, 4), 0, "zero dividend guards to 0");
        assert_eq!(ceil_div(4, 2), 2, "exact division");
        assert_eq!(ceil_div(5, 2), 3, "rounding up");
        assert_eq!(ceil_div(0, 0), 0, "both zero guards to 0");
    }

    #[test]
    fn image_pixel_exact_grid_is_unchanged() {
        // 64x64 image, 16x16 regions: a perfectly divisible grid keeps the
        // canvas size.
        assert_eq!(regions_from_pixels(64, 64, 16, 16), (4, 4));
        assert_eq!(canvas_from_grid(4, 4, 16, 16), (64, 64));
    }

    #[test]
    fn pixel_driven_split_grows_the_canvas() {
        // 64x64 image, 20x20 regions: 4x4 regions of 20x20 -> 80x80 canvas
        // (pixel-driven enlargement).
        assert_eq!(regions_from_pixels(64, 64, 20, 20), (4, 4));
        assert_eq!(canvas_from_grid(4, 4, 20, 20), (80, 80));
    }

    #[test]
    fn region_driven_split_derives_pixel_size_and_grows_canvas() {
        // 64x64 image split into a 3x3 grid: 22x22 cells -> 66x66 canvas
        // (region-driven enlargement).
        assert_eq!(pixels_from_regions(64, 64, 3, 3), (22, 22));
        assert_eq!(canvas_from_grid(3, 3, 22, 22), (66, 66));
    }

    #[test]
    fn ceil_round_trips_are_stateless_projections() {
        // pixels -> regions -> pixels does NOT restore the original region
        // size: 64x64 @ 20x20 -> 4x4 regions -> re-derived 16x16 (NOT 20x20).
        // These helpers keep no state; the UI owns the chosen region size.
        assert_eq!(regions_from_pixels(64, 64, 20, 20), (4, 4));
        assert_eq!(pixels_from_regions(64, 64, 4, 4), (16, 16));

        // The region side IS stable: imploding the image with a derived cell
        // size reproduces the same grid.
        assert_eq!(pixels_from_regions(64, 64, 3, 3), (22, 22));
        assert_eq!(regions_from_pixels(64, 64, 22, 22), (3, 3));
    }

    #[test]
    fn gif_sheet_rounds_up_to_full_lines() {
        let sheet = gif_sheet(22, 4, 8, 16).unwrap();
        assert_eq!((sheet.cells, sheet.rows), (24, 6));
        assert_eq!((sheet.canvas_w, sheet.canvas_h), (32, 96));

        let exact = gif_sheet(20, 4, 8, 16).unwrap();
        assert_eq!((exact.cells, exact.rows), (20, 5));

        let strip = gif_sheet(22, 1, 8, 16).unwrap();
        assert_eq!((strip.cells, strip.rows), (22, 22));
        assert_eq!((strip.canvas_w, strip.canvas_h), (8, 352));
    }

    #[test]
    fn gif_sheet_zero_frame_count_or_line_length_is_none() {
        assert_eq!(gif_sheet(0, 4, 8, 8), None, "no frames -> no sheet");
        assert_eq!(gif_sheet(22, 0, 8, 8), None, "zero line length -> no sheet");
        assert_eq!(gif_sheet(0, 0, 8, 8), None);
    }

    #[test]
    fn region_rects_are_row_major_and_ordered() {
        let rects = region_rects(2, 2, 16, 16);
        assert_eq!(
            rects,
            vec![
                Rect2i::new(0, 0, 16, 16),
                Rect2i::new(16, 0, 16, 16),
                Rect2i::new(0, 16, 16, 16),
                Rect2i::new(16, 16, 16, 16),
            ]
        );

        let single = region_rects(1, 1, 8, 8);
        assert_eq!(single, vec![Rect2i::new(0, 0, 8, 8)]);
    }
}