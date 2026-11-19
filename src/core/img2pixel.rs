//! Image-to-Pixel conversion pipeline (inspired by SLK_img2pixel, CC0-1.0).
//!
//! Pure core logic: pre-processing (brightness, contrast, saturation, gamma,
//! hue, blur, sharpen), offset-aware downsampling, optional K-means palette
//! extraction, ordered/error-diffusion dithering, and alpha thresholding.

/// Downsampling filter used when reducing the source image to pixel-art size.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SampleMode {
    /// Crisp nearest-neighbor sampling with configurable sub-cell offset.
    Point,
    /// Box / area averaging over each target pixel cell (good for noisy photos).
    #[default]
    Average,
    /// Smooth bilinear interpolation at the offset-adjusted sample center.
    Bilinear,
}

impl SampleMode {
    pub const ALL: [Self; 3] = [Self::Point, Self::Average, Self::Bilinear];

    pub fn label(self) -> &'static str {
        match self {
            Self::Point => "Point",
            Self::Average => "Average",
            Self::Bilinear => "Bilinear",
        }
    }
}

/// How output colors are quantized.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PaletteMode {
    /// Keep full 32-bit RGBA colors after adjustments & downsampling.
    Original,
    /// Quantize to the currently active project palette.
    ProjectPalette,
    /// Extract `kmeans_colors` representative colors using K-means clustering.
    #[default]
    ExtractKMeans,
}

impl PaletteMode {
    pub const ALL: [Self; 3] = [Self::Original, Self::ExtractKMeans, Self::ProjectPalette];

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::ProjectPalette => "Project Palette",
            Self::ExtractKMeans => "K-Means",
        }
    }
}

/// Color distance metric used for palette matching and K-means clustering.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorDistanceMode {
    RgbEuclidean,
    #[default]
    PerceptualRedmean,
}

impl ColorDistanceMode {
    pub const ALL: [Self; 2] = [Self::PerceptualRedmean, Self::RgbEuclidean];

    pub fn label(self) -> &'static str {
        match self {
            Self::RgbEuclidean => "RGB Euclidean",
            Self::PerceptualRedmean => "Perceptual",
        }
    }
}

/// Dithering algorithm applied during palette quantization.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DitherMode {
    None,
    Bayer2x2,
    #[default]
    Bayer4x4,
    Bayer8x8,
    Cluster4x4,
    FloydSteinberg,
}

impl DitherMode {
    pub const ALL: [Self; 6] = [
        Self::None,
        Self::Bayer2x2,
        Self::Bayer4x4,
        Self::Bayer8x8,
        Self::Cluster4x4,
        Self::FloydSteinberg,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Bayer2x2 => "Bayer 2x2",
            Self::Bayer4x4 => "Bayer 4x4",
            Self::Bayer8x8 => "Bayer 8x8",
            Self::Cluster4x4 => "Cluster 4x4",
            Self::FloydSteinberg => "Floyd-Steinberg",
        }
    }
}

/// Tunable parameters for [`process_image_to_pixel`].
#[derive(Clone, Debug, PartialEq)]
pub struct Img2PixelParams {
    pub target_width: u32,
    pub target_height: u32,
    pub lock_aspect: bool,
    pub offset_x: f32,
    pub offset_y: f32,
    pub sample_mode: SampleMode,
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub gamma: f32,
    pub hue_shift: f32,
    pub blur: f32,
    pub sharpen: f32,
    pub palette_mode: PaletteMode,
    pub kmeans_colors: usize,
    pub color_distance: ColorDistanceMode,
    pub dither_mode: DitherMode,
    pub dither_amount: f32,
    pub alpha_threshold: u8,
}

impl Default for Img2PixelParams {
    fn default() -> Self {
        Self {
            target_width: 64,
            target_height: 64,
            lock_aspect: true,
            offset_x: 0.5,
            offset_y: 0.5,
            sample_mode: SampleMode::Average,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            gamma: 1.0,
            hue_shift: 0.0,
            blur: 0.0,
            sharpen: 0.0,
            palette_mode: PaletteMode::ExtractKMeans,
            kmeans_colors: 16,
            color_distance: ColorDistanceMode::PerceptualRedmean,
            dither_mode: DitherMode::Bayer4x4,
            dither_amount: 0.35,
            alpha_threshold: 128,
        }
    }
}

