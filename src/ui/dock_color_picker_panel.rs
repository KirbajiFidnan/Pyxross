//! Floating Color Picker dock panel (id 104).
//!
//! Three columns: an HSV square with a bound vertical hue slider, the New/Old
//! previews (both clickable: Old reverts to the color the panel opened with,
//! New restores the last working color), the RGBA/HSL/alpha/hex fields, the
//! overlapping FOREGROUND/BACKGROUND swatch pair with a Swap button, and four
//! clickable variation chip rows (Shading, Lightness, Saturation, Hue).
//! Registered as a `PanelPlacement::Floating` dock panel with
//! `can_pop_out == false` and `resizable == false`, so it floats, cannot be
//! docked or edge-resized, and closes from its header's Close action.
//!
//! The panel is a pure view over [`ColorPickerHost`]: every gesture pushes a
//! [`ToolbarEvent::ColorChanged`] (or [`ToolbarEvent::SwapColors`]) for the App
//! to apply to the active session, so edits are live. The square is HSV; the
//! H/S/L fields are HSL; the two spaces are converted through the pure helpers
//! below.

use crate::core::color::Color;
use crate::ui::dock_hosts::{ColorPickerHost, ColorPickerView};
use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
    FLOATING_PANEL_CHROME_HEIGHT,
};
use crate::ui::theme::SkinState;
use crate::ui::toolbar::ToolbarEvent;

/// The dock id for the Color Picker panel.
pub const COLOR_PICKER_PANEL_ID: u64 = 104;

/// Edge length of the cached square gradient texture, in pixels.
const SQUARE_TEXTURE_SIZE: usize = 64;

/// Height of the cached hue bar gradient texture, in pixels.
const HUE_TEXTURE_SIZE: usize = 64;

/// Debug label of the cached square gradient texture.
const SQUARE_TEXTURE_NAME: &str = "color-picker-square";

/// Debug label of the cached hue bar gradient texture.
const HUE_TEXTURE_NAME: &str = "color-picker-hue";

/// Width of the vertical hue slider, in points.
const HUE_BAR_WIDTH: f32 = 16.0;

/// Width of the RGB/HSL/hex field column, in points: the widest field row
/// (the hex row) plus the label gutter.
const MIDDLE_COLUMN_WIDTH: f32 = 154.0;

/// Width of the variation chip column, in points: nine chips plus the gaps
/// between them (with slack), so the finer Shading/Lightness rows stay on one
/// line in the panel width.
const RIGHT_COLUMN_WIDTH: f32 = 9.0 * CHIP_SIZE + 8.0 * CHIP_GAP + 2.0;

/// Width of one numeric field, in points.
const FIELD_WIDTH: f32 = 34.0;

/// Height of one field row, in points.
const FIELD_HEIGHT: f32 = 18.0;

/// Width of the hex text field, in points.
const HEX_FIELD_WIDTH: f32 = 58.0;

/// Gap between the three columns, in points.
const COLUMN_GAP: f32 = 8.0;

/// Smallest square edge that still shows a usable gradient.
const MIN_SQUARE_SIDE: f32 = 48.0;

/// Largest square edge, so the panel stays three-column on wide docks.
const MAX_SQUARE_SIDE: f32 = 168.0;

/// Side length of one clickable color chip, in points.
const CHIP_SIZE: f32 = 14.0;

/// Horizontal gap between chips in one row, in points.
const CHIP_GAP: f32 = 3.0;

/// Side length of one FG/BG swatch square: twice the variation chips.
const FG_BG_SWATCH_SIZE: f32 = CHIP_SIZE * 2.0;

/// Down-right offset of the BACKGROUND square behind the FOREGROUND square,
/// scaled with the doubled swatches.
const FG_BG_SWATCH_OFFSET: f32 = CHIP_SIZE * 0.7;

/// Font size of the FG/BG swatch labels, in points.
const FG_BG_LABEL_FONT: f32 = 10.0;

/// Width of the FG/BG swap button, in points.
const SWAP_BUTTON_WIDTH: f32 = 34.0;

/// Height of the FG/BG swap button, in points.
const SWAP_BUTTON_HEIGHT: f32 = 18.0;

/// Interaction id of the FOREGROUND swatch square.
const FG_SWATCH_ID: &str = "color-picker-fg-swatch";

/// Interaction id of the BACKGROUND swatch square.
const BG_SWATCH_ID: &str = "color-picker-bg-swatch";

/// Interaction id of the primary/secondary swap button.
const SWAP_BUTTON_ID: &str = "color-picker-swap-colors";

/// Height of the New/Old preview swatches, in points.
const PREVIEW_HEIGHT: f32 = 34.0;

/// Content height reserved for the nest, in points: the middle column's
/// natural height (the New/Old preview row, the four field rows and the FG/BG
/// pair with its swap button), the tallest of the three columns.
const CONTENT_HEIGHT: f32 = 196.0;

/// Floating window height that fits the picker content snugly: the nest
/// content plus the floating panel chrome around it. Fixed for every picker
/// spec, so a remembered floating rect's height is normalized away.
pub const PICKER_WINDOW_HEIGHT: f32 = CONTENT_HEIGHT + FLOATING_PANEL_CHROME_HEIGHT;

/// Smallest usable picker width, in points.
const MIN_PANEL_WIDTH: f32 = 520.0;

/// Number of brighter chips before the base chip in the Shading row.
const SHADING_BRIGHT_STEPS: usize = 4;

