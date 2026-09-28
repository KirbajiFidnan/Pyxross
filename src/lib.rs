//! Pyxross Workshop — region-based pixel art editor (Pyxel Edit clone).
//!
//! Layer organization (ARCHITECTURE.md §1):
//! - [`core`] — pure logic, NO UI dependencies (egui/wgpu/winit). Headless-testable.
//! - [`render`] — wgpu: canvas upload, chunk draw, overlays, projection quad pass.
//! - [`platform`] — OS/platform backends (SCTK layer-shell, winit projections).
//! - [`ui`] — egui panels, canvas widget, theme system, preview manager.
//! - [`input`], [`io`] — tool logic/gestures; .pyxross format & PNG import/export.
//!
//! R0 milestone status: workspace skeleton + core types + window bootstrap (D66)
//! + projection scaffold (D65). See docs/ROADMAP.md.

pub mod core;
pub mod input;
pub mod io;
pub mod platform;
pub mod render;
pub mod ui;