impl Img2PixelParams {
    /// Returns defaults sized proportionally to `src_w × src_h` (clamped to
    /// `<= 64` on the longer edge while keeping at least `1`).
    pub fn for_source(src_w: u32, src_h: u32) -> Self {
        let mut params = Self::default();
        let max_side = src_w.max(src_h).max(1);
        if max_side <= 128 {
            params.target_width = src_w.max(1);
            params.target_height = src_h.max(1);
        } else {
            let scale = 64.0 / max_side as f32;
            params.target_width = ((src_w as f32 * scale).round() as u32).max(1);
            params.target_height = ((src_h as f32 * scale).round() as u32).max(1);
        }
        params
    }
}

/// Result of [`process_image_to_pixel`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Img2PixelOutput {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub extracted_palette: Option<Vec<[u8; 4]>>,
}

/// Runs the full Image-to-Pixel conversion pipeline on `src_rgba`.
pub fn process_image_to_pixel(
    src_w: u32,
    src_h: u32,
    src_rgba: &[u8],
    project_palette: &[[u8; 4]],
    params: &Img2PixelParams,
) -> Img2PixelOutput {
    let sw = src_w.max(1) as usize;
    let sh = src_h.max(1) as usize;
    let tw = params.target_width.clamp(1, 2048) as usize;
    let th = params.target_height.clamp(1, 2048) as usize;
    if src_rgba.len() < sw * sh * 4 {
        return Img2PixelOutput {
            width: tw as u32,
            height: th as u32,
            rgba: vec![0; tw * th * 4],
            extracted_palette: None,
        };
    }

    // 1. Downsample directly from source (zero full-resolution copy), then
    // apply blur/sharpen on the target pixel grid so slider drags stay instant
    // even for large source images.
    let mut pixels = downsample_rgba(
        &src_rgba[..sw * sh * 4],
        sw,
        sh,
        tw,
        th,
        params.sample_mode,
        params.offset_x.clamp(0.0, 1.0),
        params.offset_y.clamp(0.0, 1.0),
    );

    if params.blur > 0.01 {
        pixels = blur_rgba_f32(&pixels, tw, th, params.blur.clamp(0.0, 4.0));
    }

    if params.sharpen > 0.01 {
        pixels = sharpen_rgba(&pixels, tw, th, params.sharpen.clamp(0.0, 2.0));
    }

    // 2. Tone & color adjustments + alpha threshold.
    apply_adjustments(&mut pixels, params);

    // 3. Palette resolution & quantization + dithering.
    let (palette, extracted_palette) = match params.palette_mode {
        PaletteMode::Original => (None, None),
        PaletteMode::ProjectPalette => {
            let pal: Vec<[u8; 4]> = project_palette
                .iter()
                .copied()
                .filter(|c| c[3] > 0)
                .collect();
            if pal.is_empty() {
                (None, None)
            } else {
                (Some(pal), None)
            }
        }
        PaletteMode::ExtractKMeans => {
            let k = params.kmeans_colors.clamp(2, 64);
            let extracted = extract_kmeans_palette(&pixels, k, params.color_distance);
            if extracted.is_empty() {
                (None, None)
            } else {
                (Some(extracted.clone()), Some(extracted))
            }
        }
    };

    if let Some(pal) = &palette {
        quantize_and_dither(
            &mut pixels,
            tw,
            th,
            pal,
            params.color_distance,
            params.dither_mode,
            params.dither_amount.clamp(0.0, 1.0),
        );
    }

    let mut out_rgba = vec![0u8; tw * th * 4];
    for (i, px) in pixels.iter().enumerate() {
        let a = if px[3] >= params.alpha_threshold as f32 {
            255
        } else {
            0
        };
        let o = i * 4;
        if a == 0 {
            out_rgba[o..o + 4].copy_from_slice(&[0, 0, 0, 0]);
        } else {
            out_rgba[o] = px[0].clamp(0.0, 255.0).round() as u8;
            out_rgba[o + 1] = px[1].clamp(0.0, 255.0).round() as u8;
            out_rgba[o + 2] = px[2].clamp(0.0, 255.0).round() as u8;
            out_rgba[o + 3] = 255;
        }
    }

    Img2PixelOutput {
        width: tw as u32,
        height: th as u32,
        rgba: out_rgba,
        extracted_palette,
    }
}

