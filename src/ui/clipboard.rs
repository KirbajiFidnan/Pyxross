//! OS image clipboard bridge (Task G1, Part B; G3 finding #6).
//!
//! Thin, error-tolerant wrappers over [`arboard`] for the RGBA8 image
//! clipboard used by copy/paste. Every failure — no clipboard service, no
//! image on the clipboard, an unsupported format, a size mismatch — degrades
//! to `None` / `false`; none of these helpers ever panic.
//!
//! ## Persistent owner (Wayland owner lifetime)
//!
//! On Wayland an `arboard::Clipboard` connection *is* the clipboard owner: the
//! selection is served from that connection for as long as it lives, and the
//! compositor drops it the moment the connection dies. A short-lived handle
//! that is dropped right after `set_image` therefore loses the copied image as
//! soon as the call returns. [`OsClipboard`] keeps a single handle alive across
//! frames (created lazily on first use) so a copy persists until the next one.
//!
//! ## Non-blocking reads
//!
//! `arboard::Clipboard::get_image()` can block for seconds on Linux (the
//! toolkit negotiates with the compositor / X11 selection owner). The paste
//! flow therefore never calls it on the render thread: it spawns a worker that
//! owns its OWN [`read_image`] handle and polls [`poll_read`] once per frame.
//!
//! `arboard` is already pulled in transitively by `egui-winit` (with the
//! `image-data` feature), so declaring it directly adds no new crate to the
//! build while letting the UI read/write OS images without going through the
//! egui frame output.
//!
//! Core purity note: this module lives in the UI layer, so using an OS crate
//! here does not violate the `src/core` purity rule.

use std::sync::mpsc::{Receiver, TryRecvError};

/// Decoded RGBA8 image: `(pixels, width, height)`, row-major, stride `w * 4`.
pub type ClipboardImage = (Vec<u8>, usize, usize);

/// The receiving end of an in-flight background clipboard read.
///
/// A `None` payload means "the read finished but there is no image" (the
/// paste flow then falls back to the internal project clipboard).
pub type PasteReadReceiver = Receiver<Option<ClipboardImage>>;

/// A single long-lived owner of the OS image clipboard.
///
/// Holds the `arboard::Clipboard` connection for the lifetime of the `App` so
/// Wayland keeps serving the image we wrote. The handle is created lazily on
/// first use and reused by every subsequent [`set_image`](Self::set_image) /
/// [`take_image`](Self::take_image) call; creation failures leave it `None`
/// and every method degrades gracefully.
#[derive(Default)]
pub struct OsClipboard {
    handle: Option<arboard::Clipboard>,
}

impl OsClipboard {
    /// Create an empty owner. The OS handle is opened lazily on first use so
    /// constructing the `App` never touches the clipboard service.
    pub const fn new() -> Self {
        Self { handle: None }
    }

    /// The persistent handle, created on first access. `None` when the host
    /// has no clipboard service (headless, no display, unsupported platform).
    fn handle(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.handle.is_none() {
            self.handle = arboard::Clipboard::new().ok();
        }
        self.handle.as_mut()
    }

    /// Write an RGBA8 image (`w * h * 4` bytes, row-major) to the OS clipboard.
    ///
    /// The persistent handle is reused (and kept alive after this returns) so
    /// the copied image survives — required for the Wayland clipboard owner.
    /// Returns `false` on any error (malformed buffer, no clipboard service) —
    /// never panics.
    pub fn set_image(&mut self, rgba: &[u8], w: usize, h: usize) -> bool {
        let Some(expected) = w.checked_mul(h).and_then(|n| n.checked_mul(4)) else {
            return false;
        };
        if rgba.len() != expected {
            return false;
        }
        let Some(clipboard) = self.handle() else {
            return false;
        };
        let image = arboard::ImageData {
            width: w,
            height: h,
            bytes: std::borrow::Cow::Borrowed(rgba),
        };
        clipboard.set_image(image).is_ok()
    }

    /// Read the OS clipboard as RGBA8 `(pixels, width, height)` through the
    /// persistent handle.
    ///
    /// NOTE: `get_image()` may block for seconds; the UI paste flow calls this
    /// only through the worker-thread [`read_image`] entry point, never on the
    /// render thread. Returns `None` on any error (no clipboard service,
    /// empty/non-image clipboard, unsupported format) or when the buffer does
    /// not match `width * height * 4`.
    pub fn take_image(&mut self) -> Option<ClipboardImage> {
        let clipboard = self.handle()?;
        let image = clipboard.get_image().ok()?;
        validate_image(image.bytes.into_owned(), image.width, image.height)
    }
}

