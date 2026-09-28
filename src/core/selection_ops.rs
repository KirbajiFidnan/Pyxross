//! Wand / lasso / grid-region selection shape algorithms.
//!
//! Each function turns a gesture — a seed pixel, a freehand polygon, or a run of
//! grid cells — into a `(bounding box, row-major mask)` pair, the same shape
//! API [`crate::core::select::Selection::capture_mask`] consumes.  None of these
//! functions mutate the buffer: they read pixels and report which cells are
//! selected.
//!
// allow: SIZE_OK — shape algorithms plus the mandated inline test suite.

use crate::core::buffer::PixelBuffer;
use crate::core::clip::PixelClip;
use crate::core::color::Color;
use crate::core::math::Rect2i;

/// 4-connected neighbourhood offsets.
const NEIGHBORS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Whether two colors differ by at most `tolerance` on every channel, counting
/// alpha as a channel.
pub fn within_tolerance(a: Color, b: Color, tolerance: u8) -> bool {
    a.r.abs_diff(b.r) <= tolerance
        && a.g.abs_diff(b.g) <= tolerance
        && a.b.abs_diff(b.b) <= tolerance
        && a.a.abs_diff(b.a) <= tolerance
}

/// Whether a pixel belongs to an optional clip (absent means everywhere).
fn pixel_allowed(clip: Option<&PixelClip>, x: i32, y: i32) -> bool {
    match clip {
        None => true,
        Some(clip) => clip.contains(x, y),
    }
}

/// Compacts a full-canvas `selected` map into a row-major mask over `bbox`.
fn compact_mask(selected: &[bool], width: i32, bbox: Rect2i) -> Vec<bool> {
    let mut mask = vec![false; bbox.area().max(0) as usize];
    for y in bbox.y..bbox.bottom() {
        for x in bbox.x..bbox.right() {
            let src = (y as usize) * (width as usize) + x as usize;
            let dst = ((y - bbox.y) as usize) * (bbox.w as usize) + (x - bbox.x) as usize;
            mask[dst] = selected.get(src).copied().unwrap_or(false);
        }
    }
    mask
}

/// The region a magic-wand gesture selects.
///
/// Contiguous mode floods 4-connected from `seed` over pixels within
/// `tolerance` of the seed color; global mode matches every pixel of the
/// canvas.  `clip`, when present, bounds both the seed and the match.  Returns
/// the tight bounding box plus its row-major mask, or `None` when the seed is
/// out of bounds or excluded by the clip.  The buffer is never mutated.
pub fn magic_wand_shape(
    buf: &PixelBuffer,
    seed: (i32, i32),
    tolerance: u8,
    contiguous: bool,
    clip: Option<&PixelClip>,
) -> Option<(Rect2i, Vec<bool>)> {
    let w = buf.width() as i32;
    let h = buf.height() as i32;
    let (sx, sy) = seed;
    if sx < 0 || sy < 0 || sx >= w || sy >= h || !pixel_allowed(clip, sx, sy) {
        return None;
    }
    let seed_color = buf.get_pixel(sx as usize, sy as usize)?;
    let idx_of = |x: i32, y: i32| (y as usize) * (w as usize) + x as usize;

    let mut selected = vec![false; (w as usize) * (h as usize)];
    let mut min_x = sx;
    let mut min_y = sy;
    let mut max_x = sx;
    let mut max_y = sy;

    if contiguous {
        selected[idx_of(sx, sy)] = true;
        let mut stack = vec![(sx, sy)];
        while let Some((cx, cy)) = stack.pop() {
            for (dx, dy) in NEIGHBORS {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx < 0 || ny < 0 || nx >= w || ny >= h || !pixel_allowed(clip, nx, ny) {
                    continue;
                }
                let idx = idx_of(nx, ny);
                if selected[idx] {
                    continue;
                }
                let color = buf.get_pixel(nx as usize, ny as usize)?;
                if !within_tolerance(color, seed_color, tolerance) {
                    continue;
                }
                selected[idx] = true;
                stack.push((nx, ny));
                min_x = min_x.min(nx);
                min_y = min_y.min(ny);
                max_x = max_x.max(nx);
                max_y = max_y.max(ny);
            }
        }
    } else {
        for py in 0..h {
            for px in 0..w {
                if !pixel_allowed(clip, px, py) {
                    continue;
                }
                let color = buf.get_pixel(px as usize, py as usize)?;
                if !within_tolerance(color, seed_color, tolerance) {
                    continue;
                }
                selected[idx_of(px, py)] = true;
                min_x = min_x.min(px);
                min_y = min_y.min(py);
                max_x = max_x.max(px);
                max_y = max_y.max(py);
            }
        }
    }

    let bbox = Rect2i::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1);
    Some((bbox, compact_mask(&selected, w, bbox)))
}

