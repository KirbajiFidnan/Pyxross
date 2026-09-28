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
