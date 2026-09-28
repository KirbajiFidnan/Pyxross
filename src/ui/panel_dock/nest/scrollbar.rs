//! Custom skinned scrollbar geometry for the static nest.
//!
//! The geometry is pure and unit-tested; painting and drag interaction live in
//! [`super::statik`]. The static nest reuses egui's scroll measurement for
//! overflow and offset clamping, but draws its own bars with the atlas skins
//! (`scrollbar_track` / `scrollbar_handle`) and a flat token fallback.

use egui::{pos2, vec2, Id, Rect, Sense, Ui};

use super::super::skin::PanelChrome;
use crate::ui::theme::SkinState;

/// Minimum thumb length in points, so a tiny viewport fraction still yields a
/// grabbable thumb.
pub const MIN_THUMB: f32 = 16.0;

/// Width of a scrollbar track in points.
pub const BAR_SIZE: f32 = 10.0;

/// Geometry of one scrollbar thumb along its track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbGeometry {
    /// Thumb length along the track axis.
    pub thumb_len: f32,
    /// Thumb start position along the track axis.
    pub thumb_pos: f32,
}

/// Computes the thumb geometry for one axis.
///
/// Returns `None` when the content fits the viewport (no overflow → no bar) or
/// the track is degenerate.
///
/// `thumb_len = clamp(viewport/content * track_len, MIN_THUMB, track_len)`;
/// `thumb_pos = offset / max(1, content - viewport) * (track_len - thumb_len)`,
/// clamped so the thumb never leaves the track.
pub fn thumb_geometry(
    viewport: f32,
    content: f32,
    offset: f32,
    track_len: f32,
) -> Option<ThumbGeometry> {
    if content <= viewport || track_len < MIN_THUMB {
        return None;
    }
    let thumb_len = (viewport / content * track_len).clamp(MIN_THUMB, track_len);
    let travel = (track_len - thumb_len).max(1.0);
    let max_offset = (content - viewport).max(1.0);
    let thumb_pos = (offset / max_offset * travel).clamp(0.0, travel);
    Some(ThumbGeometry {
        thumb_len,
        thumb_pos,
    })
}

/// Maps a thumb position along the track back to a scroll offset.
///
/// The inverse of [`thumb_geometry`]'s position rule, clamped to the valid
/// offset range. Used when the user drags a thumb.
pub fn offset_from_thumb_pos(
    thumb_pos: f32,
    thumb_len: f32,
    track_len: f32,
    content: f32,
    viewport: f32,
) -> f32 {
    let travel = (track_len - thumb_len).max(1.0);
    let max_offset = (content - viewport).max(1.0);
    (thumb_pos / travel * max_offset).clamp(0.0, max_offset)
}

/// Paints the vertical and horizontal bars for `viewport` given the content
/// size and the shared offset, and handles thumb dragging.
///
/// Each axis shows a bar only when its content overflows. The track is the
/// `scrollbar_track` skin center-stretched along the axis; the thumb is the
/// `scrollbar_handle` skin. Without skins, the track falls back to the
/// `panel_border` token and the thumb to `selection_bg_fill`.
pub fn paint_bars(
    ui: &mut Ui,
    chrome: &PanelChrome,
    viewport: Rect,
    content_size: egui::Vec2,
    offset: &mut egui::Vec2,
    id: Id,
) {
    // Vertical bar along the right edge.
    if let Some(geo) = thumb_geometry(
        viewport.height(),
        content_size.y,
        offset.y,
        viewport.height(),
    ) {
        let track = Rect::from_min_size(
            pos2(viewport.right() - BAR_SIZE, viewport.top()),
            vec2(BAR_SIZE, viewport.height()),
        );
        paint_track(ui, chrome, track);
        let thumb_rect = Rect::from_min_size(
            pos2(track.left(), track.top() + geo.thumb_pos),
            vec2(BAR_SIZE, geo.thumb_len),
        );
        let response = ui.interact(thumb_rect, id.with("v-thumb"), Sense::drag());
        if response.dragged()
            && chrome
                .capture
                .handles_buttons(chrome.surface, response.drag_started())
        {
            let delta = ui.input(|input| input.pointer.delta());
            let new_pos =
                (geo.thumb_pos + delta.y).clamp(0.0, (track.height() - geo.thumb_len).max(0.0));
            offset.y = offset_from_thumb_pos(
                new_pos,
                geo.thumb_len,
                track.height(),
                content_size.y,
                viewport.height(),
            );
        }
        paint_thumb(ui, chrome, thumb_rect, thumb_state(response));
    }

    // Horizontal bar along the bottom edge.
    if let Some(geo) = thumb_geometry(viewport.width(), content_size.x, offset.x, viewport.width())
    {
        let track = Rect::from_min_size(
            pos2(viewport.left(), viewport.bottom() - BAR_SIZE),
            vec2(viewport.width(), BAR_SIZE),
        );
        paint_track(ui, chrome, track);
        let thumb_rect = Rect::from_min_size(
            pos2(track.left() + geo.thumb_pos, track.top()),
            vec2(geo.thumb_len, BAR_SIZE),
        );
        let response = ui.interact(thumb_rect, id.with("h-thumb"), Sense::drag());
        if response.dragged()
            && chrome
                .capture
                .handles_buttons(chrome.surface, response.drag_started())
        {
            let delta = ui.input(|input| input.pointer.delta());
            let new_pos =
                (geo.thumb_pos + delta.x).clamp(0.0, (track.width() - geo.thumb_len).max(0.0));
            offset.x = offset_from_thumb_pos(
                new_pos,
                geo.thumb_len,
                track.width(),
                content_size.x,
                viewport.width(),
            );
        }
        paint_thumb(ui, chrome, thumb_rect, thumb_state(response));
    }
}