/// Number of darker chips after the base chip in the Shading row.
const SHADING_DARK_STEPS: usize = 4;

/// Number of chips in the Shading row: brighter + base + darker.
const SHADING_CHIP_COUNT: usize = SHADING_BRIGHT_STEPS + 1 + SHADING_DARK_STEPS;

/// HSV value removed per dark shading ramp step.
const SHADING_DARKEN_STEP: f32 = 0.2;

/// Fraction of the headroom to white added per bright shading ramp step.
const SHADING_BRIGHTEN_STEP: f32 = 0.25;

/// Hue the bright shading ramp moves toward: yellow, the classic
/// complementary warm-highlight anchor.
const BRIGHT_SHADING_HUE_ANCHOR: f32 = 60.0;

/// Number of chips in the Lightness row (nine even lightness levels).
const LIGHTNESS_CHIP_COUNT: usize = 9;

/// Lightness ladder of the Lightness row (light to dark).
const LIGHTNESS_LEVELS: [f32; LIGHTNESS_CHIP_COUNT] = [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1];

/// Number of chips in the Saturation and Hue rows.
const AXIS_CHIP_COUNT: usize = 7;

/// Hue anchors of the shading map: `(base hue, signed hue shift)`.
///
/// blue -> toward red, green -> toward blue, red -> toward blue,
/// yellow -> toward red, cyan -> toward blue, magenta -> toward blue. "Toward"
/// is the shortest rotation on the wheel, hence the signed degrees.
const SHADING_ANCHORS: [(f32, f32); 7] = [
    (0.0, -120.0),
    (60.0, -60.0),
    (120.0, 120.0),
    (180.0, 60.0),
    (240.0, 120.0),
    (300.0, -60.0),
    (360.0, -120.0),
];

/// Full-texture UV rect for the cached gradient textures.
const FULL_UV: egui::Rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

/// Neutral multiply tint for the gradient textures (not chrome).
const NEUTRAL_TEXTURE_TINT: egui::Color32 = egui::Color32::WHITE; // NB: neutral texture tint (multiply blend, not chrome)

/// Rounds a unit float to an 8-bit channel.
fn channel(value: f32) -> u8 {
    let scaled = (value.clamp(0.0, 1.0) * 255.0).round();
    // `scaled` is clamped to [0, 255], so the cast cannot truncate.
    scaled as u8
}

/// Converts a straight-alpha color to HSV: hue in degrees, saturation and
/// value in [0, 1].
pub fn color_to_hsv(color: Color) -> (f32, f32, f32) {
    let r = f32::from(color.r) / 255.0;
    let g = f32::from(color.g) / 255.0;
    let b = f32::from(color.b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue.rem_euclid(360.0), saturation.clamp(0.0, 1.0), max)
}

/// Converts HSV (hue degrees, saturation/value in [0, 1]) to a color.
pub fn hsv_to_color(hue: f32, saturation: f32, value: f32, alpha: u8) -> Color {
    let hue = hue.rem_euclid(360.0);
    let saturation = saturation.clamp(0.0, 1.0);
    let value = value.clamp(0.0, 1.0);
    let chroma = value * saturation;
    let sector = hue / 60.0;
    let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match sector.floor() as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = value - chroma;
    Color::rgba(channel(r1 + m), channel(g1 + m), channel(b1 + m), alpha)
}

/// Converts a straight-alpha color to HSL: hue in degrees, saturation and
/// lightness in [0, 1].
pub fn color_to_hsl(color: Color) -> (f32, f32, f32) {
    let (hue, _, max) = color_to_hsv(color);
    let min = f32::from(color.r.min(color.g).min(color.b)) / 255.0;
    let delta = max - min;
    let lightness = (max + min) / 2.0;
    let saturation = if delta <= f32::EPSILON {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };
    (hue, saturation.clamp(0.0, 1.0), lightness)
}

/// Converts HSL (hue degrees, saturation/lightness in [0, 1]) to a color.
pub fn hsl_to_color(hue: f32, saturation: f32, lightness: f32, alpha: u8) -> Color {
    let hue = hue.rem_euclid(360.0);
    let saturation = saturation.clamp(0.0, 1.0);
    let lightness = lightness.clamp(0.0, 1.0);
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue / 60.0;
    let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match sector.floor() as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = lightness - chroma / 2.0;
    Color::rgba(channel(r1 + m), channel(g1 + m), channel(b1 + m), alpha)
}

/// The signed hue shift (degrees) the shading map applies to `base_hue`.
///
/// Hues between two anchors interpolate the two neighbour shifts by their
/// weights, so e.g. orange (between red and yellow) gets `-90`.
pub fn shading_hue_shift(base_hue: f32) -> f32 {
    let hue = base_hue.rem_euclid(360.0);
    for segment in SHADING_ANCHORS.windows(2) {
        let (start_hue, start_shift) = segment[0];
        let (end_hue, end_shift) = segment[1];
        if (start_hue..=end_hue).contains(&hue) {
            let span = end_hue - start_hue;
            let weight = if span <= f32::EPSILON {
                0.0
            } else {
                (hue - start_hue) / span
            };
            return start_shift + (end_shift - start_shift) * weight;
        }
    }
    SHADING_ANCHORS[0].1
}

