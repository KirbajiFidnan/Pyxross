//! Integer math types.

/// Axis-aligned integer rectangle: position + size.
/// Right/bottom edges are EXCLUSIVE: `right == x + w`, `bottom == y + h`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect2i {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect2i {
    pub const ZERO: Rect2i = Rect2i::new(0, 0, 0, 0);

    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn from_pos_size(pos: (i32, i32), size: (i32, i32)) -> Self {
        Self::new(pos.0, pos.1, size.0, size.1)
    }

    pub const fn right(self) -> i32 {
        self.x.saturating_add(self.w)
    }

    pub const fn bottom(self) -> i32 {
        self.y.saturating_add(self.h)
    }

    pub const fn is_empty(self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub const fn contains(self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    pub fn contains_rect(self, other: Rect2i) -> bool {
        self.x <= other.x
            && self.y <= other.y
            && self.right() >= other.right()
            && self.bottom() >= other.bottom()
    }

    pub fn intersects(self, other: Rect2i) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }

    pub fn intersection(self, other: Rect2i) -> Rect2i {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Rect2i::new(x, y, right - x, bottom - y)
    }

    pub fn union(self, other: Rect2i) -> Rect2i {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect2i::new(x, y, right - x, bottom - y)
    }

    pub const fn translate(self, dx: i32, dy: i32) -> Rect2i {
        Rect2i::new(
            self.x.saturating_add(dx),
            self.y.saturating_add(dy),
            self.w,
            self.h,
        )
    }

    /// Shrink-wrap to `bounds`: keeps the intersection, empty if none.
    pub fn clamp_to(self, bounds: Rect2i) -> Rect2i {
        self.intersection(bounds)
    }

    /// Area in pixels (i64: protects against i32 overflow on huge canvases).
    pub const fn area(self) -> i64 {
        self.w as i64 * self.h as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect2i = Rect2i::new(10, 20, 30, 40);

    #[test]
    fn edges_are_exclusive() {
        assert!(R.contains(10, 20));
        assert!(R.contains(39, 59));
        assert!(!R.contains(40, 60));
        assert!(!R.contains(9, 59));
    }

    #[test]
    fn empty_rect() {
        assert!(Rect2i::ZERO.is_empty());
        assert!(Rect2i::new(0, 0, 0, 40).is_empty());
        assert!(Rect2i::new(0, 0, 40, -1).is_empty());
        assert!(!R.is_empty());
    }

    #[test]
    fn containment() {
        let inner = Rect2i::new(15, 25, 10, 10);
        assert!(R.contains_rect(inner));
        assert!(!R.contains_rect(Rect2i::new(15, 25, 30, 10)));
    }

    #[test]
    fn intersection_semantics() {
        let overlapping = Rect2i::new(30, 30, 40, 40);
        assert!(R.intersects(overlapping));
        assert_eq!(R.intersection(overlapping), Rect2i::new(30, 30, 10, 30));
        let touching = Rect2i::new(40, 20, 5, 5);
        assert!(!R.intersects(touching));
        assert!(R.intersection(touching).is_empty());
        assert_eq!(
            R.clamp_to(Rect2i::new(0, 0, 20, 30)),
            Rect2i::new(10, 20, 10, 10)
        );
        assert!(R.clamp_to(Rect2i::new(100, 100, 5, 5)).is_empty());
    }

    #[test]
    fn union_and_translate() {
        assert_eq!(
            Rect2i::new(0, 0, 10, 10).union(Rect2i::new(5, 5, 10, 10)),
            Rect2i::new(0, 0, 15, 15)
        );
        assert_eq!(R.translate(-10, 10), Rect2i::new(0, 30, 30, 40));
    }

    #[test]
    fn area() {
        assert_eq!(R.area(), 30 * 40);
        assert_eq!(Rect2i::ZERO.area(), 0);
        assert_eq!(Rect2i::new(0, 0, 4096, 4096).area(), 4096i64 * 4096);
    }

    #[test]
    fn edge_arithmetic_saturates_instead_of_wrapping() {
        let max = Rect2i::new(i32::MAX, i32::MAX, 1, 1);
        assert_eq!(max.right(), i32::MAX);
        assert_eq!(max.bottom(), i32::MAX);
        assert_eq!(max.translate(1, 1).x, i32::MAX);
    }
}
