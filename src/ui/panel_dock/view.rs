use egui::{
    pos2, vec2, Align, Align2, FontId, Id, Layout, Order, Pos2, Rect, Sense, Stroke, UiBuilder,
    Vec2, Window,
};

use crate::ui::input_capture::SurfaceId;
use crate::ui::theme::SkinState;

use super::geometry::drag_outside_threshold;
use super::nest::{nest_is_usable, nest_viewport, NEST_PADDING};
use super::skin::PanelChrome;
use super::state::{DockAction, DockManager, DockSide, PanelHeaderAction, PanelId, PanelPlacement};

const HEADER_HEIGHT: f32 = 28.0;
const HANDLE_SIZE: f32 = 6.0;
/// Inset between a docked panel's skinned frame border and its content.
const FRAME_PADDING: f32 = 2.0;
/// Margin between a floating window's rect and its content, in points
/// (`show_floating`'s frame margin).
const FLOATING_WINDOW_MARGIN: f32 = 4.0;
/// Vertical chrome a floating panel adds around its nest content: the window
/// margin, the header band, the skinned frame padding and the nest padding on
/// both sides. Lets a fixed-size panel spec size its window to its content.
pub const FLOATING_PANEL_CHROME_HEIGHT: f32 =
    2.0 * FLOATING_WINDOW_MARGIN + HEADER_HEIGHT + 2.0 * FRAME_PADDING + 2.0 * NEST_PADDING;
/// The nest's ScrollArea expands its content rect to at least this extent on
/// the scroll axis (`ScrollArea::min_scrolled_size` default), so the nest
/// viewport can extend below a short panel; the visibility rule must keep the
/// viewport inside the panel rect.
const NEST_MIN_SCROLLED_EXTENT: f32 = 64.0;

/// Layer id of a floating panel's egui window; the ordering step raises these.
fn floating_window_id(id: PanelId) -> Id {
    Id::new(("panel-dock-floating", id.raw()))
}

/// Whether a panel is painted inside a dock strip or as a floating window.
///
/// Docked panels paint their own `panel_bg` nine-slice frame over their full
/// rect; floating panels get the frame from their `Window` already.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PanelScope {
    Docked,
    Floating,
}

/// The rect a docked panel's nest content receives: the panel rect below the
/// header band, inset by the skinned frame padding on every side. Mirrors
/// `paint_panel`'s body computation.
fn panel_nest_body_rect(panel_rect: Rect) -> Rect {
    let below_header = Rect::from_min_max(
        pos2(panel_rect.left(), panel_rect.top() + HEADER_HEIGHT),
        panel_rect.max,
    );
    below_header.shrink(FRAME_PADDING)
}

/// A docked panel is visible iff its nest content is usable and stays inside
/// the panel: the nest viewport derived from the panel's body rect must clear
/// `NEST_MIN_USABLE_EXTENT` on both axes and fit within the panel rect.
///
/// The skinned frame padding is always subtracted, even when the theme has no
/// skins: a flat theme gives the nest *more* room, so this can only hide a
/// panel slightly early — never leave panel chrome with hidden content.
fn panel_is_visible(panel_rect: Rect) -> bool {
    let body = panel_nest_body_rect(panel_rect);
    let full = Rect::from_min_max(
        body.min,
        pos2(
            body.max.x,
            body.min.y + NEST_MIN_SCROLLED_EXTENT.max(body.height()),
        ),
    );
    let viewport = nest_viewport(full);
    nest_is_usable(viewport) && panel_rect.contains_rect(viewport)
}

impl DockManager {
    pub fn show(&mut self, ctx: &egui::Context, chrome: &PanelChrome) {
        egui::Area::new(Id::new("panel-dock-demo"))
            .order(Order::Foreground)
            .movable(false)
            .show(ctx, |ui| {
                self.show_inside(ui, chrome);
            });
    }

    pub fn show_inside(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let viewport = ui.max_rect();
        ui.set_min_size(viewport.size());
        let ctx = ui.ctx().clone();
        self.show_docks(ui, viewport, chrome);
        self.show_floating(&ctx, viewport, chrome);
        self.order_floating(&ctx);
    }

