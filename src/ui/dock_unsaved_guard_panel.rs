//! Floating "Unsaved Changes" dock panel (id 211).
//!
//! Registered as a `PanelPlacement::Floating`, `can_pop_out == false`,
//! `resizable == false` dock panel holding a [`NestMode::Static`] nest,
//! matching the app's modal panels (`dock_create_document_panel`,
//! `dock_save_as_panel`, `dock_import_image_panel`).
//!
//! Prompts the user to **Save**, **Discard**, or **Cancel** before a pending
//! Close / Open / New action discards unsaved edits. Emits
//! [`UnsavedGuardEvent`]s for the App shell to execute.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
    FLOATING_PANEL_CHROME_HEIGHT,
};

/// Dock id for the Unsaved Changes floating panel.
pub const UNSAVED_GUARD_PANEL_ID: u64 = 211;

/// Panel/window title.
const UNSAVED_GUARD_TITLE: &str = "Unsaved Changes";

/// Fixed minimum size of the (non-resizable) floating window. Sized so the
/// two-line prompt, separator, and up to 5 action buttons (Save, Save All,
/// Cancel, Discard, Discard All) fit cleanly with zero scrollbars.
pub const UNSAVED_GUARD_MIN_SIZE: egui::Vec2 = vec2(476.0, 108.0 + FLOATING_PANEL_CHROME_HEIGHT);

/// Draft view state for the Unsaved Changes panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnsavedGuardView {
    pub document_name: String,
    /// Number of dirty documents remaining in an app-exit queue (>1 enables
    /// "Save All" and "Discard All" buttons and shows progress).
    pub remaining_count: usize,
    /// Total dirty documents when the exit guard started.
    pub total_count: usize,
}

/// Actions emitted by the Unsaved Changes panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsavedGuardEvent {
    Save,
    SaveAll,
    Discard,
    DiscardAll,
    Cancel,
}

/// Shared cells for the Unsaved Changes panel.
#[derive(Clone, Default)]
pub struct UnsavedGuardHost {
    pub view: Rc<RefCell<UnsavedGuardView>>,
    pub events: Rc<RefCell<Vec<UnsavedGuardEvent>>>,
}

impl UnsavedGuardHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_document_name(&self, name: impl Into<String>) {
        self.view.borrow_mut().document_name = name.into();
    }

    pub fn set_queue_progress(&self, remaining_count: usize, total_count: usize) {
        let mut view = self.view.borrow_mut();
        view.remaining_count = remaining_count;
        view.total_count = total_count;
    }

    pub fn take_events(&self) -> Vec<UnsavedGuardEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

/// Static form component inside the Unsaved Changes panel.
struct UnsavedGuardComponent {
    host: UnsavedGuardHost,
}

impl Component for UnsavedGuardComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let (doc_name, remaining_count, total_count) = {
            let v = self.host.view.borrow();
            (v.document_name.clone(), v.remaining_count, v.total_count)
        };
        let display_name = if doc_name.is_empty() {
            "Untitled"
        } else {
            doc_name.as_str()
        };
        let multi = remaining_count > 1;
        if total_count > 1 {
            let current = total_count.saturating_sub(remaining_count) + 1;
            ui.horizontal(|ui| {
                ui.strong(format!("Save changes to \"{display_name}\"?"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak(format!("{current} of {total_count}"));
                });
            });
        } else {
            ui.strong(format!("Save changes to \"{display_name}\"?"));
        }
        ui.add_space(4.0);
        ui.weak("Your unsaved changes will be lost if you discard them.");

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        let mut emitted = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if chrome.button(ui, "Discard", ButtonStyle::plain()).clicked() {
                emitted = Some(UnsavedGuardEvent::Discard);
            }
            if multi && chrome.button(ui, "Discard All", ButtonStyle::plain()).clicked() {
                emitted = Some(UnsavedGuardEvent::DiscardAll);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome.button(ui, "Save", ButtonStyle::toggled(true)).clicked() {
                    emitted = Some(UnsavedGuardEvent::Save);
                }
                if multi && chrome.button(ui, "Save All", ButtonStyle::plain()).clicked() {
                    emitted = Some(UnsavedGuardEvent::SaveAll);
                }
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    emitted = Some(UnsavedGuardEvent::Cancel);
                }
            });
        });

        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            emitted = Some(UnsavedGuardEvent::Cancel);
        } else if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            emitted = Some(UnsavedGuardEvent::Save);
        }

        if let Some(ev) = emitted {
            self.host.events.borrow_mut().push(ev);
        }
    }
}

struct UnsavedGuardPanelContent {
    nest: NestContent,
}

impl PanelContent for UnsavedGuardPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

/// Static nest holding the Unsaved Changes prompt.
pub(crate) fn build_unsaved_guard_nest(host: &UnsavedGuardHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(UnsavedGuardComponent { host: host.clone() });
    nest
}