/// The signed hue shift (degrees) the bright shading ramp applies to
/// `base_hue`: the shortest rotation toward the yellow anchor, capped at the
/// dark map's shift magnitude so both sides move a comparable amount.
///
/// Ties (a hue exactly opposite yellow) resolve to the warm side, matching
/// the classic warm-highlight rule.
pub fn bright_shading_hue_shift(base_hue: f32) -> f32 {
    let hue = base_hue.rem_euclid(360.0);
    let toward_yellow = (BRIGHT_SHADING_HUE_ANCHOR - hue).rem_euclid(360.0);
    let signed = if toward_yellow > 180.0 {
        toward_yellow - 360.0
    } else {
        toward_yellow
    };
    let magnitude = shading_hue_shift(hue).abs();
    signed.clamp(-magnitude, magnitude)
}

/// The Shading row: nine tones of `base`, brightest first. The four chips
/// before the base brighten (hue toward yellow, value toward white), the base
/// sits in the middle, and the four chips after it darken along the existing
/// hue-shift map with the same step sizes.
pub fn shading_chips(base: Color) -> [Color; SHADING_CHIP_COUNT] {
    let (hue, saturation, value) = color_to_hsv(base);
    let dark_shift = shading_hue_shift(hue);
    let bright_shift = bright_shading_hue_shift(hue);
    let steps = SHADING_BRIGHT_STEPS as f32;
    let mut chips = [base; SHADING_CHIP_COUNT];
    for (index, chip) in chips.iter_mut().enumerate() {
        let (chip_hue, chip_value) = if index < SHADING_BRIGHT_STEPS {
            let step = (SHADING_BRIGHT_STEPS - index) as f32;
            let progress = step / steps;
            (
                hue + bright_shift * progress,
                value + (1.0 - value) * SHADING_BRIGHTEN_STEP * step,
            )
        } else if index == SHADING_BRIGHT_STEPS {
            (hue, value)
        } else {
            let step = (index - SHADING_BRIGHT_STEPS) as f32;
            let progress = step / steps;
            (
                hue + dark_shift * progress,
                value * (1.0 - SHADING_DARKEN_STEP * step),
            )
        };
        *chip = hsv_to_color(chip_hue, saturation, chip_value, base.a);
    }
    chips[SHADING_BRIGHT_STEPS] = base;
    chips
}

/// The Lightness row: nine chips with the base hue/saturation and the fixed
/// light-to-dark lightness ladder.
pub fn lightness_chips(base: Color) -> [Color; LIGHTNESS_CHIP_COUNT] {
    let (hue, saturation, _) = color_to_hsl(base);
    LIGHTNESS_LEVELS.map(|lightness| hsl_to_color(hue, saturation, lightness, base.a))
}

/// The Saturation row: seven chips from fully saturated to fully desaturated,
/// keeping the base hue and lightness.
pub fn saturation_chips(base: Color) -> [Color; 7] {
    let (hue, _, lightness) = color_to_hsl(base);
    std::array::from_fn(|index| {
        let saturation = 1.0 - index as f32 / (AXIS_CHIP_COUNT - 1) as f32;
        hsl_to_color(hue, saturation, lightness, base.a)
    })
}

/// The Hue row: seven evenly spaced rotations around the wheel starting at the
/// base hue, keeping the base saturation and lightness.
pub fn hue_chips(base: Color) -> [Color; 7] {
    let (hue, saturation, lightness) = color_to_hsl(base);
    std::array::from_fn(|index| {
        let rotated = hue + index as f32 * (360.0 / AXIS_CHIP_COUNT as f32);
        hsl_to_color(rotated, saturation, lightness, base.a)
    })
}

/// Formats a color as `#RRGGBB` (alpha is not represented).
pub fn format_hex(color: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

/// Parses `#RRGGBB` (the leading `#` is optional) into an opaque color.
pub fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.trim().strip_prefix('#').unwrap_or_else(|| text.trim());
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    Some(Color::rgb(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ))
}

/// Converts a core color to an egui color.
fn to_color32(color: Color) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

/// The square's S/V gradient for `hue`: horizontal saturation, vertical value
/// (top = bright).
fn square_gradient_image(hue: f32) -> egui::ColorImage {
    let size = SQUARE_TEXTURE_SIZE;
    let last = (size - 1) as f32;
    let mut pixels = Vec::with_capacity(size * size);
    for y in 0..size {
        let value = 1.0 - y as f32 / last;
        for x in 0..size {
            let saturation = x as f32 / last;
            pixels.push(to_color32(hsv_to_color(hue, saturation, value, 255)));
        }
    }
    egui::ColorImage::new([size, size], pixels)
}

/// The vertical hue bar gradient: hue 0 at the top to hue 360 at the bottom.
fn hue_bar_gradient_image() -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(HUE_TEXTURE_SIZE);
    for y in 0..HUE_TEXTURE_SIZE {
        let hue = y as f32 / HUE_TEXTURE_SIZE as f32 * 360.0;
        pixels.push(to_color32(hsv_to_color(hue, 1.0, 1.0, 255)));
    }
    egui::ColorImage::new([1, HUE_TEXTURE_SIZE], pixels)
}

/// The color a pick at `pos` inside `rect` produces, keeping `hue` and `alpha`.
fn pick_sv(rect: egui::Rect, pos: egui::Pos2, hue: f32, alpha: u8) -> Color {
    let saturation = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    let value = 1.0 - ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
    hsv_to_color(hue, saturation, value, alpha)
}

