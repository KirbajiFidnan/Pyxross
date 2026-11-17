//! Animation sequence: an ordered list of frames with a loop flag.

use super::frame::Frame;

/// Hard cap on the number of frames in a sequence.
pub const MAX_FRAMES: usize = 4096;

/// Lower bound for the delay multiplier, in hundredths (0.01×).
pub const MIN_DELAY_MULTIPLIER_CENTI: u32 = 1;
/// Upper bound for the delay multiplier, in hundredths (100.00×).
pub const MAX_DELAY_MULTIPLIER_CENTI: u32 = 10_000;
/// Default delay multiplier, in hundredths (1.00×).
pub const DEFAULT_DELAY_MULTIPLIER_CENTI: u32 = 100;

/// Upper bound for an effective playback delay in milliseconds (60 s),
/// matching [`Frame`]'s own cap.
const MAX_EFFECTIVE_DELAY_MS: u64 = 60_000;

/// An ordered animation sequence: frames play in order, optionally looping.
///
/// Frames are rect references into the sprite sheet — never pixel copies.
///
/// Each sequence carries a **delay multiplier** stored as an integer in
/// hundredths ("centi"): `100` = 1.00×, `200` = 2.00×, `50` = 0.50×. An
/// integer keeps the type's `Eq` derive valid (a float would not) while still
/// expressing two decimal places.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimationSequence {
    frames: Vec<Frame>,
    loop_flag: bool,
    name: String,
    /// Delay multiplier in hundredths; always clamped to
    /// [`MIN_DELAY_MULTIPLIER_CENTI`]`..=`[`MAX_DELAY_MULTIPLIER_CENTI`].
    delay_multiplier: u32,
    /// Free-form animation tags (e.g. "walk", "idle"), mirroring the
    /// document format's `SequenceDoc::tags`. Empty tags are never stored and
    /// duplicates are ignored by [`Self::add_tag`].
    tags: Vec<String>,
}

impl AnimationSequence {
    /// Creates an empty, non-looping sequence with the default delay
    /// multiplier of 1.00×.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            frames: Vec::new(),
            loop_flag: false,
            name: name.into(),
            delay_multiplier: DEFAULT_DELAY_MULTIPLIER_CENTI,
            tags: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// The sequence's animation tags, in insertion order.
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Replaces the tag list wholesale.
    pub fn set_tags(&mut self, tags: Vec<String>) {
        self.tags = tags;
    }

    /// Appends `tag` unless it is empty (after trimming) or already present.
    pub fn add_tag(&mut self, tag: String) {
        if tag.trim().is_empty() || self.tags.iter().any(|existing| existing == &tag) {
            return;
        }
        self.tags.push(tag);
    }

