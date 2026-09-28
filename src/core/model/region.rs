//! Sprite-sheet region: a named rectangular window into the sheet.
//!
//! The canvas is the sprite sheet; a [`Region`] is a pure rect reference
//! into it — it never owns or copies pixels (R3 "pencere yöntemi" /
//! rect-reference design).

use crate::core::math::Rect2i;

/// A named rectangular window into the sprite sheet.
///
/// A [`Region`] stores only a [`Rect2i`] and a name; it is a reference into
/// the sheet, never a pixel copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    pub rect: Rect2i,
    pub name: String,
}

impl Region {
    pub fn new(rect: Rect2i, name: impl Into<String>) -> Self {
        Self {
            rect,
            name: name.into(),
        }
    }

    pub fn rect(&self) -> Rect2i {
        self.rect
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Moves the window by `(dx, dy)` in sheet space.
    pub fn translate(&mut self, dx: i32, dy: i32) {
        self.rect = self.rect.translate(dx, dy);
    }

    /// Whether the sheet-space point `(x, y)` lies inside the window.
    /// Edges are exclusive: `x`/`y` must be `< right`/`< bottom`.
    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        self.rect.contains(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rect() -> Rect2i {
        Rect2i::new(10, 20, 30, 40)
    }

    #[test]
    fn construction_stores_rect_and_name() {
        let region = Region::new(sample_rect(), "idle");
        assert_eq!(region.rect(), sample_rect());
        assert_eq!(region.name(), "idle");
        assert_eq!(region.rect, sample_rect());
        assert_eq!(region.name, "idle");
    }

    #[test]
    fn set_name_updates_name() {
        let mut region = Region::new(sample_rect(), "idle");
        region.set_name("run");
        assert_eq!(region.name(), "run");
    }

    #[test]
    fn translate_moves_rect_and_updates_containment() {
        let mut region = Region::new(Rect2i::new(0, 0, 10, 10), "box");
        assert!(region.contains_point(5, 5));
        region.translate(10, 20);
        assert_eq!(region.rect(), Rect2i::new(10, 20, 10, 10));
        assert!(!region.contains_point(5, 5));
        assert!(region.contains_point(15, 25));
    }

    #[test]
    fn contains_point_true_inside() {
        let region = Region::new(sample_rect(), "r");
        assert!(region.contains_point(10, 20));
        assert!(region.contains_point(39, 59));
    }

    #[test]
    fn contains_point_false_outside_edges() {
        let region = Region::new(sample_rect(), "r");
        assert!(!region.contains_point(9, 20));
        assert!(!region.contains_point(40, 20));
        assert!(!region.contains_point(10, 19));
        assert!(!region.contains_point(10, 60));
    }

    #[test]
    fn zero_size_rect_contains_nothing() {
        let region = Region::new(Rect2i::ZERO, "empty");
        assert!(!region.contains_point(0, 0));
        let region = Region::new(Rect2i::new(5, 5, 0, 10), "empty");
        assert!(!region.contains_point(5, 5));
    }

    #[test]
    fn equality() {
        let a = Region::new(sample_rect(), "idle");
        let b = Region::new(sample_rect(), "idle");
        let c = Region::new(sample_rect(), "run");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