/// Read the OS clipboard as RGBA8 `(pixels, width, height)` with a FRESH,
/// short-lived handle.
///
/// This is the entry point for the background paste thread: it owns its own
/// [`arboard::Clipboard`], created ON the worker thread, so the blocking
/// `get_image()` never touches the render thread. `pixels` is row-major with
/// stride `width * 4`. Returns `None` on any error (no clipboard service,
/// empty/non-image clipboard, unsupported format) or when the returned buffer
/// does not match `width * height * 4`.
pub fn read_image() -> Option<ClipboardImage> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    validate_image(image.bytes.into_owned(), image.width, image.height)
}

/// Shared size validation: non-zero dimensions and exactly `w * h * 4` bytes.
fn validate_image(bytes: Vec<u8>, width: usize, height: usize) -> Option<ClipboardImage> {
    if width == 0 || height == 0 {
        return None;
    }
    let expected = width.checked_mul(height)?.checked_mul(4)?;
    if bytes.len() != expected {
        return None;
    }
    Some((bytes, width, height))
}

/// Poll an in-flight background clipboard read WITHOUT blocking.
///
/// Returns `None` while the read is still pending, or `Some(image_or_none)`
/// once it has completed; the slot is cleared on completion so a finished
/// request is consumed exactly once. A disconnected sender is treated as a
/// completed read with no image (the worker panicked or dropped its sender).
pub fn poll_read(rx: &mut Option<PasteReadReceiver>) -> Option<Option<ClipboardImage>> {
    let receiver = rx.as_ref()?;
    let result = receiver.try_recv();
    match result {
        Ok(image) => {
            *rx = None;
            Some(image)
        }
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            *rx = None;
            Some(None)
        }
    }
}

/// Frames an in-flight paste read may stay pending before it is abandoned.
///
/// `arboard::get_image()` can block or hang indefinitely on Linux/Wayland (no
/// data-control service, an unresponsive selection owner), so the wait is
/// bounded by a FRAME COUNTER — deterministic and testable — rather than a
/// wall-clock timer. At the live ~60 fps repaint cadence 20 frames is ≈0.33 s,
/// long enough for a healthy read to land and short enough that a wedged read
/// does not make Ctrl+V feel dead.
pub const PASTE_WAIT_FRAMES: u32 = 20;

/// An in-flight background clipboard read plus its bounded-wait budget.
///
/// `App` stores this behind its existing `os_paste_rx` field. Bundling the
/// receiver with the elapsed-frame counter keeps the `struct App` field set
/// unchanged (an ownership-inventory test enumerates those fields) while still
/// letting [`poll`](Self::poll) give up deterministically instead of waiting
/// forever on a hung `get_image()`. The worker thread itself is left running —
/// it may still deliver later — but once the deadline passes its receiver is
/// dropped, so the result is discarded and the paste falls back to the internal
/// clipboard.
pub struct PendingPasteRead {
    rx: Option<PasteReadReceiver>,
    waited_frames: u32,
}

impl PendingPasteRead {
    /// Wrap a freshly started receiver.
    pub fn start(rx: PasteReadReceiver) -> Self {
        Self {
            rx: Some(rx),
            waited_frames: 0,
        }
    }