/// The 4-connected component of transparent-adjacent pixels containing `seed`.
///
/// Selects every pixel reachable from `seed` through 4-connected neighbours
/// whose alpha is non-zero.  Returns `None` when the seed is out of bounds or
/// itself transparent.
pub fn alpha_neighbors_shape(buf: &PixelBuffer, seed: (i32, i32)) -> Option<(Rect2i, Vec<bool>)> {
    let w = buf.width() as i32;
    let h = buf.height() as i32;
    let (sx, sy) = seed;
    if sx < 0 || sy < 0 || sx >= w || sy >= h {
        return None;
    }
    if buf.get_pixel(sx as usize, sy as usize)?.a == 0 {
        return None;
    }
    let idx_of = |x: i32, y: i32| (y as usize) * (w as usize) + x as usize;

    let mut selected = vec![false; (w as usize) * (h as usize)];
    selected[idx_of(sx, sy)] = true;
    let mut min_x = sx;
    let mut min_y = sy;
    let mut max_x = sx;
    let mut max_y = sy;
    let mut stack = vec![(sx, sy)];
    while let Some((cx, cy)) = stack.pop() {
        for (dx, dy) in NEIGHBORS {
            let nx = cx + dx;
            let ny = cy + dy;
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            let idx = idx_of(nx, ny);
            if selected[idx] {
                continue;
            }
            if buf.get_pixel(nx as usize, ny as usize)?.a == 0 {
                continue;
            }
            selected[idx] = true;
            stack.push((nx, ny));
            min_x = min_x.min(nx);
            min_y = min_y.min(ny);
            max_x = max_x.max(nx);
            max_y = max_y.max(ny);
        }
    }
    let bbox = Rect2i::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1);
    Some((bbox, compact_mask(&selected, w, bbox)))
}

