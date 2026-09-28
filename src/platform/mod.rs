//! Platform/OS backends (allowed: wgpu/winit/egui; never imported by core,
//! never imports `ui`).
//!
//! - `wayland/layer_shell.rs` — SCTK layer-shell backend [D65] (R1, Linux).
//! - `winit_projection.rs` — winit frameless fallback [D65] (R0 scaffold).
