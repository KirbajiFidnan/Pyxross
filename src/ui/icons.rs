//! NerdFont glyph constants and font registration for the UI.

use std::sync::{Arc, OnceLock};

use egui::{FontData, FontDefinitions, FontFamily};

/// Embedded NerdFont bytes used as fallback so Material Design / NerdFont
/// glyphs always render without falling back to missing-glyph boxes.
const NERDFONT_BYTES: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMonoNLNerdFont-Regular.ttf");

/// Shared `FontDefinitions` built once per process so tests and windows reuse
/// the same parsed font payload without duplicating 2.4 MB per `Context`.
fn shared_font_definitions() -> &'static FontDefinitions {
    static FONTS: OnceLock<FontDefinitions> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fonts = FontDefinitions::default();
        fonts.font_data.insert(
            "nerdfont".to_owned(),
            Arc::new(FontData::from_static(NERDFONT_BYTES)),
        );
        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.push("nerdfont".to_owned());
        }
        if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
            family.push("nerdfont".to_owned());
        }
        fonts
    })
}

/// Registers the embedded NerdFont fallback on `ctx`.
pub fn configure_fonts(ctx: &egui::Context) {
    ctx.set_fonts(shared_font_definitions().clone());
}

// Tool icons (Material Design / NerdFont PUA)
pub const PENCIL: &str = "\u{f03eb}";
pub const ERASER: &str = "\u{f01fe}";
pub const SELECTION: &str = "\u{f0486}";
pub const FILL: &str = "\u{f0266}";
pub const EYEDROPPER: &str = "\u{f020a}";
pub const TILE: &str = "\u{f0570}";
pub const ANIMATION: &str = "\u{f022e}";
pub const TEXT: &str = "\u{f0284}";
pub const ALIGN_LEFT: &str = "\u{f0262}";
pub const ALIGN_CENTER: &str = "\u{f0260}";
pub const ALIGN_RIGHT: &str = "\u{f0263}";
pub const ONION_PREV: &str = "\u{f0f63}";
pub const ONION_NEXT: &str = "\u{f0f61}";

// Chrome & panel action icons
pub const SETTINGS: &str = "\u{f0493}";
pub const PLAY: &str = "\u{f040a}";
pub const PAUSE: &str = "\u{f03e4}";
pub const PREV: &str = "\u{f04ae}";
pub const NEXT: &str = "\u{f04ad}";
pub const LOOP: &str = "\u{f0456}";
pub const PLUS: &str = "\u{f0415}";
pub const TRASH: &str = "\u{f01b4}";
pub const CLOSE: &str = "\u{f0156}";
pub const EYE_OPEN: &str = "\u{f0208}";
pub const EYE_CLOSED: &str = "\u{f0209}";
pub const LOCK: &str = "\u{f033e}";
pub const UNLOCK: &str = "\u{f033f}";
pub const FOLDER: &str = "\u{f024b}";
