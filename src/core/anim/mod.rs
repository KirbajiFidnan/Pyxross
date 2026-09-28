//! Animation controller — deterministic frame timing & playback.

mod onion;

pub use onion::OnionConfig;

/// Deterministic animation controller driven entirely by explicit
/// `advance(delta_ms)` calls — no wall-clock, no scene-tree timer.
/// The app loop is the single source of time deltas.
#[derive(Clone, Debug)]
pub struct AnimationController {
    frame_count: usize,
    frame_duration_ms: u32,
    delays: Vec<u32>,
    current: usize,
    elapsed_ms: u32,
    playing: bool,
    looping: bool,
    change_epoch: u64,
}

impl AnimationController {
    /// Creates a new controller at frame 0, not playing, looping enabled.
    pub fn new(frame_count: usize, frame_duration_ms: u32) -> Self {
        Self {
            frame_count,
            frame_duration_ms,
            delays: Vec::new(),
            current: 0,
            elapsed_ms: 0,
            playing: false,
            looping: true,
            change_epoch: 0,
        }
    }

    /// Creates a controller in per-frame delay mode. `delays[i]` is the
    /// duration of frame `i`; frames past the end of `delays` fall back to
    /// the uniform `frame_duration_ms` (100 ms).
    pub fn new_from_delays(frame_count: usize, delays: Vec<u32>) -> Self {
        Self {
            frame_count,
            frame_duration_ms: 100,
            delays,
            current: 0,
            elapsed_ms: 0,
            playing: false,
            looping: true,
            change_epoch: 0,
        }
    }

    /// Replaces the per-frame delay table. Bumps epoch once.
    pub fn set_delays(&mut self, delays: Vec<u32>) {
        self.delays = delays;
        self.change_epoch += 1;
    }

    /// Per-frame delay at `index`, or the uniform `frame_duration_ms`
    /// fallback when `index` is past the end of the delay table.
    pub fn delay_ms(&self, index: usize) -> u32 {
        if index < self.delays.len() {
            self.delays[index]
        } else {
            self.frame_duration_ms
        }
    }

