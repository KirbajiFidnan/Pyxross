//! `.pyxross` save-format document — pure data, no I/O.
//!
//! [`Document`] is the on-disk representation of a Pyxross Workshop project
//! (PERSISTENCE.md [D08]). It is a plain data structure: every field is
//! public, the type derives `Serialize`/`Deserialize` for the manifest, and
//! the module imports nothing beyond `std` and `serde`. All filesystem and
//! PNG work lives in [`crate::io`]; this module stays free of `std::fs`,
//! `png`, egui, and every other I/O or UI dependency so the core layer stays
//! pure (ARCHITECTURE.md §8, enforced by `scripts/check-core-purity.sh`).
//!
//! # Layout
//!
//! A saved project is a directory:
//!
//! ```text
//! MyProject.pyxross/
//! ├── manifest.json   — this [`Document`] (layer pixels skipped)
//! └── layers/
//!     └── <layer_id>.png
//! ```
//!
//! `manifest.json` carries the metadata; each layer's pixels live in a
//! full-canvas 8-bit RGBA PNG named after the layer id. Frames and regions
//! are rect references into the sheet — pixel content is never duplicated.
//!
//! # Versioning
//!
//! [`FORMAT_VERSION`] is the current on-disk format version. `manifest.json`
//! stores it under `format_version`; [`crate::io::load_document`] migrates
//! older manifests forward and rejects newer ones.

use serde::{Deserialize, Serialize};

use crate::core::model::BlendMode;

/// Current on-disk format version of the `.pyxross` save format.
///
/// Bump this when the manifest structure changes incompatibly, and add a
/// stepwise migration in [`crate::io::migrate_document`] for the previous
/// version. Loaders reject manifests saved by a newer version.
pub const FORMAT_VERSION: u32 = 2;

/// A complete `.pyxross` project: canvas, layers, regions, frames,
/// sequences, palette, and editor settings.
///
/// This is the pure-data counterpart of the in-memory [`crate::core::model`]
/// types. Later features (File menu, autosave, recovery) construct one from
/// app state and hand it to [`crate::io::save_document`].
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct Document {
    /// Canvas width in pixels.
    pub canvas_width: u32,
    /// Canvas height in pixels.
    pub canvas_height: u32,
    /// Tile size in pixels (the grid snap unit).
    pub tile_size: u32,
    /// Layers, bottom-to-top (index 0 is the bottom layer).
    pub layers: Vec<LayerDoc>,
    /// Named rectangular windows into the sheet.
    pub regions: Vec<RegionDoc>,
    /// Animation frames: a region reference plus a display delay.
    pub frames: Vec<FrameDoc>,
    /// Ordered animation sequences over the frames.
    pub sequences: Vec<SequenceDoc>,
    /// Project palette, straight-alpha RGBA.
    pub palette: Vec<[u8; 4]>,
    /// Editor preferences.
    pub editor: EditorSettings,
}

/// A single layer's metadata plus its pixel buffer.
///
/// `pixels` is row-major straight-alpha RGBA with
/// `len == canvas_width * canvas_height * 4`. It is `#[serde(skip)]`ped so
/// `manifest.json` carries metadata only; [`crate::io::save_document`] writes
/// the pixels to a per-layer PNG and [`crate::io::load_document`] fills them
/// back from that PNG. `pixels` participates in `PartialEq`, so a
/// save→load→compare round-trip is byte-exact.
///
/// Group layers (`is_group == true`) carry an EMPTY `pixels` vector and no
/// PNG on disk; their content comes from compositing their children. The
/// layer tree is stored as a flat DFS pre-order list: a group's descendants
/// immediately follow it, and each layer's `parent` names its group (or
/// `None` for a root-level layer).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct LayerDoc {
    /// Stable layer id; matches the in-memory `LayerId`.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Whether the layer participates in compositing.
    pub visible: bool,
    /// Layer opacity in `0..=1`.
    pub opacity: f32,
    /// Blend mode against the accumulated backdrop.
    pub blend: BlendMode,
    /// Parent group id; `None` for a root-level layer.
    pub parent: Option<u64>,
    /// True for group layers (which carry an empty `pixels` vector).
    pub is_group: bool,
    /// Row-major RGBA8 pixels, `canvas_width * canvas_height * 4` bytes.
    #[serde(skip)]
    pub pixels: Vec<u8>,
}

/// A named rectangular window into the sprite sheet.
///
/// Mirrors [`crate::core::model::Region`] as plain `u32` fields (the sheet
/// is never negative); frames reference regions by [`RegionDoc::id`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RegionDoc {
    /// Stable region id.
    pub id: u64,
    /// Left edge in sheet space.
    pub x: u32,
    /// Top edge in sheet space.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// An animation frame: a region reference plus a display delay.
///
/// Mirrors [`crate::core::model::Frame`]; the frame references the sheet by
/// region id — it never copies pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct FrameDoc {
    /// The region this frame shows.
    pub region_id: u64,
    /// Delay in milliseconds before advancing to the next frame.
    pub delay_ms: u32,
}

/// An ordered animation sequence over the frames.
///
/// Mirrors [`crate::core::model::AnimationSequence`]: frames play in
/// `frame_ids` order, optionally looping.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct SequenceDoc {
    /// Sequence display name.
    pub name: String,
    /// Frame ids in play order.
    pub frame_ids: Vec<u64>,
    /// Whether the sequence loops back to the first frame.
    pub loop_flag: bool,
    /// Free-form tags (e.g. "idle", "attack").
    pub tags: Vec<String>,
}

/// Editor preferences persisted with the document.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EditorSettings {
    /// Autosave interval in minutes.
    pub autosave_interval_min: u32,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            autosave_interval_min: 5,
        }
    }
}