    /// The rect a dock side occupies inside `viewport`.
    fn dock_rect(&self, side: DockSide, viewport: Rect) -> Rect {
        let left_edge = viewport.left() + self.dock_extent(DockSide::Left);
        let right_edge = viewport.right() - self.dock_extent(DockSide::Right);
        match side {
            DockSide::Left => Rect::from_min_max(viewport.min, pos2(left_edge, viewport.bottom())),
            DockSide::Right => Rect::from_min_max(pos2(right_edge, viewport.top()), viewport.max),
            DockSide::Bottom => {
                // The bottom band spans between the side docks; when their
                // extents overlap (tiny viewport) the band must not invert.
                let left = left_edge.min(right_edge);
                let right = right_edge.max(left_edge);
                let top =
                    (viewport.bottom() - self.dock_extent(DockSide::Bottom)).max(viewport.top());
                Rect::from_min_max(pos2(left, top), pos2(right, viewport.bottom()))
            }
        }
    }

    /// The panel under `point`, resolved by the z-order contract: floating
    /// panels (most recently clicked first) over docked panels. `None` means
    /// the point belongs to the canvas.
    pub fn surface_at(&self, point: Pos2, viewport: Rect) -> Option<PanelId> {
        for id in self.floating_order().iter().rev() {
            if self
                .floating_rect(*id)
                .is_some_and(|rect| rect.contains(point))
            {
                return Some(*id);
            }
        }
        for side in [DockSide::Left, DockSide::Right, DockSide::Bottom] {
            let dock = self.dock_rect(side, viewport);
            if dock.contains(point) {
                return self
                    .panel_rects(side, dock)
                    .into_iter()
                    .find(|(_, panel_rect)| {
                        panel_is_visible(*panel_rect) && panel_rect.contains(point)
                    })
                    .map(|(id, _)| id);
            }
        }
        None
    }

    /// Raises the most recently used floating panel. egui lifts marked layers
    /// above unmarked ones but keeps their relative order, so exactly one layer
    /// is marked per frame - the top of the MRU stack.
    fn order_floating(&self, ctx: &egui::Context) {
        if let Some(top) = self.floating_order().last() {
            ctx.move_to_top(egui::LayerId::new(Order::Middle, floating_window_id(*top)));
        }
    }

    fn show_docks(&mut self, ui: &mut egui::Ui, viewport: Rect, chrome: &PanelChrome) {
        let left_rect = self.dock_rect(DockSide::Left, viewport);
        let right_rect = self.dock_rect(DockSide::Right, viewport);
        let bottom_rect = self.dock_rect(DockSide::Bottom, viewport);
        self.paint_dock(ui, DockSide::Left, left_rect, chrome);
        self.paint_dock(ui, DockSide::Right, right_rect, chrome);
        self.paint_dock(ui, DockSide::Bottom, bottom_rect, chrome);

        let left_handle = Rect::from_min_max(
            pos2(left_rect.right() - HANDLE_SIZE, left_rect.top()),
            pos2(left_rect.right() + HANDLE_SIZE, left_rect.bottom()),
        );
        let left_response = ui.interact(
            left_handle,
            Id::new("panel-dock-left-resize"),
            Sense::drag(),
        );
        if left_response.dragged() {
            let delta = ui.ctx().input(|input| input.pointer.delta());
            self.resize_dock_area(DockSide::Left, delta.x, viewport.size());
        }
        paint_resize_handle(ui, left_handle, chrome);

        let right_handle = Rect::from_min_max(
            pos2(right_rect.left() - HANDLE_SIZE, right_rect.top()),
            pos2(right_rect.left() + HANDLE_SIZE, right_rect.bottom()),
        );
        let right_response = ui.interact(
            right_handle,
            Id::new("panel-dock-right-resize"),
            Sense::drag(),
        );
        if right_response.dragged() {
            let delta = ui.ctx().input(|input| input.pointer.delta());
            self.resize_dock_area(DockSide::Right, -delta.x, viewport.size());
        }
        paint_resize_handle(ui, right_handle, chrome);

        let bottom_handle = Rect::from_min_max(
            pos2(bottom_rect.left(), bottom_rect.top() - HANDLE_SIZE),
            pos2(bottom_rect.right(), bottom_rect.top() + HANDLE_SIZE),
        );
        let bottom_response = ui.interact(
            bottom_handle,
            Id::new("panel-dock-bottom-resize"),
            Sense::drag(),
        );
        if bottom_response.dragged() {
            let delta = ui.ctx().input(|input| input.pointer.delta());
            self.resize_dock_area(DockSide::Bottom, -delta.y, viewport.size());
        }
        paint_resize_handle(ui, bottom_handle, chrome);
    }

