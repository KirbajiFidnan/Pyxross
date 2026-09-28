//! Animation frame: a sprite-sheet region plus a display delay.

use super::region::Region;

/// Maximum frame delay in milliseconds (60 s).
const MAX_DELAY_MS: u32 = 60_000;

/// A single animation frame: a [`Region`] window into the sprite sheet plus
/// the delay in milliseconds before advancing to the next frame.
///
/// The frame references the sheet by rect — it never copies pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub region: Region,
    pub delay_ms: u32,
}

impl Frame {
    /// Creates a frame; `delay_ms` is clamped to `0..=60_000`.
    pub fn new(region: Region, delay_ms: u32) -> Self {
        Self {
            region,
            delay_ms: delay_ms.min(MAX_DELAY_MS),
        }
    }

    pub fn region(&self) -> &Region {
        &self.region
    }

    pub fn delay_ms(&self) -> u32 {
        self.delay_ms
    }

    /// Sets the delay, clamped to `0..=60_000`.
    pub fn set_delay_ms(&mut self, ms: u32) {
        self.delay_ms = ms.min(MAX_DELAY_MS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math::Rect2i;

    fn sample_region() -> Region {
        Region::new(Rect2i::new(0, 0, 16, 16), "frame-0")
    }

    #[test]
    fn construction_stores_region_and_delay() {
        let frame = Frame::new(sample_region(), 100);
        assert_eq!(frame.region(), &sample_region());
        assert_eq!(frame.delay_ms(), 100);
        assert_eq!(frame.region.rect, Rect2i::new(0, 0, 16, 16));
        assert_eq!(frame.region.name, "frame-0");
    }

    #[test]
    fn new_clamps_delay_to_max_and_keeps_zero() {
        assert_eq!(Frame::new(sample_region(), 5_000_000).delay_ms(), 60_000);
        assert_eq!(Frame::new(sample_region(), 60_000).delay_ms(), 60_000);
        assert_eq!(Frame::new(sample_region(), 0).delay_ms(), 0);
    }

    #[test]
    fn set_delay_ms_clamps_both_directions() {
        let mut frame = Frame::new(sample_region(), 100);
        frame.set_delay_ms(5_000_000);
        assert_eq!(frame.delay_ms(), 60_000);
        frame.set_delay_ms(0);
        assert_eq!(frame.delay_ms(), 0);
        frame.set_delay_ms(250);
        assert_eq!(frame.delay_ms(), 250);
    }

    #[test]
    fn region_accessor_returns_region() {
        let region = sample_region();
        let frame = Frame::new(region.clone(), 50);
        assert_eq!(frame.region(), &region);
        assert_eq!(frame.region().name(), "frame-0");
    }

    #[test]
    fn equality() {
        let a = Frame::new(sample_region(), 100);
        let b = Frame::new(sample_region(), 100);
        let c = Frame::new(sample_region(), 200);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
