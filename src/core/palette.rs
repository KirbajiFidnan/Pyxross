use serde::{Deserialize, Serialize};

use super::color::Color;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    pub version: u32,
    pub colors: Vec<[u8; 4]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteError {
    EmptyName,
    UnsupportedVersion(u32),
}

impl std::fmt::Display for PaletteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(f, "palette name must not be empty"),
            Self::UnsupportedVersion(version) => write!(f, "unsupported palette version {version}"),
        }
    }
}

impl std::error::Error for PaletteError {}

impl Palette {
    pub const CURRENT_VERSION: u32 = 1;

    pub fn new(name: impl Into<String>, colors: Vec<[u8; 4]>) -> Result<Self, PaletteError> {
        let palette = Self {
            name: name.into(),
            version: Self::CURRENT_VERSION,
            colors,
        };
        palette.validate()?;
        Ok(palette)
    }

    pub fn try_new(name: impl Into<String>, colors: Vec<[u8; 4]>) -> Result<Self, PaletteError> {
        Self::new(name, colors)
    }

    pub fn validate(&self) -> Result<(), PaletteError> {
        if self.name.trim().is_empty() {
            return Err(PaletteError::EmptyName);
        }
        if self.version != Self::CURRENT_VERSION {
            return Err(PaletteError::UnsupportedVersion(self.version));
        }
        Ok(())
    }

    pub fn color(&self, index: usize) -> Option<Color> {
        self.colors.get(index).copied().map(Color::from)
    }

    pub fn select(&self, index: usize) -> Option<Color> {
        self.color(index)
    }

    pub fn entries(&self) -> &[[u8; 4]] {
        &self.colors
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            name: "Default".to_string(),
            version: Self::CURRENT_VERSION,
            colors: vec![
                [0, 0, 0, 255],
                [255, 255, 255, 255],
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 0, 255],
                [255, 0, 255, 255],
                [0, 255, 255, 255],
            ],
        }
    }
}

/// Curated copyright-free built-in pixel-art palettes selectable from the
/// top bar's `Palette` menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinPaletteKind {
    /// Pyxross signature 32-color palette: 16 main colors + 16 harmonious shades.
    Ross,
    /// PICO-8 16-color palette.
    Pico8,
    /// GrafxKid's Sweetie 16 (CC0 / Public Domain).
    Sweetie16,
    /// DawnBringer 16.
    Db16,
    /// DawnBringer 32.
    Db32,
    /// Endesga 32.
    Endesga32,
    /// Adigun A. Polack's AAP-64.
    Aap64,
}

fn hex_rgba(hex: u32) -> [u8; 4] {
    [
        ((hex >> 16) & 0xFF) as u8,
        ((hex >> 8) & 0xFF) as u8,
        (hex & 0xFF) as u8,
        255,
    ]
}