fn box_blur_f32(pixels: &[[f32; 4]], w: usize, h: usize, r: isize) -> Vec<[f32; 4]> {
    if r <= 0 || w == 0 || h == 0 {
        return pixels.to_vec();
    }
    let mut tmp = vec![[0.0f32; 4]; w * h];
    let mut dst = vec![[0.0f32; 4]; w * h];

    // Horizontal pass.
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            let mut count = 0.0f32;
            let x0 = (x as isize - r).max(0) as usize;
            let x1 = (x as isize + r).min(w as isize - 1) as usize;
            for sx in x0..=x1 {
                let p = pixels[y * w + sx];
                acc[0] += p[0];
                acc[1] += p[1];
                acc[2] += p[2];
                acc[3] += p[3];
                count += 1.0;
            }
            tmp[y * w + x] = [
                acc[0] / count,
                acc[1] / count,
                acc[2] / count,
                acc[3] / count,
            ];
        }
    }

    // Vertical pass.
    for y in 0..h {
        let y0 = (y as isize - r).max(0) as usize;
        let y1 = (y as isize + r).min(h as isize - 1) as usize;
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            let mut count = 0.0f32;
            for sy in y0..=y1 {
                let p = tmp[sy * w + x];
                acc[0] += p[0];
                acc[1] += p[1];
                acc[2] += p[2];
                acc[3] += p[3];
                count += 1.0;
            }
            dst[y * w + x] = [
                acc[0] / count,
                acc[1] / count,
                acc[2] / count,
                acc[3] / count,
            ];
        }
    }
    dst
}

