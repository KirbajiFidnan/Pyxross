//! RGBA8 pixel color type.

/// Straight-alpha RGBA8 color. Layout: `[r, g, b, a]`, one byte each.
/// `Pod`/`Zeroable` so buffers map directly onto wgpu `write_texture` payloads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }

    /// `0xRRGGBBAA`.
    pub const fn from_packed(packed: u32) -> Self {
        Self::rgba(
            (packed >> 24) as u8,
            (packed >> 16) as u8,
            (packed >> 8) as u8,
            packed as u8,
        )
    }

    pub const fn to_packed(self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16) | ((self.b as u32) << 8) | self.a as u32
    }

    pub const fn is_opaque(self) -> bool {
        self.a == 255
    }

    pub const fn is_transparent(self) -> bool {
        self.a == 0
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl From<[u8; 4]> for Color {
    fn from(rgba: [u8; 4]) -> Self {
        Self::rgba(rgba[0], rgba[1], rgba[2], rgba[3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_roundtrip() {
        let c = Color::rgba(0x12, 0x34, 0x56, 0x78);
        assert_eq!(Color::from_packed(c.to_packed()), c);
        assert_eq!(
            Color::from_packed(0x00112233),
            Color::rgba(0x00, 0x11, 0x22, 0x33)
        );
    }

    #[test]
    fn consts() {
        assert_eq!(Color::default(), Color::TRANSPARENT);
        assert!(Color::TRANSPARENT.is_transparent());
        assert!(Color::BLACK.is_opaque());
        assert!(Color::WHITE.is_opaque());
    }

    #[test]
    fn pod_layout_matches_rgba_bytes() {
        let c = Color::rgba(1, 2, 3, 4);
        let bytes: [u8; 4] = bytemuck::cast(c);
        assert_eq!(bytes, [1, 2, 3, 4]);
    }
}
