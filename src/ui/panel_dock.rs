mod demo;
mod geometry;
pub mod nest;
pub mod preview;
pub mod skin;
mod state;
mod view;

pub use geometry::drag_outside_threshold;
pub use nest::{Component, ComponentPlacement, NestContent, NestMode, NestReset, NestResetHandle};
pub use preview::{PreviewControls, PreviewFeed, PreviewImage};
pub use skin::{AtlasCache, ButtonStyle, PanelChrome, SkinAtlas};
pub use state::{
    DockAction, DockError, DockManager, DockSide, PanelContent, PanelHeaderAction, PanelId,
    PanelMetadata, PanelPlacement, PanelSpec,
};
pub use view::FLOATING_PANEL_CHROME_HEIGHT;
