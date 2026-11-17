//! Shared per-frame state cells for the animation dock panels.
//!
//! The App writes a typed snapshot of the project's animation list into `list`
//! before rendering the dock and drains `events` after rendering; the panels
//! are pure views that never mutate the model. The settings panel's editable
//! fields live in persistent buffers here so typed text survives across frames,
//! and the transport/onion/loop flags mirror the App's playback state.

use std::cell::RefCell;
use std::rc::Rc;

/// One rendered animation-list row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationRow {
    /// Index of the animation in the project's animation list.
    pub index: usize,
    /// Display name.
    pub name: String,
    /// Number of frames the animation spans.
    pub frame_count: usize,
    /// Whether this is the active animation (exactly one row should be).
    pub active: bool,
}

/// Snapshot of the animation list for one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationListView {
    /// The project's animations in playback order.
    pub animations: Vec<AnimationRow>,
    /// Whether the active animation may be removed right now (the `-` button's
    /// enabled state; the App keeps at least one animation).
    pub remove_enabled: bool,
}

/// An animation-panel gesture emitted by the list or the settings panel.
///
/// The App shell owns the project's animations and playback state; these pure
/// views only ever push events into the host cells.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationPanelEvent {
    /// Make the animation at this index active.
    Select(usize),
    /// Append a new animation.
    Add,
    /// Remove the animation at this index.
    Remove(usize),
    /// Open the floating settings panel for the animation at this index.
    OpenSettings(usize),
    /// Open the "set frames" tool for the active animation (frame-region editing).
    SetFramesTool,
    /// Commit a new name for the animation at `index`.
    SetName { index: usize, name: String },
    /// Commit a delay multiplier for the animation at `index`.
    SetDelayMultiplier { index: usize, mult: f64 },
    /// Set the loop flag for the animation at `index`.
    SetLoop { index: usize, on: bool },
    /// Set the shared per-frame delay in milliseconds.
    SetFrameDelay { ms: u32 },
    /// Toggle the previous-frame onion skin.
    OnionPrev(bool),
    /// Toggle the next-frame onion skin.
    OnionNext(bool),
    /// Start or resume playback.
    Play,
    /// Stop playback, preserving the current frame.
    Stop,
    /// Stop playback and jump back to frame 0.
    StopReset,
    /// Step to the previous frame.
    PrevFrame,
    /// Step to the next frame.
    NextFrame,
}

/// Shared cells for the animation panels (list, settings, preview).
#[derive(Clone, Default)]
pub struct AnimationPanelHost {
    pub list: Rc<RefCell<AnimationListView>>,
    pub events: Rc<RefCell<Vec<AnimationPanelEvent>>>,
    /// Which animation's floating settings panel is open, if any.
    pub open_settings: Rc<RefCell<Option<usize>>>,
    /// The row the settings panel is editing (name + multiplier + loop snapshot).
    pub settings_row: Rc<RefCell<Option<AnimationRow>>>,
    /// In-progress rename text, kept across frames so typed characters survive.
    pub rename_buffer: Rc<RefCell<String>>,
    /// In-progress delay-multiplier text (parsed to `f64` on edit).
    pub multiplier_buffer: Rc<RefCell<String>>,
    /// The settings panel's loop checkbox state.
    pub settings_loop: Rc<RefCell<bool>>,
    /// Whether playback is currently running.
    pub playing: Rc<RefCell<bool>>,
    /// Whether the active animation loops.
    pub loop_on: Rc<RefCell<bool>>,
    /// Whether the previous-frame onion skin is shown.
    pub onion_prev: Rc<RefCell<bool>>,
    /// Whether the next-frame onion skin is shown.
    pub onion_next: Rc<RefCell<bool>>,
    /// The preview's current frame index.
    pub current_frame: Rc<RefCell<usize>>,
    /// The active animation's frame count.
    pub frame_count: Rc<RefCell<usize>>,
}