    fn paint_dock(&mut self, ui: &mut egui::Ui, side: DockSide, rect: Rect, chrome: &PanelChrome) {
        if !chrome.paint_slice(ui.painter(), "panel_bg", SkinState::Normal, rect) {
            ui.painter()
                .rect_filled(rect, 2.0, chrome.colors.panel_bg32());
            ui.painter().rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, chrome.colors.panel_border32()),
                egui::StrokeKind::Inside,
            );
        }
        let panel_rects = self.panel_rects(side, rect);
        for (index, (id, panel_rect)) in panel_rects.iter().copied().enumerate() {
            if !panel_is_visible(panel_rect) {
                continue;
            }
            self.paint_panel_in_rect(ui, id, panel_rect, chrome);
            if index + 1 < panel_rects.len() {
                let handle_rect = match side {
                    DockSide::Left => Rect::from_min_max(
                        pos2(panel_rect.left(), panel_rect.bottom() - HANDLE_SIZE),
                        pos2(panel_rect.right(), panel_rect.bottom() + HANDLE_SIZE),
                    ),
                    DockSide::Right => Rect::from_min_max(
                        pos2(panel_rect.left(), panel_rect.bottom() - HANDLE_SIZE),
                        pos2(panel_rect.right(), panel_rect.bottom() + HANDLE_SIZE),
                    ),
                    DockSide::Bottom => Rect::from_min_max(
                        pos2(panel_rect.right() - HANDLE_SIZE, panel_rect.top()),
                        pos2(panel_rect.right() + HANDLE_SIZE, panel_rect.bottom()),
                    ),
                };
                let response = ui.interact(
                    handle_rect,
                    Id::new(("panel-dock-panel-resize", id.raw())),
                    Sense::drag(),
                );
                if response.dragged() {
                    let delta = ui.ctx().input(|input| input.pointer.delta());
                    let axis_delta = match side {
                        DockSide::Left => delta.y,
                        DockSide::Right => delta.y,
                        DockSide::Bottom => delta.x,
                    };
                    self.resize_dock_panel(id, axis_delta, rect.size());
                }
                paint_resize_handle(ui, handle_rect, chrome);
            }
        }
        self.paint_drop_preview(ui, side, rect, chrome);
    }

    fn paint_panel_in_rect(
        &mut self,
        ui: &mut egui::Ui,
        id: PanelId,
        rect: Rect,
        chrome: &PanelChrome,
    ) {
        ui.scope_builder(
            UiBuilder::new()
                .id(Id::new(("panel-dock-panel", id.raw())))
                .max_rect(rect),
            |panel_ui| {
                self.paint_panel(panel_ui, id, chrome, PanelScope::Docked);
            },
        );
    }

    fn paint_panel(
        &mut self,
        ui: &mut egui::Ui,
        id: PanelId,
        chrome: &PanelChrome,
        scope: PanelScope,
    ) {
        let Some(metadata) = self.metadata(id) else {
            return;
        };
        let title = metadata.title.clone();
        let header_action = self.header_action(id);
        let width = ui.available_width().max(1.0);
        let skinned_frame = scope == PanelScope::Docked
            && chrome.paint_slice(ui.painter(), "panel_bg", SkinState::Normal, ui.max_rect());
        let (header_rect, response) =
            ui.allocate_exact_size(vec2(width, HEADER_HEIGHT), Sense::drag());
        let mut popped_out = false;
        let mut pop_out_rect = None;
        let header_state = if response.dragged() {
            SkinState::Pressed
        } else {
            SkinState::Normal
        };
        if !chrome.paint_slice(ui.painter(), "panel_header", header_state, header_rect) {
            ui.painter()
                .rect_filled(header_rect, 2.0, chrome.colors.panel_header_bg32());
        }
        // The title is painted centered in the header band; the action button
        // stays pinned to the right.
        ui.painter().text(
            header_rect.center(),
            Align2::CENTER_CENTER,
            title,
            FontId::proportional(13.0),
            ui.visuals().text_color(),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(header_rect.shrink(4.0))
                .layout(Layout::right_to_left(Align::Center)),
            |header| {
                if let Some(action) = header_action {
                    let label = action.label();
                    let clicked = if chrome.skin("button").is_some() {
                        let (button_rect, button_response) = header.allocate_exact_size(
                            skinned_button_size(label, header),
                            Sense::click(),
                        );
                        let state = if !button_response.enabled() {
                            SkinState::Disabled
                        } else if button_response.is_pointer_button_down_on() {
                            SkinState::Pressed
                        } else if button_response.hovered() {
                            SkinState::Hover
                        } else {
                            SkinState::Normal
                        };
                        chrome.paint_slice(header.painter(), "button", state, button_rect);
                        header.painter().text(
                            button_rect.center(),
                            Align2::CENTER_CENTER,
                            label,
                            FontId::proportional(12.0),
                            header.visuals().text_color(),
                        );
                        pop_out_rect = Some(button_rect);
                        button_response.clicked()
                    } else {
                        let button = header.small_button(label);
                        pop_out_rect = Some(button.rect);
                        button.clicked()
                    };
                    if clicked {
                        match action {
                            PanelHeaderAction::Close => {
                                self.actions.push(DockAction::ClosePanel(id))
                            }
                            PanelHeaderAction::PopOut => self.actions.push(DockAction::PopOut(id)),
                            PanelHeaderAction::Float => {
                                let _ = self.transition_panel(id, PanelPlacement::Floating);
                            }
                            PanelHeaderAction::Dock => {
                                let _ = self.restore_panel(id);
                            }
                        }
                        popped_out = true;
                    }
                }
            },
        );
        if !popped_out {
            self.update_drag(id, response, pop_out_rect, ui.ctx());
        }

        let body_rect = ui.available_rect_before_wrap();
        let body_rect = if skinned_frame {
            body_rect.shrink(FRAME_PADDING)
        } else {
            body_rect
        };
        ui.scope_builder(UiBuilder::new().max_rect(body_rect), |body| {
            // The scroll fade multiplies content colors near the edges, which
            // would dim nest content; the static nest draws its own bars.
            body.spacing_mut().scroll.fade.strength = 0.0;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(body, |body| {
                    if let Some((_, content)) = self.panel_content_mut(id) {
                        content.ui(body, &chrome.for_surface(SurfaceId::Panel(id)));
                    }
                });
        });
    }

    fn show_floating(&mut self, ctx: &egui::Context, viewport: Rect, chrome: &PanelChrome) {
        let floating_ids: Vec<PanelId> = self
            .panels
            .iter()
            .filter_map(|(id, entry)| (entry.placement == PanelPlacement::Floating).then_some(*id))
            .collect();
        for id in floating_ids {
            let Some(rect) = self.floating_rect(id) else {
                continue;
            };
            let Some(metadata) = self.metadata(id) else {
                continue;
            };
            let min_size = metadata.min_size;
            let resizable = metadata.resizable;
            let drag_before = self
                .drag
                .as_ref()
                .is_some_and(|session| session.panel == id);
            let result = Window::new("")
                .id(floating_window_id(id))
                .title_bar(false)
                .resizable([resizable, resizable])
                .collapsible(false)
                .movable(false)
                .frame(egui::Frame::NONE.inner_margin(FLOATING_WINDOW_MARGIN))
                .min_size(min_size)
                .current_pos(rect.min)
                .default_size(rect.size())
                .constrain_to(viewport)
                .show(ctx, |window_ui| {
                    // The frame rect covers the content plus the 4pt margin,
                    // matching the fill/stroke coverage of the old Frame.
                    let frame_rect = window_ui.max_rect().expand(4.0);
                    if !chrome.paint_slice(
                        window_ui.painter(),
                        "panel_bg",
                        SkinState::Normal,
                        frame_rect,
                    ) {
                        window_ui.painter().rect_filled(
                            frame_rect,
                            2.0,
                            chrome.colors.panel_bg32(),
                        );
                    }
                    window_ui.painter().rect_stroke(
                        frame_rect,
                        2.0,
                        Stroke::new(1.0, chrome.colors.panel_border32()),
                        egui::StrokeKind::Inside,
                    );
                    self.paint_panel(window_ui, id, chrome, PanelScope::Floating)
                });
            // Any press inside the window raises it to the top of the stack.
            let pressed_inside = result.as_ref().is_some_and(|inner| {
                ctx.input(|input| {
                    input.pointer.any_pressed()
                        && input
                            .pointer
                            .interact_pos()
                            .is_some_and(|point| inner.response.rect.contains(point))
                })
            });
            if pressed_inside {
                self.raise_floating(id);
            }
            let drag_after = self
                .drag
                .as_ref()
                .is_some_and(|session| session.panel == id);
            if !drag_before && !drag_after {
                if let Some(inner) = result {
                    self.set_floating_rect(id, inner.response.rect);
                }
            }
            self.clamp_floating(id, viewport);
        }
    }

    fn update_drag(
        &mut self,
        id: PanelId,
        response: egui::Response,
        pop_out_rect: Option<Rect>,
        ctx: &egui::Context,
    ) {
        let is_floating = self.placement(id) == Some(PanelPlacement::Floating);
        let pointer =
            ctx.input(|input| input.pointer.latest_pos().or(input.pointer.press_origin()));
        let primary_pressed =
            ctx.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
        let primary_released =
            ctx.input(|input| input.pointer.button_released(egui::PointerButton::Primary));
        let pop_out_pressed = ctx.input(|input| {
            input
                .pointer
                .press_origin()
                .is_some_and(|origin| pop_out_rect.is_some_and(|rect| rect.contains(origin)))
        });
        let drag_started = !pop_out_pressed
            && (response.drag_started()
                || (primary_pressed
                    && pointer.is_some_and(|position| response.rect.contains(position))));
        if drag_started {
            let start = pointer
                .or_else(|| response.interact_pointer_pos())
                .unwrap_or(response.rect.center());
            let offset = if is_floating {
                self.floating_rect(id)
                    .map(|rect| start - rect.min)
                    .unwrap_or(Vec2::ZERO)
            } else {
                start - response.rect.min
            };
            self.drag = Some(super::state::DragSession {
                panel: id,
                start,
                current: start,
                offset,
            });
            response.dnd_set_drag_payload(id);
        }
        let mut floating_move = None;
        {
            let Some(session) = &mut self.drag else {
                return;
            };
            if session.panel != id {
                return;
            }
            if let Some(pointer) = pointer {
                let pointer_moved = pointer != session.current;
                session.current = pointer;
                if is_floating && pointer_moved {
                    floating_move = Some((pointer, session.offset));
                }
            }
        }
        if let Some((pointer, offset)) = floating_move {
            if let Some(rect) = self.floating_rect(id) {
                self.set_floating_rect(id, Rect::from_min_size(pointer - offset, rect.size()));
            }
        }
        if primary_pressed || (!response.drag_stopped() && !primary_released) {
            return;
        }
        let Some(session) = self.drag.take() else {
            return;
        };
        if let Some(target) = self.drop_target(session.current, ctx.content_rect()) {
            let (side, index) = target;
            let placement = match side {
                DockSide::Left => PanelPlacement::DockedLeft,
                DockSide::Right => PanelPlacement::DockedRight,
                DockSide::Bottom => PanelPlacement::DockedBottom,
            };
            if self.placement(id) != Some(placement) {
                let _ = self.transition_panel(id, placement);
            }
            let _ = self.reorder(side, id, index);
        } else if drag_outside_threshold(session.start, session.current) {
            if self.transition_panel(id, PanelPlacement::Floating).is_ok() {
                if let Some(rect) = self.floating_rect(id) {
                    self.set_floating_rect(
                        id,
                        Rect::from_min_size(session.current - session.offset, rect.size()),
                    );
                }
            }
        }
    }

    fn drop_target(&self, pointer: egui::Pos2, viewport: Rect) -> Option<(DockSide, usize)> {
        // A float-only panel is never dockable: offering no drop target means
        // no drop preview and no dock transition while it is dragged.
        if self
            .drag
            .as_ref()
            .is_some_and(|session| self.is_float_only(session.panel))
        {
            return None;
        }
        let left_rect = Rect::from_min_max(
            viewport.min,
            pos2(
                viewport.left() + self.dock_extent(DockSide::Left),
                viewport.bottom(),
            ),
        );
        if left_rect.contains(pointer) {
            let index = self
                .panel_rects(DockSide::Left, left_rect)
                .iter()
                .position(|(_, rect)| pointer.y < rect.center().y)
                .unwrap_or_else(|| self.dock_order(DockSide::Left).len());
            return Some((DockSide::Left, index));
        }
        let right_rect = Rect::from_min_max(
            pos2(
                viewport.right() - self.dock_extent(DockSide::Right),
                viewport.top(),
            ),
            viewport.max,
        );
        if right_rect.contains(pointer) {
            let index = self
                .panel_rects(DockSide::Right, right_rect)
                .iter()
                .position(|(_, rect)| pointer.y < rect.center().y)
                .unwrap_or_else(|| self.dock_order(DockSide::Right).len());
            return Some((DockSide::Right, index));
        }
        let bottom_rect = Rect::from_min_max(
            pos2(
                left_rect.right(),
                viewport.bottom() - self.dock_extent(DockSide::Bottom),
            ),
            pos2(right_rect.left(), viewport.bottom()),
        );
        if bottom_rect.contains(pointer) {
            let index = self
                .panel_rects(DockSide::Bottom, bottom_rect)
                .iter()
                .position(|(_, rect)| pointer.x < rect.center().x)
                .unwrap_or_else(|| self.dock_order(DockSide::Bottom).len());
            return Some((DockSide::Bottom, index));
        }
        None
    }

    fn paint_drop_preview(
        &self,
        ui: &mut egui::Ui,
        side: DockSide,
        rect: Rect,
        chrome: &PanelChrome,
    ) {
        let Some(_session) = &self.drag else { return };
        let Some(pointer) = ui.ctx().input(|input| input.pointer.latest_pos()) else {
            return;
        };
        let Some((target_side, index)) = self.drop_target(pointer, ui.ctx().content_rect()) else {
            return;
        };
        if target_side != side {
            return;
        }
        let panel_rects = self.panel_rects(side, rect);
        let preview = panel_rects
            .get(index.min(panel_rects.len().saturating_sub(1)))
            .map(|(_, panel_rect)| *panel_rect)
            .unwrap_or_else(|| match side {
                DockSide::Left => {
                    Rect::from_min_size(pos2(rect.left(), rect.top()), vec2(rect.width(), 8.0))
                }
                DockSide::Right => {
                    Rect::from_min_size(pos2(rect.left(), rect.top()), vec2(rect.width(), 8.0))
                }
                DockSide::Bottom => Rect::from_min_size(
                    pos2(rect.left(), rect.bottom() - 8.0),
                    vec2(8.0, rect.height()),
                ),
            });
        ui.painter()
            .rect_filled(preview, 2.0, chrome.colors.selection_fill32());
        ui.painter().rect_stroke(
            preview,
            2.0,
            Stroke::new(2.0, chrome.colors.selection_stroke_color32()),
            egui::StrokeKind::Inside,
        );
    }
}