/// The hue a pick at `pos` inside the hue bar produces.
fn hue_at(rect: egui::Rect, pos: egui::Pos2) -> f32 {
    (((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0) * 360.0).rem_euclid(360.0)
}

/// The three-column color picker body.
struct ColorPickerComponent {
    host: ColorPickerHost,
    /// The square's hue, remembered across frames so a pick never loses it.
    hue: f32,
    /// The color `hue` was last derived from; any other color re-derives it.
    hue_color: Option<Color>,
    /// Cached square gradient; re-uploaded only when the hue changes.
    square_texture: Option<egui::TextureHandle>,
    /// The hue the cached square texture was built for.
    square_texture_hue: f32,
    /// Cached hue bar gradient, uploaded once.
    hue_texture: Option<egui::TextureHandle>,
    /// In-progress hex text, reseeded from the color while unfocused.
    hex_buffer: String,
    /// True while the hex field has keyboard focus.
    hex_editing: bool,
}

impl ColorPickerComponent {
    fn new(host: ColorPickerHost) -> Self {
        Self {
            host,
            hue: 0.0,
            hue_color: None,
            square_texture: None,
            square_texture_hue: 0.0,
            hue_texture: None,
            hex_buffer: String::new(),
            hex_editing: false,
        }
    }

    fn emit(&self, color: Color) {
        self.host
            .events
            .borrow_mut()
            .push(ToolbarEvent::ColorChanged(color));
    }

    /// Sets the working hue (wrapped to 0..360) and emits the resulting color
    /// when it differs from the current one.
    fn apply_hue(&mut self, new_hue: f32, view: &ColorPickerView) {
        self.hue = new_hue.rem_euclid(360.0);
        let (_, saturation, value) = color_to_hsv(view.color);
        let color = hsv_to_color(self.hue, saturation, value, view.color.a);
        self.hue_color = Some(color);
        if color != view.color {
            self.emit(color);
        }
    }

    fn paint_square(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        let stale = self
            .square_texture
            .as_ref()
            .is_none_or(|_| (self.square_texture_hue - self.hue).abs() > f32::EPSILON);
        if stale {
            self.square_texture = Some(ui.ctx().load_texture(
                SQUARE_TEXTURE_NAME,
                square_gradient_image(self.hue),
                egui::TextureOptions::LINEAR,
            ));
            self.square_texture_hue = self.hue;
        }
        if let Some(texture) = &self.square_texture {
            ui.painter()
                .image(texture.id(), rect, FULL_UV, NEUTRAL_TEXTURE_TINT);
        }
    }

    fn paint_hue_bar(&mut self, ui: &egui::Ui, rect: egui::Rect) {
        if self.hue_texture.is_none() {
            self.hue_texture = Some(ui.ctx().load_texture(
                HUE_TEXTURE_NAME,
                hue_bar_gradient_image(),
                egui::TextureOptions::LINEAR,
            ));
        }
        if let Some(texture) = &self.hue_texture {
            ui.painter()
                .image(texture.id(), rect, FULL_UV, NEUTRAL_TEXTURE_TINT);
        }
    }

    fn left_column(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome, view: &ColorPickerView) {
        let available = ui.available_width();
        let left_width = (available - MIDDLE_COLUMN_WIDTH - RIGHT_COLUMN_WIDTH - 2.0 * COLUMN_GAP)
            .clamp(
                MIN_SQUARE_SIDE + HUE_BAR_WIDTH + 6.0,
                MAX_SQUARE_SIDE + HUE_BAR_WIDTH + 6.0,
            );
        let square_side =
            (left_width - HUE_BAR_WIDTH - 6.0).clamp(MIN_SQUARE_SIDE, MAX_SQUARE_SIDE);
        ui.allocate_ui_with_layout(
            egui::vec2(left_width, CONTENT_HEIGHT),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.horizontal_top(|ui| {
                    // `square_side` already reserves this gap, so the row's own
                    // item spacing must match it.
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (square_rect, _) = ui.allocate_exact_size(
                        egui::vec2(square_side, square_side),
                        egui::Sense::hover(),
                    );
                    let square = ui.interact(
                        square_rect,
                        egui::Id::new("color-picker-sv-square"),
                        egui::Sense::click_and_drag(),
                    );
                    self.paint_square(ui, square_rect);
                    if let Some(pos) = square.interact_pointer_pos() {
                        if square.clicked()
                            || square.dragged()
                            || square.is_pointer_button_down_on()
                        {
                            let color = pick_sv(square_rect, pos, self.hue, view.color.a);
                            if color != view.color {
                                self.hue_color = Some(color);
                                self.emit(color);
                            }
                        }
                    }
                    let (hue_rect, _) = ui.allocate_exact_size(
                        egui::vec2(HUE_BAR_WIDTH, square_side),
                        egui::Sense::hover(),
                    );
                    let hue_bar = ui.interact(
                        hue_rect,
                        egui::Id::new("color-picker-hue"),
                        egui::Sense::click_and_drag(),
                    );
                    self.paint_hue_bar(ui, hue_rect);
                    if let Some(pos) = hue_bar.interact_pointer_pos() {
                        if hue_bar.clicked()
                            || hue_bar.dragged()
                            || hue_bar.is_pointer_button_down_on()
                        {
                            let new_hue = hue_at(hue_rect, pos);
                            if (new_hue - self.hue).abs() > f32::EPSILON {
                                self.apply_hue(new_hue, view);
                            }
                        }
                    }
                    let hue_notches = wheel_notches(ui, hue_bar.contains_pointer());
                    if hue_notches != 0.0 {
                        self.apply_hue(self.hue + hue_notches, view);
                    }
                    let marker = chrome.colors.selection_stroke_color32();
                    let (_, saturation, value) = color_to_hsv(view.color);
                    let marker_pos = egui::pos2(
                        square_rect.left() + saturation * square_rect.width(),
                        square_rect.top() + (1.0 - value) * square_rect.height(),
                    );
                    ui.painter()
                        .circle_stroke(marker_pos, 5.0, egui::Stroke::new(2.0, marker));
                    let hue_y = hue_rect.top() + self.hue / 360.0 * hue_rect.height();
                    ui.painter().line_segment(
                        [
                            egui::pos2(hue_rect.left(), hue_y),
                            egui::pos2(hue_rect.right(), hue_y),
                        ],
                        egui::Stroke::new(2.0, marker),
                    );
                });
            },
        );
    }

    fn middle_column(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome, view: &ColorPickerView) {
        ui.allocate_ui_with_layout(
            egui::vec2(MIDDLE_COLUMN_WIDTH, CONTENT_HEIGHT),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                // Narrower numeric fields than the default interact size, so the
                // two field columns plus the hex field fit the middle column.
                ui.spacing_mut().interact_size.x = FIELD_WIDTH;
                let mut preview_pick = None;
                ui.horizontal(|ui| {
                    if preview_swatch(ui, chrome, "New", view.new_color).clicked() {
                        preview_pick = Some(view.new_color);
                    }
                    if preview_swatch(ui, chrome, "Old", view.old_color).clicked() {
                        preview_pick = Some(view.old_color);
                    }
                });
                ui.add_space(4.0);
                let (hue, saturation, lightness) = color_to_hsl(view.color);
                let display_hue = if saturation <= f32::EPSILON {
                    self.hue
                } else {
                    hue
                };
                let mut red = view.color.r;
                let mut green = view.color.g;
                let mut blue = view.color.b;
                let mut alpha = view.color.a;
                let mut hue_field = display_hue;
                let mut saturation_field = saturation * 100.0;
                let mut lightness_field = lightness * 100.0;
                if !self.hex_editing {
                    self.hex_buffer = format_hex(view.color);
                }
                let mut changed_color = None;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        let (red_response, red_notches) = field_row(ui, "R:", |ui| {
                            ui.add(egui::DragValue::new(&mut red).range(0..=255))
                        });
                        let (green_response, green_notches) = field_row(ui, "G:", |ui| {
                            ui.add(egui::DragValue::new(&mut green).range(0..=255))
                        });
                        let (blue_response, blue_notches) = field_row(ui, "B:", |ui| {
                            ui.add(egui::DragValue::new(&mut blue).range(0..=255))
                        });
                        let (alpha_response, alpha_notches) = field_row(ui, "A:", |ui| {
                            ui.add(egui::DragValue::new(&mut alpha).range(0..=255))
                        });
                        red = step_channel(red, red_notches);
                        green = step_channel(green, green_notches);
                        blue = step_channel(blue, blue_notches);
                        alpha = step_channel(alpha, alpha_notches);
                        if red_response.changed()
                            || green_response.changed()
                            || blue_response.changed()
                            || alpha_response.changed()
                            || red_notches != 0.0
                            || green_notches != 0.0
                            || blue_notches != 0.0
                            || alpha_notches != 0.0
                        {
                            changed_color = Some(Color::rgba(red, green, blue, alpha));
                        }
                    });
                    ui.vertical(|ui| {
                        let (hue_response, hue_notches) = field_row(ui, "H:", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut hue_field)
                                    .range(0.0..=360.0)
                                    .speed(1.0)
                                    .fixed_decimals(0),
                            )
                        });
                        let (saturation_response, saturation_notches) = field_row(ui, "S:", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut saturation_field)
                                    .range(0.0..=100.0)
                                    .speed(1.0)
                                    .fixed_decimals(0),
                            )
                        });
                        let (lightness_response, lightness_notches) = field_row(ui, "L:", |ui| {
                            ui.add(
                                egui::DragValue::new(&mut lightness_field)
                                    .range(0.0..=100.0)
                                    .speed(1.0)
                                    .fixed_decimals(0),
                            )
                        });
                        hue_field = (hue_field + hue_notches).rem_euclid(360.0);
                        saturation_field =
                            (saturation_field + saturation_notches).clamp(0.0, 100.0);
                        lightness_field = (lightness_field + lightness_notches).clamp(0.0, 100.0);
                        let (hex_response, _) = field_row(ui, "Hex:", |ui| {
                            ui.add_sized(
                                [HEX_FIELD_WIDTH, FIELD_HEIGHT],
                                egui::TextEdit::singleline(&mut self.hex_buffer),
                            )
                        });
                        if hue_response.changed()
                            || saturation_response.changed()
                            || lightness_response.changed()
                            || hue_notches != 0.0
                            || saturation_notches != 0.0
                            || lightness_notches != 0.0
                        {
                            changed_color = Some(hsl_to_color(
                                hue_field,
                                saturation_field / 100.0,
                                lightness_field / 100.0,
                                alpha,
                            ));
                        }
                        if hex_response.gained_focus() {
                            self.hex_editing = true;
                        }
                        if hex_response.changed() {
                            if let Some(parsed) = parse_hex(&self.hex_buffer) {
                                changed_color =
                                    Some(Color::rgba(parsed.r, parsed.g, parsed.b, alpha));
                            }
                        }
                        if hex_response.lost_focus() {
                            self.hex_editing = false;
                        }
                    });
                });
                ui.add_space(4.0);
                let mut swap_clicked = false;
                ui.horizontal_top(|ui| {
                    let secondary_swatch =
                        fg_bg_swatch_pair(ui, chrome, view.color, view.secondary_color);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.weak(egui::RichText::new("FOREGROUND").size(FG_BG_LABEL_FONT));
                        ui.weak(egui::RichText::new("BACKGROUND").size(FG_BG_LABEL_FONT));
                        ui.add_space(2.0);
                        swap_clicked =
                            swap_button(ui, chrome).clicked() || secondary_swatch.clicked();
                    });
                });
                if let Some(color) = preview_pick {
                    if color != view.color {
                        self.emit(color);
                    }
                }
                if swap_clicked {
                    self.host.events.borrow_mut().push(ToolbarEvent::SwapColors);
                }
                if let Some(color) = changed_color {
                    if color != view.color {
                        self.emit(color);
                    }
                }
            },
        );
    }

    fn right_column(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome, view: &ColorPickerView) {
        ui.allocate_ui_with_layout(
            egui::vec2(RIGHT_COLUMN_WIDTH, CONTENT_HEIGHT),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                let rows: [(&str, [Color; 7]); 2] = [
                    ("Saturation", saturation_chips(view.color)),
                    ("Hue", hue_chips(view.color)),
                ];
                let mut index = 0;
                chip_row(
                    ui,
                    chrome,
                    &self.host,
                    "Shading",
                    &shading_chips(view.color),
                    view.color,
                    &mut index,
                );
                chip_row(
                    ui,
                    chrome,
                    &self.host,
                    "Lightness",
                    &lightness_chips(view.color),
                    view.color,
                    &mut index,
                );
                for (label, chips) in rows {
                    chip_row(
                        ui, chrome, &self.host, label, &chips, view.color, &mut index,
                    );
                }
            },
        );
    }
}