/// Builds one of the curated built-in palettes.
pub fn builtin_palette(kind: BuiltinPaletteKind) -> Palette {
    let (name, hexes): (&str, &[u32]) = match kind {
        // Ross (32): 8 harmonious hue-shifted 4-step ramps (light -> mid -> shade -> deep)
        // ordered smoothly from neutrals through warm earth/red/gold to green/teal/blue/violet.
        BuiltinPaletteKind::Ross => (
            "Ross",
            &[
                // Neutrals (cream -> silver -> slate -> ink)
                0xF4EFE6, 0xC2C6CF, 0x6C7384, 0x1E1F29,
                // Warm skin & earth (peach -> terracotta -> rust -> deep umber)
                0xF7C59F, 0xC97B5D, 0x8F4A37, 0x4A2521,
                // Red & crimson (coral -> scarlet -> wine -> dark plum)
                0xF05A5B, 0xD93848, 0x962338, 0x521428,
                // Gold & amber (sun yellow -> honey -> burnt orange -> dark amber)
                0xF6D55C, 0xEDA03B, 0xD96B27, 0x8C3B1E,
                // Green & foliage (lime -> grass -> forest -> deep pine)
                0xA6D45E, 0x57B364, 0x2D7A4F, 0x19473A,
                // Teal & ocean (aqua -> seafoam -> petrol -> deep ocean)
                0x6EE0D2, 0x32B5B8, 0x1D7588, 0x154159,
                // Sky & cobalt (ice blue -> sky -> royal -> midnight)
                0x7CC4FA, 0x4589E6, 0x2D53B8, 0x1E2A6E,
                // Rose & violet (pink -> orchid -> violet -> deep eggplant)
                0xEB75B0, 0xA85CC9, 0x68389C, 0x3A1D5C,
            ],
        ),
        BuiltinPaletteKind::Pico8 => (
            "PICO-8",
            &[
                0x000000, 0x1D2B53, 0x7E2553, 0x008751,
                0xAB5236, 0x5F574F, 0xC2C3C7, 0xFFF1E8,
                0xFF004D, 0xFFA300, 0xFFEC27, 0x00E436,
                0x29ADFF, 0x83769C, 0xFF77A8, 0xFFCCAA,
            ],
        ),
        BuiltinPaletteKind::Sweetie16 => (
            "Sweetie 16",
            &[
                0x1A1C2C, 0x5D275D, 0xB13E53, 0xEF7D57,
                0xFFCD75, 0xA7F070, 0x38B764, 0x257179,
                0x29366F, 0x3B5DC9, 0x41A6F6, 0x73EFF7,
                0xF4F4F4, 0x94B0C2, 0x566C86, 0x333C57,
            ],
        ),
        BuiltinPaletteKind::Db16 => (
            "DB16",
            &[
                0x140C1C, 0x442434, 0x30346D, 0x4E4A4E,
                0x854C30, 0x346524, 0xD04648, 0x757161,
                0x597DCE, 0xD27D2C, 0x8595A1, 0x6DAA2C,
                0xD2AA99, 0x6DC2CA, 0xDAD45E, 0xDEEED6,
            ],
        ),
        BuiltinPaletteKind::Db32 => (
            "DB32",
            &[
                0x000000, 0x222034, 0x45283C, 0x663931,
                0x8F563B, 0xDF7126, 0xD9A066, 0xEEC39A,
                0xFBF236, 0x99E550, 0x6ABE30, 0x37946E,
                0x4B692F, 0x524B24, 0x323C39, 0x3F3F74,
                0x306082, 0x5B6EE1, 0x639BFF, 0x5FCDE4,
                0xCBDBFC, 0xFFFFFF, 0x9BADB7, 0x847E87,
                0x696A6A, 0x595652, 0x76428A, 0xAC3232,
                0xD95763, 0xD77BBA, 0x8F974A, 0x8A6F30,
            ],
        ),
        BuiltinPaletteKind::Endesga32 => (
            "Endesga 32",
            &[
                0xBE4A2F, 0xD77643, 0xEAD4AA, 0xE4A672,
                0xB86F50, 0x733E39, 0x3E2731, 0xA22633,
                0xE43B44, 0xF77622, 0xFEAE34, 0xFEE761,
                0x63C74D, 0x3E8948, 0x265C42, 0x193C3E,
                0x124E89, 0x0099DB, 0x2CE8F5, 0xFFFFFF,
                0xC0CBDC, 0x8B9BB4, 0x5A6988, 0x3A4466,
                0x262B44, 0x181425, 0xFF0044, 0x68386C,
                0xB55088, 0xF6757A, 0xE8B796, 0xC28569,
            ],
        ),
        BuiltinPaletteKind::Aap64 => (
            "AAP-64",
            &[
                0x060608, 0x141013, 0x3B1725, 0x73172D,
                0xB4202A, 0xDF3E23, 0xFA6A0A, 0xF9A31B,
                0xFFD541, 0xFFFC40, 0xD6F264, 0x9CDB43,
                0x59C135, 0x14A02E, 0x1A7A3E, 0x24523B,
                0x122020, 0x143464, 0x285CC4, 0x249FDE,
                0x20D6C7, 0xA6FCDB, 0xFFFFFF, 0xFEF3C0,
                0xFAD6B8, 0xF5A097, 0xE86A73, 0xBC4A9B,
                0x793A80, 0x403353, 0x242234, 0x221C1A,
                0x322B28, 0x71413B, 0xBB7547, 0xDBA463,
                0xF4D29C, 0xDAE0EA, 0xB3B9D1, 0x8B93AF,
                0x6D758D, 0x4A5462, 0x333941, 0x422433,
                0x5B3138, 0x8E5252, 0xBA756A, 0xE9B5A3,
                0xE3E6FF, 0xB9BFFB, 0x849BE4, 0x588DBE,
                0x477D85, 0x23674E, 0x328464, 0x5DAF8D,
                0x92DCBA, 0xCDF7E2, 0xE4D2AA, 0xC7B08B,
                0xA08662, 0x796755, 0x5A4E44, 0x423934,
            ],
        ),
    };
    Palette {
        name: name.to_owned(),
        version: Palette::CURRENT_VERSION,
        colors: hexes.iter().copied().map(hex_rgba).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_has_expected_colors() {
        let palette = Palette::default();
        assert_eq!(palette.name, "Default");
        assert_eq!(palette.color(0), Some(Color::BLACK));
        assert_eq!(palette.color(1), Some(Color::WHITE));
    }

    #[test]
    fn json_roundtrip_preserves_palette() {
        let palette = Palette::new("Test", vec![[1, 2, 3, 4]]).unwrap();
        let json = serde_json::to_string(&palette).unwrap();
        assert_eq!(serde_json::from_str::<Palette>(&json).unwrap(), palette);
    }

    #[test]
    fn malformed_palette_is_rejected() {
        let empty = Palette::new(" ", vec![]).unwrap_err();
        assert_eq!(empty, PaletteError::EmptyName);
        let invalid = Palette {
            name: "x".to_string(),
            version: 2,
            colors: vec![],
        };
        assert_eq!(invalid.validate(), Err(PaletteError::UnsupportedVersion(2)));
    }

    #[test]
    fn selecting_color_does_not_mutate_palette_pixels() {
        let palette = Palette::new("Test", vec![[1, 2, 3, 4]]).unwrap();
        let before = palette.clone();
        assert_eq!(palette.color(0), Some(Color::rgba(1, 2, 3, 4)));
        assert_eq!(palette, before);
    }
}
