//! Animation sequence: an ordered list of frames with a loop flag.

use super::frame::Frame;

/// Hard cap on the number of frames in a sequence.
pub const MAX_FRAMES: usize = 4096;

/// An ordered animation sequence: frames play in order, optionally looping.
///
/// Frames are rect references into the sprite sheet — never pixel copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimationSequence {
    frames: Vec<Frame>,
    loop_flag: bool,
    name: String,
}

impl AnimationSequence {
    /// Creates an empty, non-looping sequence.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            frames: Vec::new(),
            loop_flag: false,
            name: name.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Appends a frame. Returns `false` when the sequence is already at
    /// [`MAX_FRAMES`].
    pub fn push(&mut self, frame: Frame) -> bool {
        if self.frames.len() >= MAX_FRAMES {
            return false;
        }
        self.frames.push(frame);
        true
    }

    /// Inserts a frame at `index`. Returns `false` when `index > len` or the
    /// sequence is already at [`MAX_FRAMES`].
    pub fn insert(&mut self, index: usize, frame: Frame) -> bool {
        if index > self.frames.len() || self.frames.len() >= MAX_FRAMES {
            return false;
        }
        self.frames.insert(index, frame);
        true
    }

    /// Removes the frame at `index`, returning it. `None` when out of bounds.
    pub fn remove(&mut self, index: usize) -> Option<Frame> {
        if index >= self.frames.len() {
            return None;
        }
        Some(self.frames.remove(index))
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn frame(&self, index: usize) -> Option<&Frame> {
        self.frames.get(index)
    }

    pub fn frame_mut(&mut self, index: usize) -> Option<&mut Frame> {
        self.frames.get_mut(index)
    }

    /// Moves the frame at `from` to `to` (splice-move semantics). Returns
    /// `false` when either index is out of bounds.
    pub fn reorder(&mut self, from: usize, to: usize) -> bool {
        if from >= self.frames.len() || to >= self.frames.len() {
            return false;
        }
        let frame = self.frames.remove(from);
        self.frames.insert(to, frame);
        true
    }

    pub fn looping(&self) -> bool {
        self.loop_flag
    }

    pub fn set_looping(&mut self, on: bool) {
        self.loop_flag = on;
    }

    /// Iterates the frames in play order.
    pub fn iter(&self) -> impl Iterator<Item = &Frame> {
        self.frames.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::math::Rect2i;
    use crate::core::model::region::Region;

    fn frame(name: &str, delay: u32) -> Frame {
        Frame::new(Region::new(Rect2i::new(0, 0, 16, 16), name), delay)
    }

    #[test]
    fn new_creates_empty_non_looping_sequence() {
        let seq = AnimationSequence::new("walk");
        assert_eq!(seq.name(), "walk");
        assert_eq!(seq.len(), 0);
        assert!(seq.is_empty());
        assert!(!seq.looping());
    }

    #[test]
    fn name_get_and_set() {
        let mut seq = AnimationSequence::new("walk");
        seq.set_name("run");
        assert_eq!(seq.name(), "run");
    }

    #[test]
    fn push_grows_and_reports_true() {
        let mut seq = AnimationSequence::new("s");
        assert!(seq.push(frame("a", 100)));
        assert!(seq.push(frame("b", 100)));
        assert_eq!(seq.len(), 2);
        assert!(!seq.is_empty());
    }

    #[test]
    fn insert_at_zero_middle_and_end() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        seq.push(frame("b", 100));
        seq.push(frame("c", 100));
        assert!(seq.insert(0, frame("x", 100)));
        assert!(seq.insert(2, frame("y", 100)));
        assert!(seq.insert(seq.len(), frame("z", 100)));
        let names: Vec<&str> = seq.iter().map(|f| f.region().name()).collect();
        assert_eq!(names, vec!["x", "a", "y", "b", "c", "z"]);
    }

    #[test]
    fn insert_beyond_len_is_false() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        assert!(!seq.insert(2, frame("x", 100)));
        assert_eq!(seq.len(), 1);
    }

    #[test]
    fn remove_first_last_middle_returns_frame_and_shrinks() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        seq.push(frame("b", 100));
        seq.push(frame("c", 100));
        assert_eq!(seq.remove(0).unwrap().region().name(), "a");
        assert_eq!(seq.len(), 2);
        assert_eq!(seq.remove(1).unwrap().region().name(), "c");
        assert_eq!(seq.len(), 1);
        assert_eq!(seq.remove(0).unwrap().region().name(), "b");
        assert!(seq.is_empty());
    }

    #[test]
    fn remove_out_of_bounds_is_none() {
        let mut seq = AnimationSequence::new("s");
        assert_eq!(seq.remove(0), None);
        seq.push(frame("a", 100));
        assert_eq!(seq.remove(1), None);
        assert_eq!(seq.len(), 1);
    }

    #[test]
    fn frame_and_frame_mut_respect_bounds() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        assert!(seq.frame(0).is_some());
        assert!(seq.frame(1).is_none());
        assert!(seq.frame_mut(1).is_none());
    }

    #[test]
    fn frame_mut_can_change_delay_clamped() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        seq.frame_mut(0).unwrap().set_delay_ms(5_000_000);
        assert_eq!(seq.frame(0).unwrap().delay_ms(), 60_000);
    }

    #[test]
    fn reorder_moves_element() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        seq.push(frame("b", 100));
        seq.push(frame("c", 100));
        assert!(seq.reorder(2, 0));
        let names: Vec<&str> = seq.iter().map(|f| f.region().name()).collect();
        assert_eq!(names, vec!["c", "a", "b"]);
        assert!(seq.reorder(0, 2));
        let names: Vec<&str> = seq.iter().map(|f| f.region().name()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn reorder_invalid_bounds_is_false() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        seq.push(frame("b", 100));
        assert!(!seq.reorder(2, 0));
        assert!(!seq.reorder(0, 2));
        assert!(!seq.reorder(5, 5));
        assert_eq!(seq.len(), 2);
    }

    #[test]
    fn set_looping_flips_flag() {
        let mut seq = AnimationSequence::new("s");
        assert!(!seq.looping());
        seq.set_looping(true);
        assert!(seq.looping());
        seq.set_looping(false);
        assert!(!seq.looping());
    }

    #[test]
    fn iter_yields_frames_in_order() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        seq.push(frame("b", 200));
        seq.push(frame("c", 300));
        let delays: Vec<u32> = seq.iter().map(|f| f.delay_ms()).collect();
        assert_eq!(delays, vec![100, 200, 300]);
    }

    #[test]
    fn push_rejects_when_full() {
        let mut seq = AnimationSequence::new("s");
        for i in 0..MAX_FRAMES {
            assert!(seq.push(frame(&format!("f{i}"), 100)));
        }
        assert_eq!(seq.len(), MAX_FRAMES);
        assert!(!seq.push(frame("overflow", 100)));
        assert_eq!(seq.len(), MAX_FRAMES);
    }

    #[test]
    fn insert_rejects_when_full() {
        let mut seq = AnimationSequence::new("s");
        for i in 0..MAX_FRAMES {
            assert!(seq.push(frame(&format!("f{i}"), 100)));
        }
        assert!(!seq.insert(0, frame("overflow", 100)));
        assert_eq!(seq.len(), MAX_FRAMES);
    }
}
