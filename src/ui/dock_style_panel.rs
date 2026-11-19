//! Floating "Style" dock panel (id 213).
//!
//! Registered as a `PanelPlacement::Floating`, `can_pop_out == false`,
//! `resizable == false` dock panel holding a [`NestMode::Static`] nest,
//! opened from **Options -> style**. Lets the user switch built-in low-contrast
//! dark presets (`Ash`, `Amber`, `Emerald`, `Slate`), adjust corner rounding
//! (default `0`), customize UI colors, and save/load style JSON files.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};
use crate::ui::style::{user_style_path, StylePreset, UiStyle, MAX_ROUNDING};

/// Dock id for the Style floating panel.
pub const STYLE_PANEL_ID: u64 = 213;

/// Panel/window title.
const STYLE_TITLE: &str = "Style";

/// Minimum size of the resizable floating Style window.
const STYLE_MIN_SIZE: egui::Vec2 = vec2(300.0, 280.0);

/// Actions emitted by the Style panel.
#[derive(Clone, Debug, PartialEq)]
pub enum StyleEvent {
    SelectPreset(StylePreset),
    UpdateStyle(UiStyle),
    OpenBannerPicker,
    SaveFile(PathBuf),
    LoadFile(PathBuf),
}

/// Shared view & event state for the Style panel.
#[derive(Clone)]
pub struct StyleHost {
    pub style: Rc<RefCell<UiStyle>>,
    pub file_path: Rc<RefCell<String>>,
    pub status: Rc<RefCell<Option<String>>>,
    pub banner_preview: Rc<RefCell<Option<(egui::TextureId, [u32; 2])>>>,
    pub events: Rc<RefCell<Vec<StyleEvent>>>,
}

impl Default for StyleHost {
    fn default() -> Self {
        Self::new(UiStyle::default())
    }
}

impl StyleHost {
    pub fn new(initial: UiStyle) -> Self {
        let default_path = user_style_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "style.json".to_string());
        Self {
            style: Rc::new(RefCell::new(initial)),
            file_path: Rc::new(RefCell::new(default_path)),
            status: Rc::new(RefCell::new(None)),
            banner_preview: Rc::new(RefCell::new(None)),
            events: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn set_style(&self, style: UiStyle) {
        *self.style.borrow_mut() = style;
    }

    pub fn set_status(&self, msg: impl Into<String>) {
        *self.status.borrow_mut() = Some(msg.into());
    }

    pub fn set_banner_preview(&self, preview: Option<(egui::TextureId, [u32; 2])>) {
        *self.banner_preview.borrow_mut() = preview;
    }

    pub fn take_events(&self) -> Vec<StyleEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

fn color_edit_row(ui: &mut egui::Ui, label: &str, rgba: &mut [u8; 4]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut color = crate::ui::theme::rgba32(*rgba);
        if ui.color_edit_button_srgba(&mut color).changed() {
            *rgba = [color.r(), color.g(), color.b(), color.a()];
            changed = true;
        }
        ui.label(label);
    });
    changed
}

struct StyleComponent {
    host: StyleHost,
}