fn blur_rgba_f32(pixels: &[[f32; 4]], w: usize, h: usize, radius: f32) -> Vec<[f32; 4]> {
    let r_low = radius.floor() as isize;
    let frac = (radius - radius.floor()).clamp(0.0, 1.0);
    let low = box_blur_f32(pixels, w, h, r_low);
    if frac < 1e-3 {
        return low;
    }
    let high = box_blur_f32(pixels, w, h, r_low + 1);
    low.into_iter()
        .zip(high)
        .map(|(a, b)| {
            [
                a[0] + (b[0] - a[0]) * frac,
                a[1] + (b[1] - a[1]) * frac,
                a[2] + (b[2] - a[2]) * frac,
                a[3] + (b[3] - a[3]) * frac,
            ]
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn downsample_rgba(
    src: &[u8],
    sw: usize,
    sh: usize,
    tw: usize,
    th: usize,
    mode: SampleMode,
    offset_x: f32,
    offset_y: f32,
) -> Vec<[f32; 4]> {
    let mut out = vec![[0.0f32; 4]; tw * th];
    let scale_x = sw as f32 / tw as f32;
    let scale_y = sh as f32 / th as f32;

    for ty in 0..th {
        for tx in 0..tw {
            let px = match mode {
                SampleMode::Point => {
                    let sx = (((tx as f32 + offset_x) * scale_x).floor() as usize).min(sw - 1);
                    let sy = (((ty as f32 + offset_y) * scale_y).floor() as usize).min(sh - 1);
                    let idx = (sy * sw + sx) * 4;
                    [
                        f32::from(src[idx]),
                        f32::from(src[idx + 1]),
                        f32::from(src[idx + 2]),
                        f32::from(src[idx + 3]),
                    ]
                }
                SampleMode::Bilinear => {
                    let fx = ((tx as f32 + offset_x) * scale_x - 0.5).clamp(0.0, (sw - 1) as f32);
                    let fy = ((ty as f32 + offset_y) * scale_y - 0.5).clamp(0.0, (sh - 1) as f32);
                    let x0 = fx.floor() as usize;
                    let y0 = fy.floor() as usize;
                    let x1 = (x0 + 1).min(sw - 1);
                    let y1 = (y0 + 1).min(sh - 1);
                    let wx = fx - x0 as f32;
                    let wy = fy - y0 as f32;
                    let sample = |x: usize, y: usize| -> [f32; 4] {
                        let i = (y * sw + x) * 4;
                        [
                            f32::from(src[i]),
                            f32::from(src[i + 1]),
                            f32::from(src[i + 2]),
                            f32::from(src[i + 3]),
                        ]
                    };
                    let c00 = sample(x0, y0);
                    let c10 = sample(x1, y0);
                    let c01 = sample(x0, y1);
                    let c11 = sample(x1, y1);
                    let mut res = [0.0f32; 4];
                    for c in 0..4 {
                        let top = c00[c] * (1.0 - wx) + c10[c] * wx;
                        let bot = c01[c] * (1.0 - wx) + c11[c] * wx;
                        res[c] = top * (1.0 - wy) + bot * wy;
                    }
                    res
                }
                SampleMode::Average => {
                    let shift_x = (offset_x - 0.5) * scale_x;
                    let shift_y = (offset_y - 0.5) * scale_y;
                    let x0 = ((tx as f32 * scale_x + shift_x).floor() as isize)
                        .clamp(0, sw as isize - 1) as usize;
                    let x1 = (((tx + 1) as f32 * scale_x + shift_x).ceil() as isize - 1)
                        .clamp(x0 as isize, sw as isize - 1) as usize;
                    let y0 = ((ty as f32 * scale_y + shift_y).floor() as isize)
                        .clamp(0, sh as isize - 1) as usize;
                    let y1 = (((ty + 1) as f32 * scale_y + shift_y).ceil() as isize - 1)
                        .clamp(y0 as isize, sh as isize - 1) as usize;
                    let mut acc = [0.0f32; 4];
                    let mut count = 0.0f32;
                    for sy in y0..=y1 {
                        for sx in x0..=x1 {
                            let i = (sy * sw + sx) * 4;
                            acc[0] += f32::from(src[i]);
                            acc[1] += f32::from(src[i + 1]);
                            acc[2] += f32::from(src[i + 2]);
                            acc[3] += f32::from(src[i + 3]);
                            count += 1.0;
                        }
                    }
                    [
                        acc[0] / count,
                        acc[1] / count,
                        acc[2] / count,
                        acc[3] / count,
                    ]
                }
            };
            out[ty * tw + tx] = px;
        }
    }
    out
}

fn sharpen_rgba(pixels: &[[f32; 4]], w: usize, h: usize, amount: f32) -> Vec<[f32; 4]> {
    let mut out = pixels.to_vec();
    for y in 0..h {
        for x in 0..w {
            let c = pixels[y * w + x];
            let l = pixels[y * w + x.saturating_sub(1)];
            let r = pixels[y * w + (x + 1).min(w - 1)];
            let u = pixels[y.saturating_sub(1) * w + x];
            let d = pixels[(y + 1).min(h - 1) * w + x];
            for ch in 0..3 {
                let lap = 4.0 * c[ch] - (l[ch] + r[ch] + u[ch] + d[ch]);
                out[y * w + x][ch] = (c[ch] + amount * lap).clamp(0.0, 255.0);
            }
        }
    }
    out
}

fn apply_adjustments(pixels: &mut [[f32; 4]], params: &Img2PixelParams) {
    let bright = params.brightness.clamp(-1.0, 1.0) * 255.0;
    let contrast = params.contrast.clamp(0.1, 4.0);
    let sat = params.saturation.clamp(0.0, 3.0);
    let inv_gamma = 1.0 / params.gamma.clamp(0.1, 4.0);
    let hue_deg = params.hue_shift;

    for px in pixels.iter_mut() {
        let mut r = (px[0] / 255.0).clamp(0.0, 1.0);
        let mut g = (px[1] / 255.0).clamp(0.0, 1.0);
        let mut b = (px[2] / 255.0).clamp(0.0, 1.0);

        if (inv_gamma - 1.0).abs() > 1e-4 {
            r = r.powf(inv_gamma);
            g = g.powf(inv_gamma);
            b = b.powf(inv_gamma);
        }

        r = ((r * 255.0 - 128.0) * contrast + 128.0 + bright).clamp(0.0, 255.0);
        g = ((g * 255.0 - 128.0) * contrast + 128.0 + bright).clamp(0.0, 255.0);
        b = ((b * 255.0 - 128.0) * contrast + 128.0 + bright).clamp(0.0, 255.0);

        if (sat - 1.0).abs() > 1e-4 {
            let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            r = (luma + (r - luma) * sat).clamp(0.0, 255.0);
            g = (luma + (g - luma) * sat).clamp(0.0, 255.0);
            b = (luma + (b - luma) * sat).clamp(0.0, 255.0);
        }

        if hue_deg.abs() > 1e-3 {
            let (mut h, s, v) = rgb_to_hsv(r / 255.0, g / 255.0, b / 255.0);
            h = (h + hue_deg).rem_euclid(360.0);
            let (nr, ng, nb) = hsv_to_rgb(h, s, v);
            r = nr * 255.0;
            g = ng * 255.0;
            b = nb * 255.0;
        }

        px[0] = r;
        px[1] = g;
        px[2] = b;
    }
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d < 1e-6 {
        0.0
    } else if (max - r).abs() < 1e-6 {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if (max - g).abs() < 1e-6 {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max < 1e-6 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match (h / 60.0).floor() as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r1 + m, g1 + m, b1 + m)
}

#[inline]
fn color_dist_sq(a: [f32; 3], b: [f32; 3], metric: ColorDistanceMode) -> f32 {
    let dr = a[0] - b[0];
    let dg = a[1] - b[1];
    let db = a[2] - b[2];
    match metric {
        ColorDistanceMode::RgbEuclidean => dr * dr + dg * dg + db * db,
        ColorDistanceMode::PerceptualRedmean => {
            let r_mean = 0.5 * (a[0] + b[0]);
            (2.0 + r_mean / 256.0) * dr * dr
                + 4.0 * dg * dg
                + (2.0 + (255.0 - r_mean) / 256.0) * db * db
        }
    }
}

/// Deterministic farthest-point seeding + K-means clustering over opaque pixels.
fn extract_kmeans_palette(
    pixels: &[[f32; 4]],
    k: usize,
    metric: ColorDistanceMode,
) -> Vec<[u8; 4]> {
    let samples: Vec<[f32; 3]> = pixels
        .iter()
        .filter(|p| p[3] >= 16.0)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    if samples.is_empty() {
        return Vec::new();
    }
    let k = k.min(samples.len()).max(1);

    // Seed 0: first sample (or darkest/mean); then deterministic farthest-point
    // selection so every run on identical input yields the exact same palette.
    let mut centroids = Vec::with_capacity(k);
    centroids.push(samples[0]);
    let mut min_dists = vec![f32::MAX; samples.len()];
    for _ in 1..k {
        let last = *centroids.last().unwrap();
        let mut best_idx = 0usize;
        let mut best_dist = -1.0f32;
        for (i, &s) in samples.iter().enumerate() {
            let d = color_dist_sq(s, last, metric);
            if d < min_dists[i] {
                min_dists[i] = d;
            }
            if min_dists[i] > best_dist {
                best_dist = min_dists[i];
                best_idx = i;
            }
        }
        if best_dist <= 0.0 {
            break;
        }
        centroids.push(samples[best_idx]);
    }

    // Lloyd iterations.
    let actual_k = centroids.len();
    for _ in 0..8 {
        let mut sums = vec![[0.0f64; 3]; actual_k];
        let mut counts = vec![0usize; actual_k];
        for &s in &samples {
            let idx = nearest_centroid(&centroids, s, metric);
            sums[idx][0] += f64::from(s[0]);
            sums[idx][1] += f64::from(s[1]);
            sums[idx][2] += f64::from(s[2]);
            counts[idx] += 1;
        }
        for i in 0..actual_k {
            if counts[i] > 0 {
                let c = counts[i] as f64;
                centroids[i] = [
                    (sums[i][0] / c) as f32,
                    (sums[i][1] / c) as f32,
                    (sums[i][2] / c) as f32,
                ];
            }
        }
    }

    let mut result: Vec<[u8; 4]> = centroids
        .into_iter()
        .map(|c| [c[0].round() as u8, c[1].round() as u8, c[2].round() as u8, 255])
        .collect();
    result.sort_by_key(|c| (c[0], c[1], c[2]));
    result.dedup();
    result
}

fn nearest_centroid(centroids: &[[f32; 3]], s: [f32; 3], metric: ColorDistanceMode) -> usize {
    let mut best_i = 0usize;
    let mut best_d = f32::MAX;
    for (i, &c) in centroids.iter().enumerate() {
        let d = color_dist_sq(s, c, metric);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }
    best_i
}

fn nearest_palette_color(
    palette: &[[f32; 3]],
    c: [f32; 3],
    metric: ColorDistanceMode,
) -> [f32; 3] {
    palette[nearest_centroid(palette, c, metric)]
}

const BAYER_2X2: [[f32; 2]; 2] = [[0.0, 2.0], [3.0, 1.0]];

const BAYER_4X4: [[f32; 4]; 4] = [
    [0.0, 8.0, 2.0, 10.0],
    [12.0, 4.0, 14.0, 6.0],
    [3.0, 11.0, 1.0, 9.0],
    [15.0, 7.0, 13.0, 5.0],
];

const BAYER_8X8: [[f32; 8]; 8] = [
    [0.0, 32.0, 8.0, 40.0, 2.0, 34.0, 10.0, 42.0],
    [48.0, 16.0, 56.0, 24.0, 50.0, 18.0, 58.0, 26.0],
    [12.0, 44.0, 4.0, 36.0, 14.0, 46.0, 6.0, 38.0],
    [60.0, 28.0, 52.0, 20.0, 62.0, 30.0, 54.0, 22.0],
    [3.0, 35.0, 11.0, 43.0, 1.0, 33.0, 9.0, 41.0],
    [51.0, 19.0, 59.0, 27.0, 49.0, 17.0, 57.0, 25.0],
    [15.0, 47.0, 7.0, 39.0, 13.0, 45.0, 5.0, 37.0],
    [63.0, 31.0, 55.0, 23.0, 61.0, 29.0, 53.0, 21.0],
];

const CLUSTER_4X4: [[f32; 4]; 4] = [
    [12.0, 5.0, 6.0, 13.0],
    [4.0, 0.0, 1.0, 7.0],
    [11.0, 3.0, 2.0, 8.0],
    [15.0, 10.0, 9.0, 14.0],
];

fn ordered_threshold(mode: DitherMode, x: usize, y: usize) -> f32 {
    match mode {
        DitherMode::Bayer2x2 => (BAYER_2X2[y & 1][x & 1] + 0.5) / 4.0 - 0.5,
        DitherMode::Bayer4x4 => (BAYER_4X4[y & 3][x & 3] + 0.5) / 16.0 - 0.5,
        DitherMode::Bayer8x8 => (BAYER_8X8[y & 7][x & 7] + 0.5) / 64.0 - 0.5,
        DitherMode::Cluster4x4 => (CLUSTER_4X4[y & 3][x & 3] + 0.5) / 16.0 - 0.5,
        DitherMode::None | DitherMode::FloydSteinberg => 0.0,
    }
}

fn quantize_and_dither(
    pixels: &mut [[f32; 4]],
    w: usize,
    h: usize,
    palette: &[[u8; 4]],
    metric: ColorDistanceMode,
    dither: DitherMode,
    amount: f32,
) {
    let pal: Vec<[f32; 3]> = palette
        .iter()
        .map(|c| [f32::from(c[0]), f32::from(c[1]), f32::from(c[2])])
        .collect();
    if pal.is_empty() {
        return;
    }

    if dither == DitherMode::FloydSteinberg {
        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                let old = [pixels[idx][0], pixels[idx][1], pixels[idx][2]];
                let new = nearest_palette_color(&pal, old, metric);
                pixels[idx][0] = new[0];
                pixels[idx][1] = new[1];
                pixels[idx][2] = new[2];
                let err = [
                    (old[0] - new[0]) * amount,
                    (old[1] - new[1]) * amount,
                    (old[2] - new[2]) * amount,
                ];
                let mut add_err = |nx: usize, ny: usize, factor: f32| {
                    let ni = ny * w + nx;
                    for c in 0..3 {
                        pixels[ni][c] = (pixels[ni][c] + err[c] * factor).clamp(0.0, 255.0);
                    }
                };
                if x + 1 < w {
                    add_err(x + 1, y, 7.0 / 16.0);
                }
                if y + 1 < h {
                    if x > 0 {
                        add_err(x - 1, y + 1, 3.0 / 16.0);
                    }
                    add_err(x, y + 1, 5.0 / 16.0);
                    if x + 1 < w {
                        add_err(x + 1, y + 1, 1.0 / 16.0);
                    }
                }
            }
        }
        return;
    }

    let spread = 64.0 * amount;
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let t = ordered_threshold(dither, x, y) * spread;
            let adjusted = [
                (pixels[idx][0] + t).clamp(0.0, 255.0),
                (pixels[idx][1] + t).clamp(0.0, 255.0),
                (pixels[idx][2] + t).clamp(0.0, 255.0),
            ];
            let mapped = nearest_palette_color(&pal, adjusted, metric);
            pixels[idx][0] = mapped[0];
            pixels[idx][1] = mapped[1];
            pixels[idx][2] = mapped[2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsamples_and_quantizes_with_kmeans() {
        let mut src = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16 {
            for x in 0..16 {
                let c = if x < 8 && y < 8 {
                    [220u8, 40, 40, 255]
                } else if x >= 8 && y < 8 {
                    [40, 200, 60, 255]
                } else {
                    [30, 70, 230, 255]
                };
                src.extend_from_slice(&c);
            }
        }
        let params = Img2PixelParams {
            target_width: 4,
            target_height: 4,
            palette_mode: PaletteMode::ExtractKMeans,
            kmeans_colors: 3,
            dither_mode: DitherMode::None,
            ..Img2PixelParams::default()
        };
        let out = process_image_to_pixel(16, 16, &src, &[], &params);
        assert_eq!(out.width, 4);
        assert_eq!(out.height, 4);
        assert_eq!(out.rgba.len(), 4 * 4 * 4);
        let pal = out.extracted_palette.expect("kmeans palette returned");
        assert_eq!(pal.len(), 3);
    }

    #[test]
    fn alpha_threshold_clips_semi_transparent_pixels() {
        let src = [200u8, 100, 50, 100, 200, 100, 50, 200];
        let params = Img2PixelParams {
            target_width: 2,
            target_height: 1,
            palette_mode: PaletteMode::Original,
            alpha_threshold: 128,
            ..Img2PixelParams::default()
        };
        let out = process_image_to_pixel(2, 1, &src, &[], &params);
        assert_eq!(out.rgba[3], 0);
        assert_eq!(out.rgba[7], 255);
    }

    #[test]
    fn floyd_steinberg_quantizes_only_to_project_palette() {
        let mut src = Vec::with_capacity(8 * 8 * 4);
        for i in 0..64 {
            let v = (i * 4) as u8;
            src.extend_from_slice(&[v, v, v, 255]);
        }
        let palette = [[0, 0, 0, 255], [255, 255, 255, 255]];
        let params = Img2PixelParams {
            target_width: 8,
            target_height: 8,
            palette_mode: PaletteMode::ProjectPalette,
            dither_mode: DitherMode::FloydSteinberg,
            dither_amount: 1.0,
            ..Img2PixelParams::default()
        };
        let out = process_image_to_pixel(8, 8, &src, &palette, &params);
        for px in out.rgba.chunks_exact(4) {
            assert!(
                px == [0, 0, 0, 255] || px == [255, 255, 255, 255],
                "unexpected color {px:?}"
            );
        }
    }
}