    pub fn has_per_frame_delays(&self) -> bool {
        !self.delays.is_empty()
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    pub fn frame_duration_ms(&self) -> u32 {
        self.frame_duration_ms
    }

    /// Set frame duration in milliseconds. Bumps epoch once.
    pub fn set_frame_duration_ms(&mut self, ms: u32) {
        self.frame_duration_ms = ms;
        self.change_epoch += 1;
    }

    /// Set total frame count; clamps `current` into the valid range.
    /// Bumps epoch once.
    pub fn set_frame_count(&mut self, n: usize) {
        self.frame_count = n;
        if self.current >= n {
            self.current = n.saturating_sub(1);
        }
        self.change_epoch += 1;
    }

    /// Current frame index, always in `0..frame_count` (0 when empty).
    pub fn current_index(&self) -> usize {
        if self.frame_count == 0 {
            0
        } else {
            self.current
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn looping(&self) -> bool {
        self.looping
    }

    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }

    /// Start or resume playback from the current index.
    pub fn play(&mut self) {
        self.playing = true;
        self.change_epoch += 1;
    }

    /// Stop playback, preserving the current index.
    pub fn stop(&mut self) {
        self.playing = false;
        self.change_epoch += 1;
    }

    /// Reset to frame 0 and elapsed 0. If playing, stays playing.
    pub fn restart(&mut self) {
        self.current = 0;
        self.elapsed_ms = 0;
        self.change_epoch += 1;
    }

    /// Jump to a specific frame (clamped to valid range).
    /// Bumps epoch once.
    pub fn goto(&mut self, frame: usize) {
        if self.frame_count == 0 {
            self.current = 0;
        } else if frame >= self.frame_count {
            self.current = self.frame_count - 1;
        } else {
            self.current = frame;
        }
        self.change_epoch += 1;
    }

    /// The deterministic core. Accumulates `delta_ms` into the frame budget
    /// and advances frames, keeping the remainder. When not playing this is a
    /// no-op.
    ///
    /// **Uniform mode** (no per-frame delays): advances exactly
    /// `elapsed / effective_duration` frames, keeping the remainder.
    ///
    /// **Per-frame mode** (`delays` non-empty): consumes the budget stepwise,
    /// subtracting each frame's own delay; frames past the end of `delays`
    /// fall back to the uniform `frame_duration_ms`.
    ///
    /// **Zero-duration safety**: a duration of 0 is treated as 1 ms (never
    /// divide by zero, never spin).
    pub fn advance(&mut self, delta_ms: u32) {
        if !self.playing || self.frame_count == 0 {
            return;
        }

        self.elapsed_ms = self.elapsed_ms.saturating_add(delta_ms);

        let mut changed = false;

        if self.delays.is_empty() {
            // Uniform mode: fast math path.
            let effective_duration = self.frame_duration_ms.max(1);
            let frames_to_advance = (self.elapsed_ms / effective_duration) as usize;
            if frames_to_advance == 0 {
                return;
            }
            self.elapsed_ms %= effective_duration;
            if self.looping {
                let new_index = (self.current + frames_to_advance) % self.frame_count;
                if new_index != self.current {
                    self.current = new_index;
                    changed = true;
                }
            } else {
                let end = self.frame_count - 1;
                if self.current + frames_to_advance > end {
                    if self.current != end {
                        self.current = end;
                    }
                    self.playing = false;
                    self.elapsed_ms = 0;
                    changed = true;
                } else {
                    let new_index = self.current + frames_to_advance;
                    if new_index != self.current {
                        self.current = new_index;
                        changed = true;
                    }
                }
            }
        } else if self.looping {
            // Per-frame mode, looping: consume each frame's own delay.
            while self.elapsed_ms > 0 {
                let duration = self.effective_duration(self.current);
                if self.elapsed_ms < duration {
                    break;
                }
                self.elapsed_ms -= duration;
                let next = (self.current + 1) % self.frame_count;
                if next != self.current {
                    self.current = next;
                    changed = true;
                }
            }
        } else {
            // Per-frame mode, non-looping: stop at the last frame.
            let end = self.frame_count - 1;
            while self.elapsed_ms > 0 && self.current < end {
                let duration = self.effective_duration(self.current);
                if self.elapsed_ms < duration {
                    break;
                }
                self.elapsed_ms -= duration;
                self.current += 1;
                changed = true;
            }
            if self.current == end && self.elapsed_ms >= self.effective_duration(end) {
                self.playing = false;
                self.elapsed_ms = 0;
                changed = true;
            }
        }

        if changed {
            self.change_epoch += 1;
        }
    }

    /// Effective duration of frame `index`: its per-frame delay when present
    /// (0 treated as 1 ms), else the uniform `frame_duration_ms` fallback.
    fn effective_duration(&self, index: usize) -> u32 {
        if index < self.delays.len() {
            self.delays[index].max(1)
        } else {
            self.frame_duration_ms.max(1)
        }
    }

    pub fn change_epoch(&self) -> u64 {
        self.change_epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state() {
        let ac = AnimationController::new(10, 100);
        assert_eq!(ac.current_index(), 0);
        assert!(!ac.is_playing());
        assert!(ac.looping());
        assert_eq!(ac.frame_count(), 10);
        assert_eq!(ac.frame_duration_ms(), 100);
        assert_eq!(ac.change_epoch(), 0);
    }

    #[test]
    fn advance_while_stopped_is_noop() {
        let mut ac = AnimationController::new(5, 100);
        ac.advance(500);
        assert_eq!(ac.current_index(), 0);
        assert_eq!(ac.elapsed_ms, 0);
    }

    #[test]
    fn play_then_advance_basic() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        ac.advance(100);
        assert_eq!(ac.current_index(), 1);
        ac.advance(100);
        assert_eq!(ac.current_index(), 2);
    }

    #[test]
    fn fractional_accumulation() {
        let mut ac = AnimationController::new(10, 100);
        ac.play();

        ac.advance(35);
        assert_eq!(ac.current_index(), 0);

        ac.advance(35);
        assert_eq!(ac.current_index(), 0);

        ac.advance(35);
        assert_eq!(ac.current_index(), 1);
    }

    #[test]
    fn fractional_accumulation_deterministic_sequence() {
        let mut ac = AnimationController::new(10, 100);
        ac.play();

        ac.advance(35);
        assert_eq!(ac.current_index(), 0, "after first advance(35)");

        ac.advance(35);
        assert_eq!(ac.current_index(), 0, "after second advance(35)");

        ac.advance(35);
        assert_eq!(ac.current_index(), 1, "after third advance(35)");
    }

    #[test]
    fn looping_wrap() {
        let mut ac = AnimationController::new(3, 100);
        ac.play();

        ac.advance(100);
        ac.advance(100);
        assert_eq!(ac.current_index(), 2);

        ac.advance(100);
        assert_eq!(ac.current_index(), 0);
        assert!(ac.is_playing(), "still playing when looping");
    }

    #[test]
    fn looping_wrap_multi_frame_jump() {
        let mut ac = AnimationController::new(3, 100);
        ac.play();
        ac.advance(500);
        assert_eq!(ac.current_index(), 2);
        assert!(ac.is_playing());
    }

    #[test]
    fn non_looping_stops_at_last_frame() {
        let mut ac = AnimationController::new(3, 100);
        ac.set_looping(false);
        ac.play();

        ac.advance(100);
        assert_eq!(ac.current_index(), 1);
        assert!(ac.is_playing());

        ac.advance(100);
        assert_eq!(ac.current_index(), 2);
        assert!(ac.is_playing());

        ac.advance(100);
        assert_eq!(ac.current_index(), 2);
        assert!(!ac.is_playing());
    }

    #[test]
    fn non_looping_extra_advances_are_noops() {
        let mut ac = AnimationController::new(3, 100);
        ac.set_looping(false);
        ac.play();

        ac.advance(500);
        assert_eq!(ac.current_index(), 2);
        assert!(!ac.is_playing());

        let epoch = ac.change_epoch();
        ac.advance(1000);
        assert_eq!(ac.current_index(), 2);
        assert!(!ac.is_playing());
        assert_eq!(ac.change_epoch(), epoch, "no epoch bump for noop advance");
    }

    #[test]
    fn stop_preserves_current_index() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        ac.advance(250);
        assert_eq!(ac.current_index(), 2);

        ac.stop();
        assert_eq!(ac.current_index(), 2);
        assert!(!ac.is_playing());
    }

    #[test]
    fn play_resumes_from_current() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        ac.advance(250);
        ac.stop();
        assert_eq!(ac.current_index(), 2);

        ac.play();
        ac.advance(100);
        assert_eq!(ac.current_index(), 3);
    }