    /// Removes the first tag equal to `tag`, returning `true` when one was
    /// removed.
    pub fn remove_tag(&mut self, tag: &str) -> bool {
        if let Some(index) = self.tags.iter().position(|existing| existing == tag) {
            self.tags.remove(index);
            true
        } else {
            false
        }
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

    /// Returns the delay multiplier in hundredths (e.g. `100` = 1.00×).
    pub fn delay_multiplier_centi(&self) -> u32 {
        self.delay_multiplier
    }

    /// Returns the delay multiplier as a float (e.g. `1.0` for 100 centi).
    pub fn delay_multiplier(&self) -> f64 {
        self.delay_multiplier as f64 / 100.0
    }

    /// Sets the delay multiplier in hundredths, clamped to `1..=10_000`
    /// (0.01×..=100.00×).
    pub fn set_delay_multiplier_centi(&mut self, centi: u32) {
        self.delay_multiplier = centi.clamp(
            MIN_DELAY_MULTIPLIER_CENTI,
            MAX_DELAY_MULTIPLIER_CENTI,
        );
    }

    /// Sets the delay multiplier from a float. Non-finite or out-of-range
    /// values are clamped to the nearest valid centi: the multiplier is
    /// rounded via `round(m * 100)` and clamped to `1..=10_000`.
    pub fn set_delay_multiplier(&mut self, m: f64) {
        let centi = if m.is_finite() {
            (m * 100.0).round()
        } else {
            // NaN and ±inf have no nearest valid value; fall back to the
            // nearest bound sign-aware: +inf → max, -inf/NaN → min.
            if m > 0.0 {
                MAX_DELAY_MULTIPLIER_CENTI as f64
            } else if m < 0.0 {
                MIN_DELAY_MULTIPLIER_CENTI as f64
            } else {
                DEFAULT_DELAY_MULTIPLIER_CENTI as f64
            }
        };
        let clamped = centi.clamp(
            MIN_DELAY_MULTIPLIER_CENTI as f64,
            MAX_DELAY_MULTIPLIER_CENTI as f64,
        );
        self.delay_multiplier = clamped as u32;
    }

    /// Returns the effective playback delay, in milliseconds, for the frame at
    /// `index`.
    ///
    /// This is the **single source of truth for playback AND GIF export**:
    /// both must route through this method so the two never disagree. The
    /// frame's `delay_ms` (0 when `index` is out of bounds) is scaled by the
    /// delay multiplier using integer math with round-half-up:
    ///
    /// ```text
    /// ((delay as u64 * centi as u64) + 50) / 100
    /// ```
    ///
    /// The result is clamped to `0..=60_000` (60 s), matching
    /// [`Frame::set_delay_ms`]'s cap.
    pub fn effective_delay_ms(&self, index: usize) -> u32 {
        let delay = self.frames.get(index).map_or(0, Frame::delay_ms);
        let scaled = (delay as u64 * self.delay_multiplier as u64 + 50) / 100;
        scaled.min(MAX_EFFECTIVE_DELAY_MS) as u32
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

    #[test]
    fn new_defaults_delay_multiplier_to_one() {
        let seq = AnimationSequence::new("s");
        assert_eq!(seq.delay_multiplier_centi(), 100);
        assert_eq!(seq.delay_multiplier(), 1.0);
    }

    #[test]
    fn set_delay_multiplier_centi_clamps_low_and_high() {
        let mut seq = AnimationSequence::new("s");
        seq.set_delay_multiplier_centi(0);
        assert_eq!(seq.delay_multiplier_centi(), 1);
        seq.set_delay_multiplier_centi(u32::MAX);
        assert_eq!(seq.delay_multiplier_centi(), 10_000);
        seq.set_delay_multiplier_centi(250);
        assert_eq!(seq.delay_multiplier_centi(), 250);
        assert_eq!(seq.delay_multiplier(), 2.5);
    }

    #[test]
    fn set_delay_multiplier_float_rounds_and_clamps() {
        let mut seq = AnimationSequence::new("s");
        seq.set_delay_multiplier(1.234);
        assert_eq!(seq.delay_multiplier_centi(), 123);
        seq.set_delay_multiplier(0.0);
        assert_eq!(seq.delay_multiplier_centi(), 1);
        seq.set_delay_multiplier(1000.0);
        assert_eq!(seq.delay_multiplier_centi(), 10_000);
        seq.set_delay_multiplier(f64::NAN);
        assert_eq!(seq.delay_multiplier_centi(), 100);
    }

    #[test]
    fn effective_delay_ms_scales_with_round_half_up() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 100));
        seq.push(frame("b", 101));
        assert_eq!(seq.effective_delay_ms(0), 100);
        assert_eq!(seq.effective_delay_ms(1), 101);
        // 2× multiplier
        seq.set_delay_multiplier_centi(200);
        assert_eq!(seq.effective_delay_ms(0), 200);
        assert_eq!(seq.effective_delay_ms(1), 202);
        // 0.5× multiplier: 101 * 50 / 100 = 50.5 -> round-half-up -> 51
        seq.set_delay_multiplier_centi(50);
        assert_eq!(seq.effective_delay_ms(0), 50);
        assert_eq!(seq.effective_delay_ms(1), 51);
    }

    #[test]
    fn effective_delay_ms_out_of_bounds_is_zero() {
        let mut seq = AnimationSequence::new("s");
        assert_eq!(seq.effective_delay_ms(0), 0);
        seq.push(frame("a", 100));
        assert_eq!(seq.effective_delay_ms(5), 0);
    }

    #[test]
    fn effective_delay_ms_clamps_to_sixty_seconds() {
        let mut seq = AnimationSequence::new("s");
        seq.push(frame("a", 60_000));
        seq.set_delay_multiplier_centi(10_000);
        assert_eq!(seq.effective_delay_ms(0), 60_000);
    }

    #[test]
    fn new_has_empty_tags() {
        let seq = AnimationSequence::new("walk");
        assert!(seq.tags().is_empty());
    }

    #[test]
    fn add_tag_ignores_empty_and_duplicate_and_appends_valid() {
        let mut seq = AnimationSequence::new("s");
        seq.add_tag("walk".to_string());
        seq.add_tag("walk".to_string()); // duplicate
        seq.add_tag("   ".to_string()); // trimmed-empty
        seq.add_tag(String::new()); // empty
        seq.add_tag("idle".to_string());
        assert_eq!(seq.tags(), &["walk".to_string(), "idle".to_string()]);
    }

    #[test]
    fn remove_tag_returns_true_only_when_removed() {
        let mut seq = AnimationSequence::new("s");
        seq.add_tag("walk".to_string());
        assert!(seq.remove_tag("walk"));
        assert!(!seq.remove_tag("walk"));
        assert!(seq.tags().is_empty());
        assert!(!seq.remove_tag("missing"));
    }

    #[test]
    fn set_tags_replaces_existing() {
        let mut seq = AnimationSequence::new("s");
        seq.add_tag("walk".to_string());
        seq.set_tags(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(seq.tags(), &["a".to_string(), "b".to_string()]);
    }
}
