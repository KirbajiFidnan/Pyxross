//! Onion-skinning configuration.

use crate::core::color::Color;

/// Onion-skinning configuration with Aseprite-style defaults.
///
/// Shows `prev_frames` ghosted frames before and `next_frames` after the
/// current frame, tinted `prev_tint` / `next_tint` respectively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OnionConfig {
    prev_frames: u8,
    next_frames: u8,
    prev_tint: Color,
    next_tint: Color,
}

impl OnionConfig {
    /// Aseprite defaults: 1 previous + 1 next frame, red prev tint,
    /// green next tint.
    pub fn new() -> Self {
        Self {
            prev_frames: 1,
            next_frames: 1,
            prev_tint: Color::rgb(255, 80, 80),
            next_tint: Color::rgb(80, 255, 80),
        }
    }

    pub fn prev_frames(&self) -> u8 {
        self.prev_frames
    }

    pub fn next_frames(&self) -> u8 {
        self.next_frames
    }

    pub fn prev_tint(&self) -> Color {
        self.prev_tint
    }

    pub fn next_tint(&self) -> Color {
        self.next_tint
    }

    /// Sets the number of previous frames shown, clamped to `0..=3`.
    pub fn set_prev_frames(&mut self, n: u8) {
        self.prev_frames = n.min(3);
    }

    /// Sets the number of next frames shown, clamped to `0..=3`.
    pub fn set_next_frames(&mut self, n: u8) {
        self.next_frames = n.min(3);
    }

    pub fn set_prev_tint(&mut self, tint: Color) {
        self.prev_tint = tint;
    }

    pub fn set_next_tint(&mut self, tint: Color) {
        self.next_tint = tint;
    }

    /// True when at least one previous or next frame is shown.
    pub fn has_onion(&self) -> bool {
        self.prev_frames > 0 || self.next_frames > 0
    }
}

impl Default for OnionConfig {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onion_defaults_aseprite() {
        let cfg = OnionConfig::new();
        assert_eq!(cfg.prev_frames(), 1);
        assert_eq!(cfg.next_frames(), 1);
        assert_eq!(cfg.prev_tint(), Color::rgb(255, 80, 80));
        assert_eq!(cfg.next_tint(), Color::rgb(80, 255, 80));
        assert!(cfg.has_onion());
    }

    #[test]
    fn onion_default_equals_new() {
        assert_eq!(OnionConfig::default(), OnionConfig::new());
    }

    #[test]
    fn onion_clamps_prev_frames_to_three() {
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(4);
        assert_eq!(cfg.prev_frames(), 3);
        cfg.set_prev_frames(255);
        assert_eq!(cfg.prev_frames(), 3);
        cfg.set_prev_frames(0);
        assert_eq!(cfg.prev_frames(), 0);
    }

    #[test]
    fn onion_clamps_next_frames_to_three() {
        let mut cfg = OnionConfig::new();
        cfg.set_next_frames(4);
        assert_eq!(cfg.next_frames(), 3);
        cfg.set_next_frames(0);
        assert_eq!(cfg.next_frames(), 0);
    }

    #[test]
    fn onion_has_onion_false_when_both_zero() {
        let mut cfg = OnionConfig::new();
        cfg.set_prev_frames(0);
        cfg.set_next_frames(0);
        assert!(!cfg.has_onion());
        cfg.set_prev_frames(1);
        assert!(cfg.has_onion(), "prev alone enables onion");
    }

    #[test]
    fn onion_tint_setters_roundtrip() {
        let mut cfg = OnionConfig::new();
        cfg.set_prev_tint(Color::rgb(10, 20, 30));
        cfg.set_next_tint(Color::rgb(40, 50, 60));
        assert_eq!(cfg.prev_tint(), Color::rgb(10, 20, 30));
        assert_eq!(cfg.next_tint(), Color::rgb(40, 50, 60));
    }

    #[test]
    fn onion_copy_semantics() {
        let cfg = OnionConfig::new();
        let copy = cfg;
        let mut original = cfg;
        original.set_prev_frames(3);
        original.set_next_tint(Color::rgb(1, 2, 3));
        assert_eq!(copy.prev_frames(), 1);
        assert_eq!(copy.next_frames(), 1);
        assert_eq!(copy.next_tint(), Color::rgb(80, 255, 80));
        assert_eq!(original.prev_frames(), 3);
    }
}