impl Component for ColorPickerComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = *self.host.view.borrow();
        if self.hue_color != Some(view.color) {
            self.hue = color_to_hsv(view.color).0;
            self.hue_color = Some(view.color);
        }
        ui.set_min_size(egui::vec2(ui.available_width(), CONTENT_HEIGHT));
        ui.horizontal_top(|ui| {
            // The outer row's own item spacing is the column gap; adding
            // `add_space(COLUMN_GAP)` on top of it would double the gap.
            ui.spacing_mut().item_spacing.x = COLUMN_GAP;
            self.left_column(ui, chrome, &view);
            self.middle_column(ui, chrome, &view);
            self.right_column(ui, chrome, &view);
        });
    }
}

/// One labeled field row: the label, the value widget, and the wheel notches
/// the pointer scrolled while hovering it.
///
/// The value widget registers a stable hover target (`color-picker-field` +
/// label) so the wheel reaches it and tests can address the field.
fn field_row(
    ui: &mut egui::Ui,
    label: &str,
    add_value: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> (egui::Response, f32) {
    ui.horizontal(|ui| {
        ui.label(label);
        let response = add_value(ui);
        let hover = ui.interact(
            response.rect,
            egui::Id::new(("color-picker-field", label)),
            egui::Sense::hover(),
        );
        (response, wheel_notches(ui, hover.contains_pointer()))
    })
    .inner
}

/// One wheel notch per event while `hovered`: -1 (wheel down) or +1 (wheel up).
///
/// The events and the smoothed scroll delta are consumed, so the nest's scroll
/// area does not also react while the pointer is over the control.
fn wheel_notches(ui: &egui::Ui, hovered: bool) -> f32 {
    if !hovered {
        return 0.0;
    }
    ui.input_mut(|input| {
        let mut notches = 0.0;
        input.events.retain(|event| match event {
            egui::Event::MouseWheel { delta, .. } if delta.y != 0.0 => {
                notches += delta.y.signum();
                false
            }
            _ => true,
        });
        if notches != 0.0 {
            input.smooth_scroll_delta = egui::Vec2::ZERO;
        }
        notches
    })
}

/// Applies wheel notches to an 8-bit channel, clamped to 0..=255.
fn step_channel(value: u8, notches: f32) -> u8 {
    (f32::from(value) + notches).clamp(0.0, 255.0) as u8
}

/// One labeled preview swatch (the "New"/"Old" color previews).
///
/// Returns the click response; clicking applies `color` as the working color.
fn preview_swatch(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    label: &str,
    color: Color,
) -> egui::Response {
    ui.vertical(|ui| {
        ui.weak(label);
        let width = (MIDDLE_COLUMN_WIDTH - 8.0) / 2.0;
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(width, PREVIEW_HEIGHT), egui::Sense::hover());
        let response = ui.interact(
            rect,
            egui::Id::new(("color-picker-preview", label)),
            egui::Sense::click(),
        );
        ui.painter().rect_filled(rect, 2.0, to_color32(color));
        ui.painter().rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, chrome.colors.panel_border32()),
            egui::StrokeKind::Inside,
        );
        response
    })
    .inner
}

