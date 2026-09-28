//! Pyxross Workshop — application entry point.
//!
//! R0 (F3): manual winit + egui-winit + egui-wgpu bootstrap ("blank window").
//! R0 (F4): projection scaffold — View → New Preview Surface (frameless window).

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() {
    pyxross::ui::run();
}
