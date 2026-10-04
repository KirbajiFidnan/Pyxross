//! Mask-aware pixel clipping.

use crate::core::math::Rect2i;
use crate::core::select::Selection;

/// A pixel region that drawing operations may write.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelClip {
    rect: Rect2i,
    mask: Option<std::sync::Arc<[bool]>>,
}

impl PixelClip {
    /// Creates a clip from a selection's shape.
    pub fn from_selection(selection: &Selection) -> Self {
        let mask = if selection.is_rectangular() {
            None
        } else {
            selection
                .mask()
                .map(|mask| std::sync::Arc::from(mask.to_vec()))
        };
        Self {
            rect: selection.rect(),
            mask,
        }
    }

    /// Creates a clip that admits every pixel of `rect` (the unmasked case).
    pub fn from_rect(rect: Rect2i) -> Self {
        Self { rect, mask: None }
    }

    /// Returns the clip's bounding rectangle.
    pub fn rect(&self) -> Rect2i {
        self.rect
    }

    /// Returns whether the pixel belongs to the clip.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        if !self.rect.contains(x, y) {
            return false;
        }
        match &self.mask {
            None => true,
            Some(mask) => {
                let index = ((y - self.rect.y) as i64 * self.rect.w as i64
                    + (x - self.rect.x) as i64) as usize;
                mask.get(index).copied().unwrap_or(false)
            }
        }
    }

    /// Returns whether the clip contains no pixels.
    pub fn is_empty(&self) -> bool {
        if self.rect.is_empty() {
            return true;
        }
        match &self.mask {
            None => false,
            Some(mask) => !mask.iter().any(|&selected| selected),
        }
    }

    /// Returns the intersection of two clips: a pixel belongs only when both
    /// clips contain it.  The result stays a rect-only clip when neither input
    /// carries a mask; otherwise it is a masked clip over the overlapping rect.
    pub fn intersect(&self, other: &PixelClip) -> PixelClip {
        let rect = self.rect.intersection(other.rect);
        if self.mask.is_none() && other.mask.is_none() {
            return Self { rect, mask: None };
        }
        let area = rect.area().max(0) as usize;
        let mut mask = vec![false; area];
        for y in rect.y..rect.bottom() {
            for x in rect.x..rect.right() {
                if self.contains(x, y) && other.contains(x, y) {
                    let index =
                        ((y - rect.y) as i64 * rect.w as i64 + (x - rect.x) as i64) as usize;
                    mask[index] = true;
                }
            }
        }
        Self {
            rect,
            mask: Some(std::sync::Arc::from(mask)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::buffer::PixelBuffer;
    use crate::core::math::Rect2i;
    use crate::core::select::Selection;
    use std::sync::Arc;

    #[test]
    fn rectangular_selection_uses_a_rect_only_clip() {
        let buffer = PixelBuffer::new(8, 8);
        let selection = Selection::capture(&buffer, Rect2i::new(1, 1, 3, 2)).unwrap();
        let clip = PixelClip::from_selection(&selection);

        assert_eq!(clip.rect(), Rect2i::new(1, 1, 3, 2));
        assert!(clip.mask.is_none());
        assert!(clip.contains(1, 1));
        assert!(clip.contains(3, 2));
        assert!(!clip.contains(0, 1));
        assert!(!clip.contains(4, 1));
        assert!(!clip.is_empty());
    }

    #[test]
    fn masked_selection_contains_only_selected_cells() {
        let buffer = PixelBuffer::new(8, 8);
        let rect = Rect2i::new(1, 1, 3, 2);
        let selection =
            Selection::capture_mask(&buffer, rect, vec![true, false, true, true, true, false])
                .unwrap();
        let clip = PixelClip::from_selection(&selection);
        let clone = clip.clone();

        assert_eq!(clip.rect(), rect);
        assert!(clip.mask.is_some());
        assert!(clip.contains(1, 1));
        assert!(!clip.contains(2, 1));
        assert!(clip.contains(3, 1));
        assert!(clip.contains(1, 2));
        assert!(!clip.contains(0, 1));
        assert!(!clip.contains(4, 1));
        assert!(!clip.is_empty());
        assert!(Arc::ptr_eq(
            clip.mask.as_ref().unwrap(),
            clone.mask.as_ref().unwrap()
        ));
    }

    #[test]
    /// Given a rectangular selection, when it becomes a clip, then the clip stores no mask.
    fn pixel_clip_from_rectangular_selection_has_no_mask() {
        let buffer = PixelBuffer::new(8, 8);
        let selection = Selection::capture(&buffer, Rect2i::new(1, 1, 3, 2)).unwrap();

        let clip = PixelClip::from_selection(&selection);

        assert_eq!(clip.rect(), Rect2i::new(1, 1, 3, 2));
        assert!(clip.mask.is_none());
    }

    #[test]
    /// Given a masked selection, when it becomes a clip, then only its selected cells are contained.
    fn pixel_clip_from_masked_selection_contains_only_selected_cells() {
        let buffer = PixelBuffer::new(8, 8);
        let rect = Rect2i::new(1, 1, 3, 2);
        let selection =
            Selection::capture_mask(&buffer, rect, vec![true, false, true, true, false, false])
                .unwrap();

        let clip = PixelClip::from_selection(&selection);

        assert!(clip.contains(1, 1));
        assert!(!clip.contains(2, 1));
        assert!(clip.contains(3, 1));
        assert!(clip.contains(1, 2));
        assert!(!clip.contains(2, 2));
    }

    #[test]
    /// Given a clip with a finite rectangle, when a cell is outside that rectangle, then containment is false.
    fn pixel_clip_contains_is_false_outside_the_rect() {
        let buffer = PixelBuffer::new(8, 8);
        let selection = Selection::capture(&buffer, Rect2i::new(2, 3, 2, 2)).unwrap();
        let clip = PixelClip::from_selection(&selection);

        assert!(!clip.contains(1, 3));
        assert!(!clip.contains(4, 3));
        assert!(!clip.contains(2, 2));
        assert!(!clip.contains(2, 5));
    }

    #[test]
    /// Given a bare rectangle, when it becomes a clip, then every pixel in it is contained and no mask is needed.
    fn pixel_clip_from_rect_contains_every_pixel_in_its_rect() {
        let rect = Rect2i::new(2, 3, 2, 2);

        let clip = PixelClip::from_rect(rect);

        assert!(clip.mask.is_none());
        assert_eq!(clip.rect(), rect);
        assert!(clip.contains(2, 3));
        assert!(clip.contains(3, 4));
        assert!(!clip.contains(1, 3));
        assert!(!clip.contains(2, 5));
    }

    #[test]
    /// Given an empty rectangle, when it becomes a clip, then it contains nothing and reports itself empty.
    fn pixel_clip_from_rect_of_an_empty_rect_is_empty() {
        let clip = PixelClip::from_rect(Rect2i::ZERO);

        assert!(clip.is_empty());
        assert!(!clip.contains(0, 0));
    }

    #[test]
    /// Given two clips, when they are intersected, then only pixels in both are kept.
    fn intersect_keeps_only_the_shared_pixels() {
        let buffer = PixelBuffer::new(8, 8);
        let selection = Selection::capture(&buffer, Rect2i::new(1, 1, 4, 4)).unwrap();
        let selection_clip = PixelClip::from_selection(&selection);
        let region = PixelClip::from_rect(Rect2i::new(3, 3, 4, 4));

        let clip = selection_clip.intersect(&region);

        assert!(clip.contains(3, 3) && clip.contains(4, 4));
        assert!(!clip.contains(2, 2), "outside the region");
        assert!(!clip.contains(5, 5), "outside the selection");
        assert!(!clip.contains(3, 6), "outside the selection");
    }

    #[test]
    /// Given a masked clip and a rect clip, when they are intersected, then the mask is preserved.
    fn intersect_preserves_the_mask() {
        let buffer = PixelBuffer::new(8, 8);
        let selection = Selection::capture_mask(
            &buffer,
            Rect2i::new(1, 1, 3, 3),
            vec![true, false, true, true, true, true, false, true, true],
        )
        .unwrap();
        let selection_clip = PixelClip::from_selection(&selection);
        let region = PixelClip::from_rect(Rect2i::new(1, 1, 2, 2));

        let clip = selection_clip.intersect(&region);

        assert!(clip.contains(1, 1));
        assert!(clip.contains(2, 2));
        assert!(!clip.contains(2, 1), "selection mask excludes it");
        assert!(!clip.contains(3, 1), "region excludes it");
        assert!(!clip.contains(1, 3), "region excludes it");
    }
}
