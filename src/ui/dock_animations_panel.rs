//! Animations dock panel: the project's animation list with pinned add/remove
//! actions and a per-row settings gear.
//!
//! **DOCK-ONLY.** `PanelMetadata::new("Animations", false, ...)` gives it
//! `can_pop_out == false` (and `can_native_pop_out == false`), so its header
//! exposes neither Float nor Pop-out.
//!
//! A static nest holds two components: the scrollable row list (content) and
//! the pinned action header (overlay). The header never scrolls; the rows
//! scroll under it. Every gesture is emitted as an [`AnimationPanelEvent`] for
//! the App shell to apply — the panel is a pure view over the host cells and
//! never mutates the model.

use egui::{vec2, Sense};

use crate::ui::animation_hosts::{AnimationPanelEvent, AnimationPanelHost, AnimationRow};
use crate::ui::panel_dock::nest::{Component, ComponentPlacement, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};
use crate::ui::theme::SkinState;

/// The dock id for the Animations panel.
pub const ANIMATIONS_PANEL_ID: u64 = 201;

/// Height of one animation row.
const ROW_HEIGHT: f32 = 28.0;

/// Height of the pinned header band (matches the dock header height).
const HEADER_HEIGHT: f32 = 28.0;

/// Pinned action header: `+` / `-` over the scrolling row list.
///
/// An overlay component, so it stays fixed while the rows scroll.
struct AnimationsHeaderComponent {
    host: AnimationPanelHost,
}

impl Component for AnimationsHeaderComponent {
    fn placement(&self) -> ComponentPlacement {
        ComponentPlacement::Overlay
    }

    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        // Snapshot the active index and the remove gate before pushing events,
        // so the list borrow ends before the events borrow begins.
        let active = self
            .host
            .list
            .borrow()
            .animations
            .iter()
            .find(|row| row.active)
            .map(|row| row.index);
        let remove_enabled = self.host.list.borrow().remove_enabled;
        ui.horizontal(|ui| {
            // The add/remove actions are square glyph buttons on the left.
            if header_button(ui, chrome, "+", ButtonStyle::plain())
                .on_hover_text("Add a new animation")
                .clicked()
            {
                self.host.events.borrow_mut().push(AnimationPanelEvent::Add);
            }
            let remove_style = ButtonStyle {
                enabled: remove_enabled,
                ..ButtonStyle::plain()
            };
            let remove = header_button(ui, chrome, "-", remove_style)
                .on_hover_text("Remove the active animation");
            if remove.clicked() {
                if let Some(index) = active {
                    self.host
                        .events
                        .borrow_mut()
                        .push(AnimationPanelEvent::Remove(index));
                }
            }
            // The "Set Frames" action is pinned to the far right of the band
            // while add/remove stay on the left.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome
                    .button(ui, "Set Frames", ButtonStyle::plain())
                    .on_hover_text("Set the frame regions for this animation")
                    .clicked()
                {
                    push_set_frames(&self.host);
                }
            });
        });
        ui.add_space(2.0);
        ui.separator();
    }
}

/// The square side for the pinned `+`/`-` header buttons: the theme's standard
/// interaction height, so width equals height.
///
/// Pure and unit-testable: the returned vector is always square.
fn header_button_size(ui: &egui::Ui) -> egui::Vec2 {
    let side = ui.spacing().interact_size.y;
    vec2(side, side)
}

/// Draws a square, fixed-size header button while preserving the theme button
/// skin. Mirrors the frame-edit panel's fixed-size control button.
fn header_button(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    label: &str,
    style: ButtonStyle,
) -> egui::Response {
    let size = header_button_size(ui);
    if chrome.skin("button").is_none() {
        return ui
            .add_enabled_ui(style.enabled, |ui| {
                ui.add_sized(size, egui::Button::selectable(style.selected, label))
            })
            .inner;
    }

    let (rect, response) = ui
        .add_enabled_ui(style.enabled, |ui| ui.allocate_exact_size(size, Sense::click()))
        .inner;
    let state = if !response.enabled() {
        SkinState::Disabled
    } else if response.is_pointer_button_down_on() {
        SkinState::Pressed
    } else if response.hovered() {
        SkinState::Hover
    } else {
        SkinState::Normal
    };
    chrome.paint_slice(ui.painter(), "button", state, rect);
    if style.selected {
        let wash = chrome.colors.selection_bg_fill32().gamma_multiply(0.5);
        ui.painter().rect_filled(rect, 2.0, wash);
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.painter().with_clip_rect(rect).text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        ui.visuals().text_color(),
    );
    response
}

/// Pushes the "set frames" gesture for the active animation. Factored out of
/// the button handler so it is testable without a real click.
fn push_set_frames(host: &AnimationPanelHost) {
    host.events
        .borrow_mut()
        .push(AnimationPanelEvent::SetFramesTool);
}

/// Scrollable animation list: one row per animation, in playback order.
struct AnimationsListComponent {
    host: AnimationPanelHost,
}

impl Component for AnimationsListComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let view = self.host.list.borrow();
        let active = view
            .animations
            .iter()
            .find(|row| row.active)
            .map(|row| row.index);
        // Like the Layers list, a horizontal clip viewport keeps the rows'
        // fixed-size controls from widening the nest's scroll content when the
        // panel is narrow. It is wheel/drag-neutral and bar-free, so it never
        // steals input from the rows or the nest's vertical scroll.
        egui::ScrollArea::horizontal()
            .id_salt("animations-list")
            .auto_shrink([false, true])
            .min_scrolled_width(0.0)
            .scroll_source(egui::scroll_area::ScrollSource::NONE)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                // Tall enough to overflow a short dock viewport so the rows
                // scroll under the pinned header; the +40 reserves the header
                // band plus a little tail.
                ui.set_min_size(vec2(
                    ui.available_width(),
                    view.animations.len() as f32 * ROW_HEIGHT + HEADER_HEIGHT + 40.0,
                ));
                ui.add_space(HEADER_HEIGHT);
                for row in &view.animations {
                    row_ui(&self.host, ui, row, active, chrome);
                }
            });
    }
}