/// Paints a resize handle: the `scrollbar_handle` skin center-stretched
/// (corner 0) when available, else the flat panel-border fill.
fn paint_resize_handle(ui: &mut egui::Ui, handle_rect: Rect, chrome: &PanelChrome) {
    if let Some((skin, atlas)) = chrome.skin("scrollbar_handle") {
        chrome.paint_nine_slice(
            ui.painter(),
            skin,
            atlas,
            SkinState::Normal,
            handle_rect,
            0.0,
        );
    } else {
        ui.painter()
            .rect_filled(handle_rect, 0.0, chrome.colors.panel_border32());
    }
}

/// Size of the skinned header action button: label width plus the standard
/// button padding, filling the shrunk header band.
fn skinned_button_size(label: &str, ui: &egui::Ui) -> Vec2 {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    vec2(galley.size().x + 8.0, HEADER_HEIGHT - 8.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_target_only_treats_the_actual_bottom_band_as_bottom_dock() {
        let manager = DockManager::demo();
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        assert_eq!(manager.drop_target(pos2(100.0, 100.0), viewport), None);
        assert_eq!(
            manager.drop_target(
                pos2(
                    100.0,
                    viewport.bottom() - manager.dock_extent(DockSide::Bottom) + 10.0
                ),
                viewport
            ),
            Some((DockSide::Bottom, 0)),
        );
    }

    #[test]
    fn drop_target_resolves_left_band() {
        let mut manager = DockManager::demo();
        manager
            .transition_panel(PanelId::new(3), PanelPlacement::DockedLeft)
            .unwrap();
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        assert_eq!(
            manager.drop_target(pos2(10.0, 100.0), viewport),
            Some((DockSide::Left, 0))
        );
    }

    #[test]
    fn surface_at_resolves_left_dock_panel() {
        let mut manager = DockManager::demo();
        manager
            .transition_panel(PanelId::new(3), PanelPlacement::DockedLeft)
            .unwrap();
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        assert_eq!(
            manager.surface_at(pos2(10.0, 100.0), viewport),
            Some(PanelId::new(3))
        );
    }

    #[test]
    fn bottom_dock_spans_between_left_and_right_docks() {
        let mut manager = DockManager::demo();
        manager
            .transition_panel(PanelId::new(3), PanelPlacement::DockedLeft)
            .unwrap();
        let viewport = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));

        let left = manager.dock_rect(DockSide::Left, viewport);
        let right = manager.dock_rect(DockSide::Right, viewport);
        let bottom = manager.dock_rect(DockSide::Bottom, viewport);
        assert_eq!(bottom.left(), left.right());
        assert_eq!(bottom.right(), right.left());
        assert_eq!(bottom.bottom(), viewport.bottom());
    }
}