/// The overlapping FOREGROUND/BACKGROUND swatch pair: the primary square at the
/// top-left and the secondary square offset down-right behind it.
///
/// Returns the BACKGROUND square's click response: clicking it swaps the
/// primary and secondary colors. The FOREGROUND square keeps its inert hover
/// behaviour and sits on top, so a click in the overlap never swaps.
fn fg_bg_swatch_pair(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    primary: Color,
    secondary: Color,
) -> egui::Response {
    let side = FG_BG_SWATCH_SIZE;
    let offset = FG_BG_SWATCH_OFFSET;
    let (area, _) = ui.allocate_exact_size(
        egui::vec2(side + offset, side + offset),
        egui::Sense::hover(),
    );
    let bg_rect = egui::Rect::from_min_size(
        area.min + egui::vec2(offset, offset),
        egui::vec2(side, side),
    );
    let fg_rect = egui::Rect::from_min_size(area.min, egui::vec2(side, side));
    let bg_response = ui.interact(bg_rect, egui::Id::new(BG_SWATCH_ID), egui::Sense::click());
    ui.interact(fg_rect, egui::Id::new(FG_SWATCH_ID), egui::Sense::click());
    let border = egui::Stroke::new(1.0, chrome.colors.panel_border32());
    for (rect, color) in [(bg_rect, secondary), (fg_rect, primary)] {
        ui.painter().rect_filled(rect, 2.0, to_color32(color));
        ui.painter()
            .rect_stroke(rect, 2.0, border, egui::StrokeKind::Inside);
    }
    bg_response
}

