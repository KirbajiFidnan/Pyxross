//! Floating "Select Banner" dock panel (id 215).
//!
//! Opened when clicking "change banner" in the Style panel (id 213).
//! Displays image thumbnails of all banners found in `banners/` directories
//! inside a `NestMode::Static` floating panel.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{nest_is_usable, nest_viewport};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Select Banner floating panel.
pub const BANNER_PICKER_PANEL_ID: u64 = 215;

/// Panel/window title.
const BANNER_PICKER_TITLE: &str = "Select Banner";

/// Fixed minimum size of the floating Select Banner window.
const BANNER_PICKER_MIN_SIZE: egui::Vec2 = vec2(380.0, 340.0);

/// Loaded thumbnail entry displayed inside the Select Banner panel.
#[derive(Clone, Debug, PartialEq)]
pub struct BannerThumbItem {
    pub name: String,
    pub texture: egui::TextureId,
    pub size: [u32; 2],
}

/// Actions emitted by the Select Banner panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BannerPickerEvent {
    Selected(Option<String>),
    Refresh,
    ImportBanner,
}

/// Shared view & event state for the Select Banner panel.
#[derive(Clone, Default)]
pub struct BannerPickerHost {
    pub items: Rc<RefCell<Vec<BannerThumbItem>>>,
    pub active_banner: Rc<RefCell<Option<String>>>,
    pub events: Rc<RefCell<Vec<BannerPickerEvent>>>,
}

impl BannerPickerHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_items(&self, items: Vec<BannerThumbItem>, active: Option<String>) {
        *self.items.borrow_mut() = items;
        *self.active_banner.borrow_mut() = active;
    }

    pub fn take_events(&self) -> Vec<BannerPickerEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

struct BannerPickerPanelContent {
    host: BannerPickerHost,
}

impl BannerPickerPanelContent {
    fn render_pinned_actions(&self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let active = self.host.active_banner.borrow().clone();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if chrome
                .button(ui, "None (clear)", ButtonStyle::toggled(active.is_none()))
                .clicked()
            {
                self.host
                    .events
                    .borrow_mut()
                    .push(BannerPickerEvent::Selected(None));
            }
            if chrome.button(ui, "Refresh", ButtonStyle::plain()).clicked() {
                self.host
                    .events
                    .borrow_mut()
                    .push(BannerPickerEvent::Refresh);
            }
            if chrome
                .button(ui, "Import Banner", ButtonStyle::plain())
                .on_hover_text("Import an image and crop it as a new top-bar banner")
                .clicked()
            {
                self.host
                    .events
                    .borrow_mut()
                    .push(BannerPickerEvent::ImportBanner);
            }
        });
    }

    fn render_banner_list(&self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let active = self.host.active_banner.borrow().clone();
        let items = self.host.items.borrow().clone();

        if items.is_empty() {
            ui.weak("No banner images found.");
            ui.add_space(4.0);
            ui.weak("Use Import Banner or place PNG/JPG/WEBP files into banners/.");
            return;
        }

        let avail_w = ui.available_width().max(64.0);
        for item in items {
            let selected = active.as_deref() == Some(item.name.as_str());
            let [bw, bh] = item.size;
            let thumb_h = 48.0_f32;
            let thumb_w = if bw > 0 && bh > 0 {
                (thumb_h * (bw as f32 / bh as f32)).min((avail_w - 16.0).max(16.0))
            } else {
                120.0_f32.min((avail_w - 16.0).max(16.0))
            };
            let row_h = thumb_h + 24.0;
            let (row_rect, response) =
                ui.allocate_exact_size(vec2(avail_w, row_h), egui::Sense::click());
            let fill = if selected {
                chrome.colors.selection_bg_fill32()
            } else if response.hovered() {
                chrome.colors.selection_bg_fill32().gamma_multiply(0.45)
            } else {
                chrome.colors.list_bg32()
            };
            let stroke = if selected || response.hovered() {
                chrome.colors.selection_stroke_color32()
            } else {
                chrome.colors.panel_border32()
            };
            ui.painter().rect_filled(row_rect, chrome.rounding, fill);
            ui.painter().rect_stroke(
                row_rect,
                chrome.rounding,
                egui::Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );

            let thumb_rect = egui::Rect::from_min_size(
                egui::pos2(row_rect.left() + 6.0, row_rect.top() + 4.0),
                vec2(thumb_w.max(16.0), thumb_h),
            );
            ui.painter_at(row_rect).image(
                item.texture,
                thumb_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                crate::ui::theme::SKIN_TINT,
            );
            ui.painter_at(row_rect).text(
                egui::pos2(row_rect.left() + 6.0, thumb_rect.bottom() + 4.0),
                egui::Align2::LEFT_TOP,
                &item.name,
                egui::FontId::proportional(12.0),
                ui.visuals().text_color(),
            );

            if response.clicked() {
                self.host
                    .events
                    .borrow_mut()
                    .push(BannerPickerEvent::Selected(Some(item.name)));
            }
            ui.add_space(4.0);
        }
    }
}

impl PanelContent for BannerPickerPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let full = ui.available_rect_before_wrap();
        ui.set_min_size(full.size());
        let viewport = nest_viewport(full);
        if !nest_is_usable(viewport) {
            return;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(viewport), |inner| {
            inner.set_clip_rect(viewport.intersect(inner.clip_rect()));
            // Pinned top button bar: stays static and visible regardless of scroll.
            self.render_pinned_actions(inner, chrome);
            inner.add_space(6.0);
            inner.separator();
            inner.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt(inner.id().with("banner-picker-scroll"))
                .auto_shrink([false, false])
                .show(inner, |scroll_ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 0,
                            right: 16,
                            top: 0,
                            bottom: 4,
                        })
                        .show(scroll_ui, |list_ui| {
                            self.render_banner_list(list_ui, chrome);
                        });
                });
        });
    }
}

/// Dock spec for the Select Banner floating panel (dock id 215).
pub fn banner_picker_panel_spec(host: &BannerPickerHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(BANNER_PICKER_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(BANNER_PICKER_TITLE, false, BANNER_PICKER_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(BannerPickerPanelContent { host: host.clone() }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_float_only_and_resizable() {
        let host = BannerPickerHost::new();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), BANNER_PICKER_MIN_SIZE);
        let spec = banner_picker_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), BANNER_PICKER_PANEL_ID);
        assert_eq!(spec.metadata.title, "Select Banner");
        assert!(!spec.metadata.can_pop_out);
        assert!(spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
    }
}