impl Component for StyleComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let mut style = self.host.style.borrow().clone();
        let mut modified = false;

        ui.label("Preset");
        ui.add_space(4.0);
        let current_preset = style.preset.clone();
        egui::ComboBox::from_id_salt("style_preset_combo")
            .width(ui.available_width().clamp(140.0, 240.0))
            .selected_text(&current_preset)
            .show_ui(ui, |ui| {
                for preset in StylePreset::ALL {
                    let selected = current_preset == preset.name();
                    if chrome.combo_item(ui, selected, preset.name()).clicked() && !selected {
                        self.host
                            .events
                            .borrow_mut()
                            .push(StyleEvent::SelectPreset(preset));
                    }
                }
            });

        ui.add_space(8.0);
        ui.label("Rounding");
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let mut r = style.clamped_rounding();
            let drag = ui.add(egui::DragValue::new(&mut r).range(0.0..=MAX_ROUNDING).speed(0.1));
            let slider = ui.add(egui::Slider::new(&mut r, 0.0..=MAX_ROUNDING).show_value(false));
            if drag.changed() || slider.changed() {
                style.rounding = r.clamp(0.0, MAX_ROUNDING);
                modified = true;
            }
            if chrome.button(ui, "Reset", ButtonStyle::plain()).clicked() && style.rounding != 0.0 {
                style.rounding = 0.0;
                modified = true;
            }
        });

        ui.add_space(8.0);
        ui.label("Colors");
        ui.add_space(2.0);
        let mut color_changed = false;
        color_changed |= color_edit_row(ui, "Workspace", &mut style.colors.clear_color);
        color_changed |= color_edit_row(ui, "Panel", &mut style.colors.panel_bg);
        color_changed |= color_edit_row(ui, "Header", &mut style.colors.panel_header_bg);
        color_changed |= color_edit_row(ui, "Border", &mut style.colors.panel_border);
        color_changed |= color_edit_row(ui, "Accent Fill", &mut style.colors.accent_fill);
        color_changed |= color_edit_row(ui, "Accent Stroke", &mut style.colors.accent_stroke);
        color_changed |= color_edit_row(ui, "Text", &mut style.colors.text_primary);
        if color_changed {
            style.preset = "Custom".to_string();
            modified = true;
        }

        if modified {
            *self.host.style.borrow_mut() = style.clone();
            self.host
                .events
                .borrow_mut()
                .push(StyleEvent::UpdateStyle(style.clone()));
        }

        ui.add_space(8.0);
        ui.label("File");
        ui.add_space(2.0);
        let mut path_str = self.host.file_path.borrow().clone();
        if ui
            .add(egui::TextEdit::singleline(&mut path_str).desired_width(ui.available_width()))
            .changed()
        {
            *self.host.file_path.borrow_mut() = path_str.clone();
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if chrome.button(ui, "Save File", ButtonStyle::plain()).clicked() {
                self.host
                    .events
                    .borrow_mut()
                    .push(StyleEvent::SaveFile(PathBuf::from(path_str.trim())));
            }
            if chrome.button(ui, "Load File", ButtonStyle::plain()).clicked() {
                self.host
                    .events
                    .borrow_mut()
                    .push(StyleEvent::LoadFile(PathBuf::from(path_str.trim())));
            }
        });

        if let Some(status) = self.host.status.borrow().as_deref() {
            ui.add_space(4.0);
            ui.weak(status);
        }

        ui.add_space(8.0);
        ui.label("Banner");
        ui.add_space(2.0);
        let preview = *self.host.banner_preview.borrow();
        let current_name = style.banner.clone();
        if let Some((tex_id, [bw, bh])) = preview {
            if bw > 0 && bh > 0 {
                let target_h = 44.0_f32;
                let max_w = ui.available_width().max(32.0);
                let aspect_w = target_h * (bw as f32 / bh as f32);
                let draw_size = if aspect_w <= max_w {
                    vec2(aspect_w, target_h)
                } else {
                    vec2(max_w, max_w * (bh as f32 / bw as f32))
                };
                let (rect, _) = ui.allocate_exact_size(draw_size, egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 0.0, chrome.colors.list_bg32());
                ui.painter().image(
                    tex_id,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    crate::ui::theme::SKIN_TINT,
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(1.0, chrome.colors.panel_border32()),
                    egui::StrokeKind::Inside,
                );
            }
        } else {
            ui.weak("No banner selected");
        }
        if let Some(name) = current_name.as_deref() {
            ui.weak(name);
        }
        ui.add_space(4.0);
        if chrome
            .button(ui, "change banner", ButtonStyle::plain())
            .clicked()
        {
            self.host
                .events
                .borrow_mut()
                .push(StyleEvent::OpenBannerPicker);
        }
    }
}

struct StylePanelContent {
    nest: NestContent,
}

impl PanelContent for StylePanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

pub(crate) fn build_style_nest(host: &StyleHost) -> NestContent {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(StyleComponent { host: host.clone() });
    nest
}

/// Dock spec for the Style floating panel (dock id 213).
pub fn style_panel_spec(host: &StyleHost, floating_rect: egui::Rect) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(STYLE_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(STYLE_TITLE, false, STYLE_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(StylePanelContent {
            nest: build_style_nest(host),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_float_only_resizable_and_uses_a_static_nest() {
        let host = StyleHost::default();
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), STYLE_MIN_SIZE);
        let spec = style_panel_spec(&host, rect);
        assert_eq!(spec.id.raw(), STYLE_PANEL_ID);
        assert_eq!(spec.metadata.title, "Style");
        assert!(!spec.metadata.can_pop_out);
        assert!(spec.metadata.resizable);
        assert_eq!(spec.placement, PanelPlacement::Floating);
        assert_eq!(build_style_nest(&host).mode(), NestMode::Static);
    }
}