/// One animation row: name (click selects) + frame count, with a settings gear
/// pinned to the far right.
fn row_ui(
    host: &AnimationPanelHost,
    ui: &mut egui::Ui,
    row: &AnimationRow,
    active: Option<usize>,
    chrome: &PanelChrome,
) {
    let is_active = active == Some(row.index);
    let row_bg = if is_active {
        chrome.colors.selection_bg_fill32()
    } else {
        egui::Color32::TRANSPARENT
    };
    egui::Frame::NONE
        .fill(row_bg)
        .inner_margin(egui::Margin::symmetric(4, 2))
        .show(ui, |ui| {
            // The row background must cover the full nest width.
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                // The name senses a plain click; the active row is selected.
                let name = ui.add(egui::Button::new(&row.name).selected(is_active));
                if name.clicked() {
                    host.events
                        .borrow_mut()
                        .push(AnimationPanelEvent::Select(row.index));
                }
                ui.weak(format!("{} frames", row.frame_count));
                // The gear is pinned to the far right of the row while the
                // name/count stay on the left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if chrome
                        .button(ui, "⚙", ButtonStyle::plain())
                        .on_hover_text("Animation settings")
                        .clicked()
                    {
                        host.events
                            .borrow_mut()
                            .push(AnimationPanelEvent::OpenSettings(row.index));
                    }
                });
            });
        });
}

/// Panel content: the static nest holding the animation rows and pinned header.
struct AnimationsPanelContent {
    nest: NestContent,
}

impl PanelContent for AnimationsPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the animation rows (content) and the pinned header
/// (overlay).
pub(crate) fn build_animations_nest(host: &AnimationPanelHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(AnimationsListComponent { host: host.clone() })
        .push(AnimationsHeaderComponent { host: host.clone() });
    nest
}

/// Dock spec for the Animations panel (dock id `PanelId::new(201)`).
///
/// **Dock-only:** `can_pop_out == false`, so the header offers neither Float nor
/// Pop-out; the App docks it by default.
pub fn animations_panel_spec(host: &AnimationPanelHost, placement: PanelPlacement) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(ANIMATIONS_PANEL_ID),
        metadata: PanelMetadata::new("Animations", false, vec2(220.0, 120.0)),
        placement,
        floating_rect: egui::Rect::from_min_size(egui::pos2(120.0, 120.0), vec2(260.0, 320.0)),
        content: Box::new(AnimationsPanelContent {
            nest: build_animations_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::input_capture::{InputCapture, SurfaceId};
    use crate::ui::theme::Theme;

    /// Drive one headless frame of the pinned header component.
    fn run_header_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        host: &AnimationPanelHost,
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 200.0),
            )),
            predicted_dt: 1.0 / 60.0,
            events,
            ..Default::default()
        };
        let theme = Theme::default_dark();
        let capture = InputCapture::new();
        let chrome = PanelChrome::flat(&theme, &capture, SurfaceId::Canvas);
        let mut component = AnimationsHeaderComponent { host: host.clone() };
        let mut output = ctx.run_ui(raw, |ui| {
            let view = NestView::screen(ui.max_rect());
            component.ui(ui, &view, &chrome);
        });
        output.textures_delta.clear();
        output
    }

    /// Top-left positions of every rendered text equal to `text`.
    fn text_positions(output: &egui::FullOutput, text: &str) -> Vec<egui::Pos2> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
            match shape {
                egui::Shape::Text(t) => out.push((t.galley.text().to_string(), t.pos)),
                egui::Shape::Vec(shapes) => {
                    for s in shapes {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut texts);
        }
        texts
            .into_iter()
            .filter(|(t, _)| t == text)
            .map(|(_, pos)| pos)
            .collect()
    }

    #[test]
    fn header_button_size_is_square_and_positive() {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let size = header_button_size(ui);
            assert_eq!(size.x, size.y, "header buttons must be square");
            assert!(size.x > 0.0, "header button side must be positive");
            assert_eq!(
                size,
                vec2(ui.spacing().interact_size.y, ui.spacing().interact_size.y)
            );
        });
        output.textures_delta.clear();
    }

    #[test]
    fn push_set_frames_pushes_exactly_the_event() {
        let host = AnimationPanelHost::default();
        push_set_frames(&host);
        assert_eq!(
            host.events.borrow().as_slice(),
            &[AnimationPanelEvent::SetFramesTool]
        );
    }

    #[test]
    fn clicking_set_frames_emits_the_event() {
        let ctx = egui::Context::default();
        let host = AnimationPanelHost::default();
        // First frame lays the header out; no click yet, so no event.
        let output = run_header_frame(&ctx, Vec::new(), &host);
        assert!(host.events.borrow().is_empty());
        let pos = text_positions(&output, "Set Frames")
            .into_iter()
            .next()
            .expect("the Set Frames button should render its label");
        let at = pos + egui::vec2(4.0, 8.0);

        run_header_frame(&ctx, vec![egui::Event::PointerMoved(at)], &host);
        run_header_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            &host,
        );
        run_header_frame(
            &ctx,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            &host,
        );

        assert_eq!(
            host.events.borrow().as_slice(),
            &[AnimationPanelEvent::SetFramesTool]
        );
    }
}