/// Dock spec for the Unsaved Changes floating panel (dock id 211).
pub fn unsaved_guard_panel_spec(host: &UnsavedGuardHost, floating_rect: egui::Rect) -> PanelSpec {
    let floating_rect = egui::Rect::from_min_size(
        floating_rect.min,
        vec2(
            floating_rect.width().max(UNSAVED_GUARD_MIN_SIZE.x),
            floating_rect.height().max(UNSAVED_GUARD_MIN_SIZE.y),
        ),
    );
    PanelSpec {
        id: PanelId::new(UNSAVED_GUARD_PANEL_ID),
        metadata: PanelMetadata {
            resizable: false,
            ..PanelMetadata::new(UNSAVED_GUARD_TITLE, false, UNSAVED_GUARD_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(UnsavedGuardPanelContent {
            nest: build_unsaved_guard_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_float_only_not_resizable_and_uses_a_static_nest() {
        let host = UnsavedGuardHost::new();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), UNSAVED_GUARD_MIN_SIZE);
        let spec = unsaved_guard_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), UNSAVED_GUARD_PANEL_ID);
        assert_eq!(spec.metadata.title, "Unsaved Changes");
        assert!(!spec.metadata.can_pop_out);
        assert!(!spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        assert_eq!(build_unsaved_guard_nest(&host).mode(), NestMode::Static);
    }

    #[test]
    fn enter_emits_save_and_escape_emits_cancel() {
        let host = UnsavedGuardHost::new();
        host.set_document_name("Sprite");
        let mut nest = build_unsaved_guard_nest(&host);
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(UNSAVED_GUARD_PANEL_ID)),
        );

        let raw_enter = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                UNSAVED_GUARD_MIN_SIZE,
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        ctx.run_ui(raw_enter, |ui| nest.ui(ui, &chrome))
            .textures_delta
            .clear();
        assert_eq!(host.take_events(), vec![UnsavedGuardEvent::Save]);

        let raw_esc = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                UNSAVED_GUARD_MIN_SIZE,
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        ctx.run_ui(raw_esc, |ui| nest.ui(ui, &chrome))
            .textures_delta
            .clear();
        assert_eq!(host.take_events(), vec![UnsavedGuardEvent::Cancel]);
    }

    #[test]
    fn unsaved_guard_panel_fits_all_multi_buttons_without_overflow() {
        let host = UnsavedGuardHost::new();
        host.set_document_name("LongProjectName");
        host.set_queue_progress(2, 3);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, UNSAVED_GUARD_MIN_SIZE);
        let ctx = egui::Context::default();
        let theme = crate::ui::theme::Theme::default_dark();
        let capture = crate::ui::input_capture::InputCapture::new();
        let chrome = PanelChrome::flat(
            &theme,
            &capture,
            crate::ui::input_capture::SurfaceId::Panel(PanelId::new(UNSAVED_GUARD_PANEL_ID)),
        );
        let chrome_h = FLOATING_PANEL_CHROME_HEIGHT;
        let viewport_size = vec2(rect.width() - 22.0, rect.height() - chrome_h);
        let body_rect = egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            vec2(
                viewport_size.x + 2.0 * crate::ui::panel_dock::nest::NEST_PADDING,
                viewport_size.y + 2.0 * crate::ui::panel_dock::nest::NEST_PADDING,
            ),
        );
        let mut measured_content_size = egui::Vec2::ZERO;
        let raw = egui::RawInput {
            screen_rect: Some(rect),
            ..Default::default()
        };
        ctx.run_ui(raw, |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(body_rect), |body| {
                let nest_vp =
                    crate::ui::panel_dock::nest::nest_viewport(body.available_rect_before_wrap());
                body.scope_builder(egui::UiBuilder::new().max_rect(nest_vp), |inner| {
                    let out = egui::ScrollArea::both()
                        .auto_shrink([false, false])
                        .min_scrolled_width(0.0)
                        .min_scrolled_height(0.0)
                        .show(inner, |scroll| {
                            let view = NestView::screen(nest_vp);
                            let mut comp = UnsavedGuardComponent { host: host.clone() };
                            comp.ui(scroll, &view, &chrome);
                        });
                    measured_content_size = out.content_size;
                });
            });
        })
        .textures_delta
        .clear();
        assert!(
            measured_content_size.y <= viewport_size.y,
            "Unsaved Guard content height ({}) must fit inside viewport ({}) without vertical scrollbar",
            measured_content_size.y,
            viewport_size.y
        );
        assert!(
            measured_content_size.x <= viewport_size.x,
            "Unsaved Guard content width ({}) must fit inside viewport ({}) without horizontal scrollbar",
            measured_content_size.x,
            viewport_size.x
        );
    }
}