    /// Poll once, without blocking.
    ///
    /// Returns `None` while the read is still pending and within budget, or
    /// `Some(read)` once it resolves:
    /// * `Some(image)` — a decoded image arrived;
    /// * `Some(None)` — a completed no-image read, a disconnected sender
    ///   (worker panicked / dropped its sender, mapped by [`poll_read`] to a
    ///   completed no-image read), or the bounded wait timing out.
    ///
    /// Any `Some` means the request resolved and the caller should drop the
    /// whole value. A timeout yields `Some(None)` so paste's internal-clipboard
    /// fallback runs, and clearing the value releases the "read in flight"
    /// guard in the paste flow.
    pub fn poll(&mut self) -> Option<Option<ClipboardImage>> {
        if let Some(read) = poll_read(&mut self.rx) {
            return Some(read);
        }
        self.waited_frames = self.waited_frames.saturating_add(1);
        if self.waited_frames >= PASTE_WAIT_FRAMES {
            self.rx = None;
            Some(None)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_image_rejects_size_mismatch_without_touching_the_clipboard() {
        // A buffer that does not match `w * h * 4` must be rejected up front,
        // before any clipboard service is contacted.
        let mut clipboard = OsClipboard::new();
        assert!(!clipboard.set_image(&[0u8; 15], 2, 2));
        assert!(!clipboard.set_image(&[], 1, 1));
    }

    #[test]
    fn set_image_rejects_overflowing_dimensions() {
        let mut clipboard = OsClipboard::new();
        assert!(!clipboard.set_image(&[], usize::MAX, 2));
    }

    #[test]
    fn read_image_is_error_tolerant() {
        // In a headless test environment there is no clipboard service; the
        // call must return `None` instead of panicking. (On a developer
        // machine with a clipboard it may return `Some`, which is fine too.)
        let _ = read_image();
    }

    #[test]
    fn take_image_is_error_tolerant() {
        // Same tolerance contract through the persistent-owner handle.
        let mut clipboard = OsClipboard::new();
        let _ = clipboard.take_image();
    }

    #[test]
    fn poll_read_is_pending_until_a_result_arrives() {
        // No send yet → still pending, and the receiver is kept for a retry.
        let (tx, rx) = std::sync::mpsc::channel::<Option<ClipboardImage>>();
        let mut pending = Some(rx);
        assert_eq!(poll_read(&mut pending), None);
        assert!(pending.is_some(), "a pending read must not be cleared");

        // A decoded image arrives → returned once, then the slot is cleared.
        let image: ClipboardImage = (vec![1u8, 2, 3, 4], 1, 1);
        tx.send(Some(image.clone())).expect("send image");
        assert_eq!(poll_read(&mut pending), Some(Some(image)));
        assert!(pending.is_none(), "a completed read must be consumed");

        // A completed no-image read → `Some(None)`.
        let (tx, rx) = std::sync::mpsc::channel::<Option<ClipboardImage>>();
        tx.send(None).expect("send no-image");
        let mut pending = Some(rx);
        assert_eq!(poll_read(&mut pending), Some(None));

        // No request in flight → always `None` (nothing to poll).
        let mut none: Option<PasteReadReceiver> = None;
        assert_eq!(poll_read(&mut none), None);
    }

    #[test]
    fn poll_read_treats_a_dropped_sender_as_a_completed_read() {
        let (tx, rx) = std::sync::mpsc::channel::<Option<ClipboardImage>>();
        drop(tx);
        let mut pending = Some(rx);
        assert_eq!(poll_read(&mut pending), Some(None));
        assert!(pending.is_none());
    }

    #[test]
    fn pending_paste_read_is_pending_until_the_deadline_then_times_out() {
        // A never-sending sender keeps the channel connected; `_tx` must stay
        // alive so the read is Empty rather than Disconnected.
        let (_tx, rx) = std::sync::mpsc::channel::<Option<ClipboardImage>>();
        let mut pending = PendingPasteRead::start(rx);
        for _ in 0..PASTE_WAIT_FRAMES - 1 {
            assert_eq!(pending.poll(), None, "still pending before the deadline");
        }
        // The deadline frame resolves as a completed no-image read so the
        // caller falls back to the internal clipboard.
        assert_eq!(pending.poll(), Some(None));
    }

    #[test]
    fn pending_paste_read_prefers_a_completed_result_over_the_deadline() {
        let (tx, rx) = std::sync::mpsc::channel::<Option<ClipboardImage>>();
        let mut pending = PendingPasteRead::start(rx);
        let image: ClipboardImage = (vec![1u8, 2, 3, 4], 1, 1);
        tx.send(Some(image.clone())).expect("send image");
        assert_eq!(pending.poll(), Some(Some(image)));
    }

    #[test]
    fn pending_paste_read_maps_a_disconnected_sender_to_no_image() {
        let (tx, rx) = std::sync::mpsc::channel::<Option<ClipboardImage>>();
        drop(tx);
        let mut pending = PendingPasteRead::start(rx);
        assert_eq!(pending.poll(), Some(None));
    }
}