fn thumb_state(response: egui::Response) -> SkinState {
    if !response.enabled() {
        SkinState::Disabled
    } else if response.is_pointer_button_down_on() {
        SkinState::Pressed
    } else if response.hovered() {
        SkinState::Hover
    } else {
        SkinState::Normal
    }
}

/// Paints a track: the `scrollbar_track` skin center-stretched along the axis,
/// else the flat `panel_border` fill.
fn paint_track(ui: &mut Ui, chrome: &PanelChrome, track: Rect) {
    if let Some((skin, atlas)) = chrome.skin("scrollbar_track") {
        chrome.paint_nine_slice(ui.painter(), skin, atlas, SkinState::Normal, track, 0.0);
    } else {
        ui.painter()
            .rect_filled(track, 0.0, chrome.colors.panel_border32());
    }
}

/// Paints a thumb: the `scrollbar_handle` skin center-stretched, else the flat
/// `selection_bg_fill` fill.
fn paint_thumb(ui: &mut Ui, chrome: &PanelChrome, thumb: Rect, state: SkinState) {
    if let Some((skin, atlas)) = chrome.skin("scrollbar_handle") {
        chrome.paint_nine_slice(ui.painter(), skin, atlas, state, thumb, 0.0);
    } else {
        ui.painter()
            .rect_filled(thumb, 0.0, chrome.colors.selection_bg_fill32());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_bar_when_content_fits() {
        assert_eq!(thumb_geometry(300.0, 200.0, 0.0, 300.0), None);
        assert_eq!(thumb_geometry(300.0, 300.0, 0.0, 300.0), None);
        assert_eq!(thumb_geometry(300.0, 400.0, 0.0, 0.0), None);
    }

    #[test]
    fn thumb_len_scales_with_the_viewport_fraction() {
        // Content twice the viewport: thumb is half the track.
        let geo = thumb_geometry(200.0, 400.0, 0.0, 200.0).unwrap();
        assert_eq!(geo.thumb_len, 100.0);
        assert_eq!(geo.thumb_pos, 0.0);
    }

    #[test]
    fn thumb_len_never_below_minimum() {
        // Content 100x the viewport: raw thumb would be 2px; clamped to MIN_THUMB.
        let geo = thumb_geometry(200.0, 20_000.0, 0.0, 200.0).unwrap();
        assert_eq!(geo.thumb_len, MIN_THUMB);
    }

    #[test]
    fn thumb_len_never_exceeds_the_track() {
        let geo = thumb_geometry(200.0, 201.0, 0.0, 200.0).unwrap();
        assert!(geo.thumb_len <= 200.0);
    }

    #[test]
    fn thumb_pos_tracks_the_offset_ratio() {
        // Content 2x viewport, track 200: travel = 100.
        let geo = thumb_geometry(200.0, 400.0, 100.0, 200.0).unwrap();
        assert_eq!(geo.thumb_pos, 50.0);
        let geo = thumb_geometry(200.0, 400.0, 200.0, 200.0).unwrap();
        assert_eq!(geo.thumb_pos, 100.0);
    }

    #[test]
    fn thumb_pos_clamps_to_the_track() {
        let geo = thumb_geometry(200.0, 400.0, 9999.0, 200.0).unwrap();
        assert_eq!(geo.thumb_pos, 100.0);
        let geo = thumb_geometry(200.0, 400.0, -50.0, 200.0).unwrap();
        assert_eq!(geo.thumb_pos, 0.0);
    }

    #[test]
    fn offset_from_thumb_pos_inverts_the_position_rule() {
        let viewport = 200.0;
        let content = 400.0;
        let track = 200.0;
        let geo = thumb_geometry(viewport, content, 100.0, track).unwrap();
        let offset = offset_from_thumb_pos(geo.thumb_pos, geo.thumb_len, track, content, viewport);
        assert!(
            (offset - 100.0).abs() < 1e-3,
            "expected 100.0, got {offset}"
        );
    }

    #[test]
    fn offset_from_thumb_pos_clamps() {
        let offset = offset_from_thumb_pos(9999.0, 100.0, 200.0, 400.0, 200.0);
        assert_eq!(offset, 200.0);
        let offset = offset_from_thumb_pos(-10.0, 100.0, 200.0, 400.0, 200.0);
        assert_eq!(offset, 0.0);
    }
}
