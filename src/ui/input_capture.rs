//! Gesture ownership for zoom/pan/scroll interactions.
//!
//! One shared [`InputCapture`] decides which surface (the canvas or a docked /
//! floating panel with its nest) owns the current gesture. Ownership is
//! resolved by the [`UiLayer`] contract so the surface the user is actually
//! looking at - the topmost one under the pointer - wins:
//!
//! ```text
//! floating panels (last clicked first) > dock area and its panels > frame UI (reserved) > canvas
//! ```
//!
//! Button gestures (middle-drag pan, scrollbar thumb drags) are owned until the
//! button is released; while owned they keep tracking the pointer even when it
//! leaves the owning panel, and ownership is dropped when the pointer leaves
//! the application window. Wheel and Ctrl+scroll have no release, so they are
//! routed per event to the topmost surface under the pointer.

use std::cell::Cell;

use super::panel_dock::PanelId;

/// A surface that can own a zoom/pan/scroll gesture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceId {
    Canvas,
    /// A docked or floating panel (its nest is the same surface).
    Panel(PanelId),
}

/// Stacking contract, highest priority first. Lower discriminant = closer to
/// the user, i.e. painted later.
///
/// `FrameUi` is a reserved slot for the future frame interface; nothing paints
/// it yet, but it keeps its place in the order so adding it later does not
/// renumber the contract.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UiLayer {
    FloatingPanel,
    Dock,
    FrameUi,
    Canvas,
}

/// True when this frame carries a Ctrl+wheel gesture.
///
/// egui reports the wheel's own modifiers on the event; `InputState::modifiers`
/// only tracks held keys, so a synthetic or fast wheel can miss it.
pub fn ctrl_wheel(ui: &egui::Ui) -> bool {
    ui.input(|input| {
        input.modifiers.ctrl
            || input.events.iter().any(|event| {
                matches!(event, egui::Event::MouseWheel { modifiers, .. } if modifiers.ctrl)
            })
    })
}

/// True when this frame carries a Shift+wheel gesture (the Draw tools' brush
/// resize). Same event-modifier caveat as [`ctrl_wheel`].
pub fn shift_wheel(ui: &egui::Ui) -> bool {
    ui.input(|input| {
        input.modifiers.shift
            || input.events.iter().any(|event| {
                matches!(event, egui::Event::MouseWheel { modifiers, .. } if modifiers.shift)
            })
    })
}

/// Picks the topmost surface among stacked candidates.
///
/// Ties are resolved by iteration order, so callers pass same-layer candidates
/// in their own z order (floating panels: most recently clicked first).
pub fn topmost(candidates: impl IntoIterator<Item = (UiLayer, SurfaceId)>) -> Option<SurfaceId> {
    candidates
        .into_iter()
        .min_by_key(|(layer, _)| *layer)
        .map(|(_, surface)| surface)
}

/// Shared gesture router. Lives in the `App` and is handed to every surface as
/// a shared reference; interior mutability keeps that borrow simple.
#[derive(Debug, Default)]
pub struct InputCapture {
    owner: Cell<Option<SurfaceId>>,
    target: Cell<Option<SurfaceId>>,
}

impl InputCapture {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the topmost surface under the pointer for this frame.
    pub fn set_target(&self, surface: Option<SurfaceId>) {
        self.target.set(surface);
    }

    /// The topmost surface under the pointer this frame.
    pub fn target(&self) -> Option<SurfaceId> {
        self.target.get()
    }

    /// The surface owning the active button gesture, if any.
    pub fn owner(&self) -> Option<SurfaceId> {
        self.owner.get()
    }

    pub fn is_owner(&self, surface: SurfaceId) -> bool {
        self.owner.get() == Some(surface)
    }

    pub fn is_target(&self, surface: SurfaceId) -> bool {
        self.target.get() == Some(surface)
    }

    /// True when a docked / floating panel is the topmost surface under the
    /// pointer this frame, i.e. the pointer sits over panel chrome.
    ///
    /// This is the SINGLE panel mask every surface consults: the App resolves
    /// [`Self::set_target`] once per frame from
    /// [`crate::ui::panel_dock::DockManager::surface_at`] (floating panels over
    /// docked ones, visible panels only) and routes wheel/pan through
    /// [`Self::handles_wheel`] / [`Self::handles_buttons`]. A surface that
    /// processes input outside its own rect — the canvas widget's RAW press
    /// signal and its hover-driven previews — MUST gate on this too, otherwise a
    /// press over a panel leaks to the surface underneath.
    pub fn pointer_over_panel(&self) -> bool {
        matches!(self.target.get(), Some(surface) if surface != SurfaceId::Canvas)
    }

    /// Claims the gesture for `surface`, unless another surface owns it.
    ///
    /// Returns whether `surface` holds the gesture afterwards.
    pub fn claim(&self, surface: SurfaceId) -> bool {
        match self.owner.get() {
            Some(owner) => owner == surface,
            None => {
                self.owner.set(Some(surface));
                true
            }
        }
    }

    /// Ends the active button gesture.
    pub fn release(&self) {
        self.owner.set(None);
    }

    /// Drops ownership when the pointer leaves the application window.
    pub fn release_if_outside(&self, pointer_inside_window: bool) {
        if !pointer_inside_window {
            self.release();
        }
    }