/// The region a closed lasso polygon selects.
///
/// The polygon is rasterized with the NONZERO winding rule, so self-overlap
/// unions and never punches holes.  A single point selects one pixel; a
/// two-point degenerate line selects its bounding box; anything longer is a
/// filled polygon clipped to the canvas.  Returns `None` for an empty point
/// list, an all-off-canvas shape, or a polygon that selects nothing.
pub fn lasso_shape(buf: &PixelBuffer, points: &[(i32, i32)]) -> Option<(Rect2i, Vec<bool>)> {
    if points.is_empty() {
        return None;
    }
    let canvas = Rect2i::new(0, 0, buf.width() as i32, buf.height() as i32);

    if points.len() == 1 {
        let (x, y) = points[0];
        let bbox = Rect2i::new(x, y, 1, 1).clamp_to(canvas);
        return (!bbox.is_empty()).then(|| (bbox, vec![true]));
    }
    if points.len() == 2 {
        let (x0, y0) = points[0];
        let (x1, y1) = points[1];
        let bbox = Rect2i::new(
            x0.min(x1),
            y0.min(y1),
            (x0 - x1).abs() + 1,
            (y0 - y1).abs() + 1,
        )
        .clamp_to(canvas);
        return (!bbox.is_empty()).then(|| (bbox, vec![true; bbox.area() as usize]));
    }

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for &(x, y) in points {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let scan = Rect2i::new(min_x, min_y, max_x - min_x + 1, max_y - min_y + 1).clamp_to(canvas);
    if scan.is_empty() {
        return None;
    }

    let width = buf.width() as i32;
    let mut selected = vec![false; (width as usize) * buf.height()];
    let mut tight: Option<Rect2i> = None;
    for y in scan.y..scan.bottom() {
        for x in scan.x..scan.right() {
            if !point_in_polygon(points, x as f64 + 0.5, y as f64 + 0.5) {
                continue;
            }
            selected[(y as usize) * (width as usize) + x as usize] = true;
            let pixel = Rect2i::new(x, y, 1, 1);
            tight = Some(match tight {
                None => pixel,
                Some(bbox) => bbox.union(pixel),
            });
        }
    }
    let bbox = tight?;
    Some((bbox, compact_mask(&selected, width, bbox)))
}

/// Nonzero-winding point-in-polygon test at a continuous position.
pub fn point_in_polygon(points: &[(i32, i32)], x: f64, y: f64) -> bool {
    let n = points.len();
    if n < 3 {
        return false;
    }
    let mut winding = 0i32;
    for i in 0..n {
        let (x1, y1) = points[i];
        let (x2, y2) = points[(i + 1) % n];
        let (x1, y1) = (x1 as f64, y1 as f64);
        let (x2, y2) = (x2 as f64, y2 as f64);
        let left = (x2 - x1) * (y - y1) - (x - x1) * (y2 - y1);
        if y1 <= y {
            if y2 > y && left > 0.0 {
                winding += 1;
            }
        } else if y2 <= y && left < 0.0 {
            winding -= 1;
        }
    }
    winding != 0
}

/// Absolute shoelace area of a polygon.
pub fn polygon_area(points: &[(i32, i32)]) -> f64 {
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let mut twice = 0i64;
    for i in 0..n {
        let (x1, y1) = points[i];
        let (x2, y2) = points[(i + 1) % n];
        twice += x1 as i64 * y2 as i64 - x2 as i64 * y1 as i64;
    }
    (twice as f64).abs() / 2.0
}

/// The grid cell rect containing `(x, y)` for a `tile`-sized grid.
fn cell_rect(cx: i32, cy: i32, tile: i32) -> Rect2i {
    Rect2i::new(cx.saturating_mul(tile), cy.saturating_mul(tile), tile, tile)
}

/// Every grid cell the segment `origin -> dest` passes through.
///
/// Cells are walked one at a time (Amanatides & Woo), deduplicated, and
/// returned in traversal order.  `tile_size` 0 is treated as 1.
pub fn tile_cells_on_segment(origin: (i32, i32), dest: (i32, i32), tile_size: u32) -> Vec<Rect2i> {
    let tile = tile_size.max(1).min(i32::MAX as u32) as i32;
    let (ox, oy) = origin;
    let (dx, dy) = dest;

    let mut cx = ox.div_euclid(tile);
    let mut cy = oy.div_euclid(tile);
    let ex = dx.div_euclid(tile);
    let ey = dy.div_euclid(tile);

    let vx = (dx - ox) as f64;
    let vy = (dy - oy) as f64;
    let step_x = if vx > 0.0 {
        1
    } else if vx < 0.0 {
        -1
    } else {
        0
    };
    let step_y = if vy > 0.0 {
        1
    } else if vy < 0.0 {
        -1
    } else {
        0
    };

    let next_x = if step_x > 0 {
        (cx + 1) as f64 * tile as f64
    } else {
        cx as f64 * tile as f64
    };
    let next_y = if step_y > 0 {
        (cy + 1) as f64 * tile as f64
    } else {
        cy as f64 * tile as f64
    };
    let mut t_max_x = if vx != 0.0 {
        (next_x - ox as f64) / vx
    } else {
        f64::INFINITY
    };
    let mut t_max_y = if vy != 0.0 {
        (next_y - oy as f64) / vy
    } else {
        f64::INFINITY
    };
    let t_delta_x = if vx != 0.0 {
        tile as f64 / vx.abs()
    } else {
        f64::INFINITY
    };
    let t_delta_y = if vy != 0.0 {
        tile as f64 / vy.abs()
    } else {
        f64::INFINITY
    };

    let mut cells: Vec<Rect2i> = Vec::new();
    loop {
        let rect = cell_rect(cx, cy, tile);
        if cells.last() != Some(&rect) {
            cells.push(rect);
        }
        if cx == ex && cy == ey {
            break;
        }
        if cx == ex {
            cy += step_y;
            t_max_y += t_delta_y;
        } else if cy == ey || t_max_x < t_max_y {
            cx += step_x;
            t_max_x += t_delta_x;
        } else if t_max_y < t_max_x {
            cy += step_y;
            t_max_y += t_delta_y;
        } else {
            cx += step_x;
            cy += step_y;
            t_max_x += t_delta_x;
            t_max_y += t_delta_y;
        }
    }
    cells
}

/// The union of a run of grid cells as a mask.
///
/// Each cell is clipped to the canvas; the result is the tight bounding box of
/// the clipped cells plus its row-major mask.  Returns `None` when `cells` is
/// empty or every cell lies off-canvas.
pub fn tile_region_shape(buf: &PixelBuffer, cells: &[Rect2i]) -> Option<(Rect2i, Vec<bool>)> {
    let canvas = Rect2i::new(0, 0, buf.width() as i32, buf.height() as i32);
    let mut clipped: Vec<Rect2i> = Vec::new();
    let mut bbox: Option<Rect2i> = None;
    for &cell in cells {
        let rect = cell.clamp_to(canvas);
        if rect.is_empty() {
            continue;
        }
        bbox = Some(match bbox {
            None => rect,
            Some(current) => current.union(rect),
        });
        clipped.push(rect);
    }
    let bbox = bbox?;

    let mut mask = vec![false; bbox.area().max(0) as usize];
    for rect in clipped {
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                let idx = ((y - bbox.y) as usize) * (bbox.w as usize) + (x - bbox.x) as usize;
                mask[idx] = true;
            }
        }
    }
    Some((bbox, mask))
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-pattern, mirroring `pattern_fill` in `select.rs`.
    fn pattern_fill(buf: &mut PixelBuffer) {
        let w = buf.width();
        let h = buf.height();
        for y in 0..h {
            for x in 0..w {
                let r = ((x * 17 + y * 31) & 0xFF) as u8;
                let g = ((x * 13 + y * 23) & 0xFF) as u8;
                let b = ((x * 11 + y * 37) & 0xFF) as u8;
                buf.set_pixel(x, y, Color::rgba(r, g, b, 255));
            }
        }
    }

    /// Reads a row-major mask over `bbox`.
    fn mask_contains(bbox: Rect2i, mask: &[bool], x: i32, y: i32) -> bool {
        if !bbox.contains(x, y) {
            return false;
        }
        mask[((y - bbox.y) as usize) * (bbox.w as usize) + (x - bbox.x) as usize]
    }

    #[test]
    fn within_tolerance_accepts_exact_and_rejects_beyond() {
        let a = Color::rgba(10, 20, 30, 40);
        assert!(within_tolerance(a, a, 0));
        assert!(within_tolerance(Color::rgba(11, 20, 30, 40), a, 1));
        assert!(!within_tolerance(Color::rgba(12, 20, 30, 40), a, 1));
        assert!(!within_tolerance(Color::rgba(10, 20, 30, 42), a, 1));
    }

    #[test]
    fn within_tolerance_treats_alpha_as_a_channel() {
        let a = Color::rgba(0, 0, 0, 10);
        assert!(within_tolerance(Color::rgba(0, 0, 0, 11), a, 1));
        assert!(!within_tolerance(Color::rgba(0, 0, 0, 12), a, 1));
        assert!(!within_tolerance(Color::rgba(0, 0, 0, 11), a, 0));
    }

    #[test]
    fn wand_contiguous_stops_at_color_boundary() {
        let a = Color::rgb(10, 10, 10);
        let b = Color::rgb(200, 200, 200);
        let mut buf = PixelBuffer::new(5, 1);
        for x in [0, 1, 3, 4] {
            buf.set_pixel(x, 0, a);
        }
        buf.set_pixel(2, 0, b);
        let epoch = buf.change_epoch();

        let (bbox, mask) = magic_wand_shape(&buf, (0, 0), 0, true, None).unwrap();

        assert_eq!(bbox, Rect2i::new(0, 0, 2, 1));
        assert_eq!(mask, vec![true, true]);
        assert_eq!(buf.change_epoch(), epoch, "wand must not mutate the buffer");
    }

    #[test]
    fn wand_global_reaches_disconnected_regions() {
        let a = Color::rgb(10, 10, 10);
        let b = Color::rgb(200, 200, 200);
        let mut buf = PixelBuffer::new(5, 1);
        for x in [0, 1, 3, 4] {
            buf.set_pixel(x, 0, a);
        }
        buf.set_pixel(2, 0, b);

        let (bbox, mask) = magic_wand_shape(&buf, (0, 0), 0, false, None).unwrap();

        assert_eq!(bbox, Rect2i::new(0, 0, 5, 1));
        assert_eq!(mask, vec![true, true, false, true, true]);
    }

    #[test]
    fn wand_tolerance_widens_the_match() {
        let mut buf = PixelBuffer::new(3, 1);
        buf.set_pixel(0, 0, Color::rgb(100, 100, 100));
        buf.set_pixel(1, 0, Color::rgb(102, 100, 100));
        buf.set_pixel(2, 0, Color::rgb(105, 100, 100));

        let (exact, _) = magic_wand_shape(&buf, (0, 0), 0, true, None).unwrap();
        assert_eq!(exact, Rect2i::new(0, 0, 1, 1));

        let (widened, mask) = magic_wand_shape(&buf, (0, 0), 2, true, None).unwrap();
        assert_eq!(widened, Rect2i::new(0, 0, 2, 1));
        assert_eq!(mask, vec![true, true]);
    }

    #[test]
    fn wand_seed_out_of_bounds_is_none() {
        let mut buf = PixelBuffer::new(4, 4);
        pattern_fill(&mut buf);
        for seed in [(-1, 0), (0, -1), (4, 0), (0, 4), (100, 100)] {
            assert!(
                magic_wand_shape(&buf, seed, 0, true, None).is_none(),
                "{seed:?} must be rejected"
            );
        }
        // A clip that excludes the seed also yields nothing.
        let clip = PixelClip::from_rect(Rect2i::new(2, 2, 1, 1));
        assert!(magic_wand_shape(&buf, (0, 0), 0, true, Some(&clip)).is_none());
    }

    #[test]
    fn wand_restrict_to_region_clamps_to_seed_cell() {
        let mut buf = PixelBuffer::new(5, 5);
        pattern_fill(&mut buf);
        let clip = PixelClip::from_rect(Rect2i::new(2, 2, 1, 1));

        let (bbox, mask) = magic_wand_shape(&buf, (2, 2), 0, true, Some(&clip)).unwrap();

        assert_eq!(bbox, Rect2i::new(2, 2, 1, 1));
        assert_eq!(mask, vec![true]);
    }

    #[test]
    fn alpha_neighbors_selects_opaque_component() {
        let opaque = Color::rgba(255, 255, 255, 255);
        let mut buf = PixelBuffer::new(3, 3);
        for (x, y) in [(0, 0), (1, 0), (0, 1), (2, 2)] {
            buf.set_pixel(x, y, opaque);
        }

        let (bbox, mask) = alpha_neighbors_shape(&buf, (0, 0)).unwrap();

        assert_eq!(bbox, Rect2i::new(0, 0, 2, 2));
        assert_eq!(mask, vec![true, true, true, false]);
    }

    #[test]
    fn alpha_neighbors_ignores_transparent() {
        let mut buf = PixelBuffer::new(3, 3);
        buf.set_pixel(1, 1, Color::rgba(255, 255, 255, 255));

        assert!(alpha_neighbors_shape(&buf, (0, 0)).is_none());
        assert!(alpha_neighbors_shape(&buf, (-1, 0)).is_none());
        assert!(alpha_neighbors_shape(&buf, (3, 0)).is_none());
    }

    #[test]
    fn lasso_triangle_selects_its_interior() {
        let buf = PixelBuffer::new(6, 6);
        let points = [(0, 0), (4, 0), (0, 4)];

        let (bbox, mask) = lasso_shape(&buf, &points).unwrap();

        assert!(mask_contains(bbox, &mask, 0, 0));
        assert!(mask_contains(bbox, &mask, 1, 1));
        assert!(mask_contains(bbox, &mask, 0, 2));
        assert!(!mask_contains(bbox, &mask, 3, 3));
        assert!(!mask_contains(bbox, &mask, 5, 5));
    }

    #[test]
    fn lasso_single_point_selects_one_pixel() {
        let buf = PixelBuffer::new(5, 5);

        let (bbox, mask) = lasso_shape(&buf, &[(2, 3)]).unwrap();
        assert_eq!(bbox, Rect2i::new(2, 3, 1, 1));
        assert_eq!(mask, vec![true]);

        assert!(lasso_shape(&buf, &[(9, 9)]).is_none());
        assert!(lasso_shape(&buf, &[]).is_none());
    }

    #[test]
    fn lasso_degenerate_line_selects_bounding_box() {
        let buf = PixelBuffer::new(6, 6);

        let (bbox, mask) = lasso_shape(&buf, &[(1, 1), (4, 3)]).unwrap();

        assert_eq!(bbox, Rect2i::new(1, 1, 4, 3));
        assert_eq!(mask, vec![true; 12]);
    }

    #[test]
    fn lasso_off_canvas_clips() {
        let buf = PixelBuffer::new(4, 4);

        let (bbox, mask) = lasso_shape(&buf, &[(2, 2), (10, 2), (10, 10), (2, 10)]).unwrap();

        assert_eq!(bbox, Rect2i::new(2, 2, 2, 2));
        assert_eq!(mask, vec![true; 4]);

        assert!(lasso_shape(&buf, &[(10, 10), (12, 10), (10, 12)]).is_none());
    }

    #[test]
    fn lasso_self_intersection_unions() {
        let buf = PixelBuffer::new(6, 6);
        // The same triangle traced twice: nonzero winding must union, not cancel.
        let points = [(0, 0), (4, 0), (0, 4), (0, 0), (4, 0), (0, 4)];

        let (bbox, mask) = lasso_shape(&buf, &points).unwrap();

        assert!(mask_contains(bbox, &mask, 0, 0));
        assert!(mask_contains(bbox, &mask, 1, 1));
        assert!(mask_contains(bbox, &mask, 0, 2));
        assert!(!mask_contains(bbox, &mask, 3, 3));
    }

    #[test]
    fn tile_cells_on_segment_covers_crossed_cells_only() {
        let cells = tile_cells_on_segment((5, 5), (25, 5), 10);
        assert_eq!(
            cells,
            vec![
                Rect2i::new(0, 0, 10, 10),
                Rect2i::new(10, 0, 10, 10),
                Rect2i::new(20, 0, 10, 10),
            ]
        );

        // tile_size 0 is treated as 1.
        let single = tile_cells_on_segment((0, 0), (3, 0), 0);
        assert_eq!(
            single,
            vec![
                Rect2i::new(0, 0, 1, 1),
                Rect2i::new(1, 0, 1, 1),
                Rect2i::new(2, 0, 1, 1),
                Rect2i::new(3, 0, 1, 1),
            ]
        );
    }

    #[test]
    fn tile_region_shape_selects_whole_cells() {
        let buf = PixelBuffer::new(10, 10);

        let (bbox, mask) =
            tile_region_shape(&buf, &[Rect2i::new(0, 0, 3, 3), Rect2i::new(6, 6, 3, 3)]).unwrap();
        assert_eq!(bbox, Rect2i::new(0, 0, 9, 9));
        assert!(mask_contains(bbox, &mask, 0, 0));
        assert!(mask_contains(bbox, &mask, 2, 2));
        assert!(mask_contains(bbox, &mask, 6, 6));
        assert!(mask_contains(bbox, &mask, 8, 8));
        assert!(!mask_contains(bbox, &mask, 3, 3));
        assert!(!mask_contains(bbox, &mask, 5, 5));

        // A cell straddling the canvas edge is clipped.
        let (bbox, mask) = tile_region_shape(&buf, &[Rect2i::new(-2, -2, 3, 3)]).unwrap();
        assert_eq!(bbox, Rect2i::new(0, 0, 1, 1));
        assert_eq!(mask, vec![true]);

        assert!(tile_region_shape(&buf, &[]).is_none());
        assert!(tile_region_shape(&buf, &[Rect2i::new(20, 20, 2, 2)]).is_none());
    }
}