    #[test]
    fn restart_resets_index_and_elapsed() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        ac.advance(250);
        ac.restart();

        assert_eq!(ac.current_index(), 0);
        assert_eq!(ac.elapsed_ms, 0);
        assert!(ac.is_playing(), "stays playing after restart");
    }

    #[test]
    fn restart_while_stopped_resets_to_zero() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        ac.advance(250);
        ac.stop();
        assert_eq!(ac.current_index(), 2);

        ac.restart();
        assert_eq!(ac.current_index(), 0);
        assert!(!ac.is_playing());
    }

    #[test]
    fn goto_clamps_out_of_range() {
        let mut ac = AnimationController::new(5, 100);
        ac.goto(999);
        assert_eq!(ac.current_index(), 4);

        ac.goto(0);
        assert_eq!(ac.current_index(), 0);
    }

    #[test]
    fn goto_zero_frame_count() {
        let mut ac = AnimationController::new(0, 100);
        ac.goto(0);
        assert_eq!(ac.current_index(), 0);
    }

    #[test]
    fn zero_duration_safety() {
        let mut ac = AnimationController::new(5, 0);
        ac.play();
        ac.advance(1_000_000);
        assert_eq!(ac.current_index(), 0);
        assert!(ac.is_playing());
    }

    #[test]
    fn zero_duration_no_infinite_spin() {
        let mut ac = AnimationController::new(1, 0);
        ac.play();
        ac.advance(1);
        assert_eq!(ac.current_index(), 0);
    }

    #[test]
    fn zero_frame_count_advance_noop() {
        let mut ac = AnimationController::new(0, 100);
        ac.play();
        ac.advance(500);
        assert_eq!(ac.current_index(), 0);
    }

    #[test]
    fn epoch_bumps_on_play() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.play();
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_bumps_on_stop() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        let before = ac.change_epoch();
        ac.stop();
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_bumps_on_goto() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.goto(3);
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_bumps_on_set_frame_duration() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.set_frame_duration_ms(200);
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_bumps_on_advance_that_changes_frame() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        let before = ac.change_epoch();
        ac.advance(100);
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_no_bump_for_stopped_advance() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.advance(100);
        assert_eq!(ac.change_epoch(), before);
    }

    #[test]
    fn epoch_no_bump_for_partial_advance() {
        let mut ac = AnimationController::new(5, 100);
        ac.play();
        let before = ac.change_epoch();
        ac.advance(50);
        assert_eq!(ac.change_epoch(), before);
    }

    #[test]
    fn epoch_bumps_on_restart() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.restart();
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_bumps_once_on_set_frame_count() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.set_frame_count(10);
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_bumps_exactly_once_per_advance_that_changes() {
        let mut ac = AnimationController::new(4, 100);
        ac.play();
        let before = ac.change_epoch();
        ac.advance(350);
        assert_eq!(ac.change_epoch(), before + 1);
    }

    #[test]
    fn epoch_no_bump_when_looping_wraps_to_same_index() {
        let mut ac = AnimationController::new(3, 100);
        ac.play();
        let before = ac.change_epoch();
        ac.advance(300);
        assert_eq!(ac.current_index(), 0);
        assert_eq!(
            ac.change_epoch(),
            before,
            "wrapped to same index, no visual change"
        );
    }

    #[test]
    fn set_frame_count_clamps_current() {
        let mut ac = AnimationController::new(10, 100);
        ac.play();
        ac.advance(500);
        assert_eq!(ac.current_index(), 5);

        ac.set_frame_count(3);
        assert_eq!(ac.current_index(), 2);
    }

    #[test]
    fn determinism_multiple_sequences_produce_same_result() {
        let sequence = [35u32, 35, 35, 35, 35, 35, 35, 35, 35, 35];
        let run = |seq: &[u32]| -> Vec<usize> {
            let mut ac = AnimationController::new(4, 100);
            ac.play();
            let mut indices = Vec::new();
            for &d in seq {
                ac.advance(d);
                indices.push(ac.current_index());
            }
            indices
        };

        let a = run(&sequence);
        let b = run(&sequence);
        assert_eq!(a, b);
    }

    #[test]
    fn advance_never_skips_beyond_delta_warrants() {
        let mut ac = AnimationController::new(100, 100);
        ac.play();

        let epoch_before = ac.change_epoch();
        ac.advance(50);
        assert_eq!(
            ac.current_index(),
            0,
            "50ms should not advance a 100ms frame"
        );
        assert_eq!(ac.change_epoch(), epoch_before, "no epoch bump");
    }

    #[test]
    fn large_delta_looping_wraps_correctly() {
        let mut ac = AnimationController::new(3, 1000);
        ac.play();
        ac.advance(5000);
        assert_eq!(ac.current_index(), 2);
    }

    #[test]
    fn set_looping_does_not_bump_epoch() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.set_looping(false);
        assert_eq!(ac.change_epoch(), before);
    }

    #[test]
    fn new_from_delays_initial_state() {
        let ac = AnimationController::new_from_delays(4, vec![100, 200, 50]);
        assert_eq!(ac.frame_count(), 4);
        assert_eq!(ac.current_index(), 0);
        assert!(!ac.is_playing());
        assert!(ac.looping());
        assert_eq!(ac.change_epoch(), 0);
        assert_eq!(ac.frame_duration_ms(), 100, "uniform fallback");
        assert!(ac.has_per_frame_delays());
        assert_eq!(ac.delay_ms(0), 100);
        assert_eq!(ac.delay_ms(1), 200);
        assert_eq!(ac.delay_ms(2), 50);
        assert_eq!(ac.delay_ms(3), 100, "past end falls back to uniform");
    }

    #[test]
    fn per_frame_advance_uses_each_frames_delay() {
        let mut ac = AnimationController::new_from_delays(3, vec![100, 200, 50]);
        ac.play();
        ac.advance(100);
        assert_eq!(ac.current_index(), 1);
        ac.advance(100);
        assert_eq!(ac.current_index(), 1, "200ms frame not yet done");
        ac.advance(100);
        assert_eq!(ac.current_index(), 2);
        ac.advance(50);
        assert_eq!(ac.current_index(), 0, "wrap to frame 0");
    }

    #[test]
    fn per_frame_wrap_rereads_delays_zero() {
        let mut ac = AnimationController::new_from_delays(2, vec![200, 200]);
        ac.play();
        ac.advance(200);
        assert_eq!(ac.current_index(), 1);
        let epoch = ac.change_epoch();
        ac.advance(200);
        assert_eq!(ac.current_index(), 0);
        assert_eq!(ac.change_epoch(), epoch + 1, "wrap bumps exactly once");
    }

    #[test]
    fn per_frame_advance_crosses_multiple_frames_one_epoch_bump() {
        let mut ac = AnimationController::new_from_delays(3, vec![100, 100, 100]);
        ac.play();
        let before = ac.change_epoch();
        ac.advance(250);
        assert_eq!(ac.current_index(), 2);
        assert_eq!(
            ac.change_epoch(),
            before + 1,
            "one bump despite crossing 2 frames"
        );
    }

    #[test]
    fn set_delays_bumps_epoch_once() {
        let mut ac = AnimationController::new(5, 100);
        let before = ac.change_epoch();
        ac.set_delays(vec![50, 60, 70]);
        assert_eq!(ac.change_epoch(), before + 1);
        assert!(ac.has_per_frame_delays());
        assert_eq!(ac.delay_ms(0), 50);
    }

    #[test]
    fn delay_ms_falls_back_to_uniform() {
        let ac = AnimationController::new(5, 100);
        assert_eq!(ac.delay_ms(0), 100, "empty table falls back to uniform");
        let ac = AnimationController::new_from_delays(5, vec![50, 60]);
        assert_eq!(ac.delay_ms(0), 50);
        assert_eq!(ac.delay_ms(1), 60);
        assert_eq!(ac.delay_ms(2), 100, "past end falls back to uniform");
    }

    #[test]
    fn has_per_frame_delays_reflects_mode() {
        assert!(!AnimationController::new(5, 100).has_per_frame_delays());
        assert!(AnimationController::new_from_delays(5, vec![100]).has_per_frame_delays());
        let mut ac = AnimationController::new_from_delays(5, vec![100]);
        ac.set_delays(Vec::new());
        assert!(!ac.has_per_frame_delays());
    }

    #[test]
    fn mixed_mode_frames_past_delays_use_uniform_fallback() {
        let mut ac = AnimationController::new_from_delays(4, vec![100, 100]);
        ac.play();
        ac.advance(100);
        assert_eq!(ac.current_index(), 1);
        ac.advance(100);
        assert_eq!(ac.current_index(), 2, "frame 2 uses uniform fallback");
        ac.advance(100);
        assert_eq!(ac.current_index(), 3);
        ac.advance(100);
        assert_eq!(ac.current_index(), 0, "wrap");
    }

    #[test]
    fn zero_per_frame_delay_no_spin() {
        let mut ac = AnimationController::new_from_delays(3, vec![0, 0, 0]);
        ac.play();
        ac.advance(1);
        assert_eq!(ac.current_index(), 1, "0 treated as 1ms");
        ac.advance(1);
        assert_eq!(ac.current_index(), 2);
        ac.advance(1);
        assert_eq!(ac.current_index(), 0, "wrap");
        assert!(ac.is_playing());
    }

    #[test]
    fn per_frame_non_looping_stops_at_last() {
        let mut ac = AnimationController::new_from_delays(3, vec![100, 200, 50]);
        ac.set_looping(false);
        ac.play();
        ac.advance(100);
        assert_eq!(ac.current_index(), 1);
        assert!(ac.is_playing());
        ac.advance(200);
        assert_eq!(ac.current_index(), 2);
        assert!(ac.is_playing());
        ac.advance(50);
        assert_eq!(ac.current_index(), 2);
        assert!(!ac.is_playing());
    }

    #[test]
    fn per_frame_partial_advance_keeps_remainder() {
        let mut ac = AnimationController::new_from_delays(3, vec![100, 200, 50]);
        ac.play();
        ac.advance(50);
        assert_eq!(ac.current_index(), 0);
        ac.advance(50);
        assert_eq!(ac.current_index(), 1, "remainder carried over");
    }

    #[test]
    fn per_frame_epoch_no_bump_for_partial() {
        let mut ac = AnimationController::new_from_delays(3, vec![100, 200, 50]);
        ac.play();
        let before = ac.change_epoch();
        ac.advance(50);
        assert_eq!(ac.change_epoch(), before);
    }

    #[test]
    fn set_delays_empty_returns_to_uniform_mode() {
        let mut ac = AnimationController::new_from_delays(3, vec![100, 100, 100]);
        ac.set_delays(Vec::new());
        assert!(!ac.has_per_frame_delays());
        ac.play();
        ac.advance(100);
        assert_eq!(ac.current_index(), 1, "uniform fallback drives timing");
    }

    #[test]
    fn delay_ms_returns_raw_per_frame_value() {
        let ac = AnimationController::new_from_delays(2, vec![0, 5000]);
        assert_eq!(ac.delay_ms(0), 0);
        assert_eq!(ac.delay_ms(1), 5000);
    }
}