/// The swap button: swaps the session's primary and secondary colors.
fn swap_button(ui: &mut egui::Ui, chrome: &PanelChrome) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(SWAP_BUTTON_WIDTH, SWAP_BUTTON_HEIGHT),
        egui::Sense::hover(),
    );
    let response = ui.interact(rect, egui::Id::new(SWAP_BUTTON_ID), egui::Sense::click());
    let state = if response.is_pointer_button_down_on() {
        SkinState::Pressed
    } else if response.hovered() {
        SkinState::Hover
    } else {
        SkinState::Normal
    };
    if !chrome.paint_slice(ui.painter(), "button", state, rect) {
        ui.painter()
            .rect_filled(rect, 2.0, chrome.colors.panel_header_bg32());
        ui.painter().rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, chrome.colors.panel_border32()),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Swap",
        egui::FontId::proportional(11.0),
        ui.visuals().text_color(),
    );
    response
}

/// One labeled chip row: small separated squares, the selected chip (equal to
/// the current color) outlined in the theme's selection stroke. Rows wider
/// than the column wrap onto a second line.
fn chip_row(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    host: &ColorPickerHost,
    label: &str,
    chips: &[Color],
    current: Color,
    index: &mut usize,
) {
    ui.weak(label);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = CHIP_GAP;
        for chip in chips {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(CHIP_SIZE, CHIP_SIZE), egui::Sense::hover());
            let response = ui.interact(
                rect,
                egui::Id::new(("color-picker-chip", *index)),
                egui::Sense::click(),
            );
            ui.painter().rect_filled(rect, 1.0, to_color32(*chip));
            ui.painter().rect_stroke(
                rect,
                1.0,
                egui::Stroke::new(1.0, chrome.colors.panel_border32()),
                egui::StrokeKind::Inside,
            );
            if *chip == current {
                ui.painter().rect_stroke(
                    rect,
                    1.0,
                    egui::Stroke::new(1.5, chrome.colors.selection_stroke_color32()),
                    egui::StrokeKind::Inside,
                );
            }
            if response.clicked() {
                host.events
                    .borrow_mut()
                    .push(ToolbarEvent::ColorChanged(*chip));
            }
            *index += 1;
        }
    });
}

/// Panel content: the static nest holding the three columns.
struct ColorPickerContent {
    nest: NestContent,
}

impl PanelContent for ColorPickerContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the three-column color picker body.
pub fn build_color_picker_nest(host: &ColorPickerHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(ColorPickerComponent::new(host.clone()));
    nest
}