    /// Whether `surface` handles a button gesture this frame.
    ///
    /// An owner keeps the gesture regardless of where the pointer is. A free
    /// gesture is claimed only when it starts (`pressed`) on the topmost
    /// surface under the pointer.
    pub fn handles_buttons(&self, surface: SurfaceId, pressed: bool) -> bool {
        if let Some(owner) = self.owner.get() {
            return owner == surface;
        }
        if !pressed || self.target.get() != Some(surface) {
            return false;
        }
        self.owner.set(Some(surface));
        true
    }

    /// Whether `surface` handles wheel / Ctrl+scroll this frame.
    ///
    /// Routed per event to the topmost surface under the pointer; an active
    /// button gesture keeps wheel input away from every other surface.
    pub fn handles_wheel(&self, surface: SurfaceId) -> bool {
        self.owner.get().is_none() && self.target.get() == Some(surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(raw: u64) -> PanelId {
        PanelId::new(raw)
    }

    #[test]
    fn layer_contract_orders_floating_above_dock_above_frame_above_canvas() {
        assert!(UiLayer::FloatingPanel < UiLayer::Dock);
        assert!(UiLayer::Dock < UiLayer::FrameUi);
        assert!(UiLayer::FrameUi < UiLayer::Canvas);
    }

    #[test]
    fn topmost_picks_the_highest_layer_and_keeps_same_layer_order() {
        let canvas = SurfaceId::Canvas;
        let dock = SurfaceId::Panel(panel(0));
        let floating_a = SurfaceId::Panel(panel(1));
        let floating_b = SurfaceId::Panel(panel(2));

        let picked = topmost([
            (UiLayer::Canvas, canvas),
            (UiLayer::Dock, dock),
            (UiLayer::FloatingPanel, floating_a),
            (UiLayer::FloatingPanel, floating_b),
        ]);
        assert_eq!(picked, Some(floating_a));

        assert_eq!(
            topmost([(UiLayer::Canvas, canvas), (UiLayer::Dock, dock)]),
            Some(dock)
        );
        assert_eq!(topmost([(UiLayer::Canvas, canvas)]), Some(canvas));
        assert_eq!(topmost([]), None);
    }

    #[test]
    fn button_gesture_is_claimed_only_on_the_target_surface() {
        let canvas = SurfaceId::Canvas;
        let nest = SurfaceId::Panel(panel(0));
        let capture = InputCapture::new();
        capture.set_target(Some(canvas));

        assert!(!capture.handles_buttons(nest, true));
        assert_eq!(capture.owner(), None);
        assert!(capture.handles_buttons(canvas, true));
        assert!(capture.is_owner(canvas));
    }

    #[test]
    fn owned_gesture_keeps_tracking_away_from_the_owning_surface() {
        let canvas = SurfaceId::Canvas;
        let nest = SurfaceId::Panel(panel(0));
        let capture = InputCapture::new();
        capture.set_target(Some(nest));
        assert!(capture.handles_buttons(nest, true));

        // Pointer moved off the nest: the nest keeps the gesture, others do not.
        capture.set_target(Some(canvas));
        assert!(capture.handles_buttons(nest, false));
        assert!(!capture.handles_buttons(canvas, false));
        assert!(!capture.handles_buttons(canvas, true));
    }

    #[test]
    fn release_frees_the_gesture_for_the_next_target() {
        let canvas = SurfaceId::Canvas;
        let nest = SurfaceId::Panel(panel(0));
        let capture = InputCapture::new();
        capture.set_target(Some(nest));
        assert!(capture.handles_buttons(nest, true));
        capture.release();

        capture.set_target(Some(canvas));
        assert!(capture.handles_buttons(canvas, true));
    }

    #[test]
    fn pointer_leaving_the_window_drops_ownership() {
        let nest = SurfaceId::Panel(panel(0));
        let capture = InputCapture::new();
        capture.set_target(Some(nest));
        assert!(capture.handles_buttons(nest, true));

        capture.release_if_outside(false);
        assert_eq!(capture.owner(), None);

        capture.release_if_outside(true);
        assert_eq!(capture.owner(), None);
    }

    #[test]
    fn wheel_is_routed_to_the_target_only_while_free() {
        let canvas = SurfaceId::Canvas;
        let nest = SurfaceId::Panel(panel(0));
        let capture = InputCapture::new();
        capture.set_target(Some(nest));

        assert!(capture.handles_wheel(nest));
        assert!(!capture.handles_wheel(canvas));

        // An active button gesture blocks wheel routing entirely.
        capture.claim(canvas);
        assert!(!capture.handles_wheel(nest));
        assert!(!capture.handles_wheel(canvas));
    }

    #[test]
    fn pointer_over_panel_is_the_inverse_of_targeting_the_canvas() {
        let canvas = SurfaceId::Canvas;
        let nest = SurfaceId::Panel(panel(0));
        let capture = InputCapture::new();

        // No target recorded (pointer outside the app frame): nothing masks the
        // canvas, and the canvas is NOT the target either.
        assert!(!capture.pointer_over_panel());
        assert!(!capture.is_target(canvas));

        capture.set_target(Some(canvas));
        assert!(!capture.pointer_over_panel());
        assert!(capture.is_target(canvas));

        capture.set_target(Some(nest));
        assert!(capture.pointer_over_panel());
        assert!(!capture.is_target(canvas));
    }
}
