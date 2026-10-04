//! Core layer — pure logic, headless-testable.
//!
//! HARD RULE (CONTEXT.md §4.2): this module tree must never import egui, wgpu,
//! winit, or any UI crate. Enforced by `scripts/check-core-purity.sh`.

pub mod anim;
pub mod brush;
pub mod buffer;
pub mod camera;
pub mod clip;
pub mod clipboard;
pub mod color;
pub mod document;
pub mod fill;
pub mod group_commands;
pub mod layer_commands;
pub mod math;
pub mod merge;
pub mod model;
pub mod palette;
pub mod png_codec;
pub mod projection;
pub mod select;
pub mod selection_ops;
pub mod stroke_command;
pub mod tile_edit;
pub mod tilemap;
pub mod transform;
pub mod undo;

pub use clip::PixelClip;
pub use document::{Document, EditorSettings, FrameDoc, LayerDoc, RegionDoc, SequenceDoc};
pub use palette::{Palette, PaletteError};