/// Dock spec for the Color Picker panel (dock id `PanelId::new(104)`).
///
/// `can_pop_out == false` (and `can_native_pop_out == false`), so the header
/// shows no Float/Dock button: the panel floats but is not dockable.
/// `resizable == false`, so the floating window is fixed-size and only
/// draggable by its header. The height is normalized to the content so the
/// layout ends snugly under the FG/BG pair.
pub fn color_picker_panel_spec(host: &ColorPickerHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(COLOR_PICKER_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(
                "Color Picker",
                false,
                egui::vec2(MIN_PANEL_WIDTH, PICKER_WINDOW_HEIGHT),
            )
        },
        placement: PanelPlacement::Floating,
        floating_rect: egui::Rect::from_min_size(
            floating_rect.min,
            egui::vec2(floating_rect.width(), PICKER_WINDOW_HEIGHT),
        ),
        content: Box::new(ColorPickerContent {
            nest: build_color_picker_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shading_row_applies_the_hue_shift_map() {
        // Blue: the dark side keeps the map (toward red, +120 over four
        // steps); the bright side walks toward yellow (+120, capped at the
        // dark shift magnitude), so the row runs red/pink/magenta/violet/base/
        // purple/magenta/dark pink/dark red.
        let blue = Color::rgb(0, 0, 255);
        assert_eq!(shading_hue_shift(240.0), 120.0);
        assert_eq!(bright_shading_hue_shift(240.0), 120.0);
        assert_eq!(
            shading_chips(blue),
            [
                Color::rgb(255, 0, 0),
                Color::rgb(255, 0, 128),
                Color::rgb(255, 0, 255),
                Color::rgb(128, 0, 255),
                Color::rgb(0, 0, 255),
                Color::rgb(102, 0, 204),
                Color::rgb(153, 0, 153),
                Color::rgb(102, 0, 51),
                Color::rgb(51, 0, 0),
            ],
            "blue must brighten toward yellow and darken toward red"
        );

        // Orange (hue 40) sits between red (-120) and yellow (-60): the
        // interpolated dark shift is -80, the bright side walks +20 to yellow.
        let orange = Color::rgb(255, 170, 0);
        let (orange_hue, _, _) = color_to_hsv(orange);
        assert!(
            (shading_hue_shift(orange_hue) - (-80.0)).abs() < 0.001,
            "orange must interpolate the red/yellow shifts: {}",
            shading_hue_shift(orange_hue)
        );
        assert!((bright_shading_hue_shift(orange_hue) - 20.0).abs() < 0.001);
        let chips = shading_chips(orange);
        assert_eq!(chips[SHADING_BRIGHT_STEPS], orange);
        let (bright_hue, _, _) = color_to_hsv(chips[0]);
        assert!(
            (bright_hue - 60.0).abs() < 0.001,
            "the brightest orange chip must reach yellow: {bright_hue}"
        );
        assert_eq!(
            chips[SHADING_CHIP_COUNT - 1],
            Color::rgb(51, 0, 34),
            "the darkest orange chip keeps the dark ramp"
        );
    }

    #[test]
    fn lightness_saturation_hue_rows_change_only_their_axis() {
        let base = Color::rgb(51, 102, 204);
        let (base_hue, base_saturation, base_lightness) = color_to_hsl(base);

        let lightness = lightness_chips(base);
        assert_eq!(lightness.len(), LIGHTNESS_CHIP_COUNT);
        let mut previous = f32::INFINITY;
        for chip in lightness {
            let (hue, saturation, value) = color_to_hsl(chip);
            assert!((hue - base_hue).abs() < 1.0, "hue drifted: {hue}");
            assert!(
                (saturation - base_saturation).abs() < 0.01,
                "saturation drifted: {saturation}"
            );
            assert!(value < previous, "the lightness ramp must darken: {value}");
            previous = value;
        }
        let (_, _, first) = color_to_hsl(lightness[0]);
        let (_, _, last) = color_to_hsl(lightness[LIGHTNESS_CHIP_COUNT - 1]);
        assert!((first - 0.9).abs() < 0.01 && (last - 0.1).abs() < 0.01);

        let saturation = saturation_chips(base);
        assert_eq!(saturation.len(), 7);
        for (index, chip) in saturation.iter().enumerate() {
            let (hue, chip_saturation, value) = color_to_hsl(*chip);
            assert!(
                (value - base_lightness).abs() < 0.01,
                "lightness drifted: {value}"
            );
            let expected = 1.0 - index as f32 / 6.0;
            assert!(
                (chip_saturation - expected).abs() < 0.01,
                "saturation must follow the ladder: {chip_saturation} vs {expected}"
            );
            if chip_saturation > 0.01 {
                assert!((hue - base_hue).abs() < 1.0, "hue drifted: {hue}");
            }
        }

        let hue = hue_chips(base);
        assert_eq!(hue.len(), 7);
        for (index, chip) in hue.iter().enumerate() {
            let (chip_hue, chip_saturation, chip_lightness) = color_to_hsl(*chip);
            let expected = (base_hue + index as f32 * (360.0 / 7.0)).rem_euclid(360.0);
            let delta = (chip_hue - expected)
                .abs()
                .min(360.0 - (chip_hue - expected).abs());
            assert!(
                delta < 1.0,
                "hue rotation drifted: {chip_hue} vs {expected}"
            );
            assert!(
                (chip_saturation - base_saturation).abs() < 0.02,
                "hue chip must keep saturation: {chip_saturation}"
            );
            assert!(
                (chip_lightness - base_lightness).abs() < 0.02,
                "hue chip must keep lightness: {chip_lightness}"
            );
        }
    }

    #[test]
    fn hsv_and_hsl_round_trip_through_each_other() {
        for color in [
            Color::rgb(0, 0, 255),
            Color::rgb(255, 170, 0),
            Color::rgb(51, 102, 204),
            Color::rgb(10, 20, 30),
            Color::rgb(255, 255, 255),
            Color::rgb(0, 0, 0),
        ] {
            let (hue, saturation, value) = color_to_hsv(color);
            assert_eq!(hsv_to_color(hue, saturation, value, color.a), color);
            let (hue, saturation, lightness) = color_to_hsl(color);
            assert_eq!(hsl_to_color(hue, saturation, lightness, color.a), color);
        }
    }

    #[test]
    fn hex_round_trips_and_rejects_malformed_input() {
        let color = Color::rgb(17, 34, 51);
        assert_eq!(format_hex(color), "#112233");
        assert_eq!(parse_hex("#112233"), Some(color));
        assert_eq!(parse_hex("112233"), Some(color));
        assert_eq!(parse_hex("#11223"), None);
        assert_eq!(parse_hex("#GGGGGG"), None);
    }
}
