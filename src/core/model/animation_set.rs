//! Multi-animation registry: an ordered set of [`AnimationSequence`]s with a
//! single active one.
//!
//! Pure core — imports only [`AnimationSequence`] and `std`, never egui/wgpu/
//! winit (ARCHITECTURE.md §8).
//!
//! # Invariant
//!
//! An [`AnimationSet`] **always contains at least one** sequence:
//! [`AnimationSet::new`] seeds one and [`AnimationSet::remove`] refuses to
//! remove the last one. `is_empty` is therefore always `false`, but is kept
//! for API completeness and to make the invariant explicit.

use super::sequence::AnimationSequence;

/// An ordered registry of animation sequences with one active sequence.
///
/// `sequences` always has `len() >= 1`; `active` always indexes a valid
/// element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimationSet {
    sequences: Vec<AnimationSequence>,
    active: usize,
}

impl AnimationSet {
    /// Creates a set seeded with a single sequence named `"Animation 1"`,
    /// which becomes active.
    pub fn new() -> Self {
        Self {
            sequences: vec![AnimationSequence::new("Animation 1")],
            active: 0,
        }
    }

    /// Number of sequences in the set. Always `>= 1`.
    pub fn len(&self) -> usize {
        self.sequences.len()
    }

    /// Whether the set contains no sequences.
    ///
    /// By the never-empty invariant this is **always `false`**; it exists for
    /// API completeness.
    pub fn is_empty(&self) -> bool {
        self.sequences.is_empty()
    }

    /// Index of the active sequence.
    pub fn active(&self) -> usize {
        self.active
    }

    /// Makes the sequence at `i` active. Returns `false` when `i` is out of
    /// bounds or already active.
    pub fn set_active(&mut self, i: usize) -> bool {
        if i >= self.sequences.len() || i == self.active {
            return false;
        }
        self.active = i;
        true
    }

    /// Returns the sequence at `i`, or `None` when out of bounds.
    pub fn get(&self, i: usize) -> Option<&AnimationSequence> {
        self.sequences.get(i)
    }

    /// Returns the sequence at `i` mutably, or `None` when out of bounds.
    pub fn get_mut(&mut self, i: usize) -> Option<&mut AnimationSequence> {
        self.sequences.get_mut(i)
    }

    /// Returns the active sequence. Panics only if the invariant is broken
    /// (never in practice).
    pub fn active_ref(&self) -> &AnimationSequence {
        &self.sequences[self.active]
    }

    /// Returns the active sequence mutably. Panics only if the invariant is
    /// broken (never in practice).
    pub fn active_mut(&mut self) -> &mut AnimationSequence {
        &mut self.sequences[self.active]
    }

    /// Appends a new sequence, returns its index, and makes it active.
    pub fn add(&mut self, name: impl Into<String>) -> usize {
        self.sequences.push(AnimationSequence::new(name));
        let index = self.sequences.len() - 1;
        self.active = index;
        index
    }

    /// Removes the sequence at `i`, returning `true` on success.
    ///
    /// Refuses to remove the last remaining sequence (returns `false`),
    /// preserving the never-empty invariant. Adjusts `active` sensibly:
    /// removing an element before the active one shifts it down; removing the
    /// active (or a later) element clamps it to the new last index.
    pub fn remove(&mut self, i: usize) -> bool {
        if self.sequences.len() <= 1 || i >= self.sequences.len() {
            return false;
        }
        self.sequences.remove(i);
        // Clamp active to a valid index; when the removed index was before
        // active, shift down by one to keep pointing at the same sequence.
        if i < self.active {
            self.active -= 1;
        }
        self.active = self.active.min(self.sequences.len() - 1);
        true
    }

    /// Iterates the sequences in order.
    pub fn iter(&self) -> impl Iterator<Item = &AnimationSequence> {
        self.sequences.iter()
    }
}

impl Default for AnimationSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_seeds_one_active_sequence() {
        let set = AnimationSet::new();
        assert_eq!(set.len(), 1);
        assert!(!set.is_empty());
        assert_eq!(set.active(), 0);
        assert_eq!(set.active_ref().name(), "Animation 1");
    }

    #[test]
    fn never_empty_invariant_holds_after_removal_attempts() {
        let mut set = AnimationSet::new();
        assert!(!set.remove(0));
        assert_eq!(set.len(), 1);
        assert_eq!(set.active(), 0);
        assert!(!set.remove(99));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn add_appends_returns_index_and_activates() {
        let mut set = AnimationSet::new();
        let i = set.add("walk");
        assert_eq!(i, 1);
        assert_eq!(set.len(), 2);
        assert_eq!(set.active(), 1);
        assert_eq!(set.active_ref().name(), "walk");
        let j = set.add("run");
        assert_eq!(j, 2);
        assert_eq!(set.active(), 2);
        assert_eq!(set.get(0).unwrap().name(), "Animation 1");
    }

    #[test]
    fn set_active_validates_bounds_and_noop() {
        let mut set = AnimationSet::new();
        set.add("walk");
        assert!(!set.set_active(1)); // already active
        assert!(set.set_active(0));
        assert_eq!(set.active(), 0);
        assert!(!set.set_active(5));
        assert_eq!(set.active(), 0);
    }

    #[test]
    fn remove_before_active_shifts_active_down() {
        let mut set = AnimationSet::new();
        set.add("b");
        set.add("c");
        // add() activates the newly appended sequence, so "c" is active.
        assert_eq!(set.active(), 2);
        assert!(set.remove(0));
        assert_eq!(set.len(), 2);
        assert_eq!(set.active(), 1);
        assert_eq!(set.active_ref().name(), "c");
    }

    #[test]
    fn remove_active_clamps_active() {
        let mut set = AnimationSet::new();
        set.add("b");
        set.add("c");
        assert_eq!(set.active(), 2);
        assert!(set.remove(2));
        assert_eq!(set.len(), 2);
        assert_eq!(set.active(), 1);
        assert_eq!(set.active_ref().name(), "b");
    }

    #[test]
    fn get_and_get_mut_respect_bounds() {
        let mut set = AnimationSet::new();
        set.add("b");
        assert!(set.get(0).is_some());
        assert!(set.get(2).is_none());
        assert!(set.get_mut(0).is_some());
        assert!(set.get_mut(2).is_none());
        set.active_mut().set_name("renamed");
        assert_eq!(set.get(1).unwrap().name(), "renamed");
    }

    #[test]
    fn iter_yields_sequences_in_order() {
        let mut set = AnimationSet::new();
        set.add("b");
        set.add("c");
        let names: Vec<&str> = set.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["Animation 1", "b", "c"]);
    }
}
