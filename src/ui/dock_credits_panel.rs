//! Floating "Credits" dock panel (id 218).
//!
//! Displays author attribution and acknowledgments for open-source projects
//! and libraries used or adapted in Pyxross Workshop.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::ui::panel_dock::nest::{Component, NestContent, NestMode, NestView};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Credits floating panel.
pub const CREDITS_PANEL_ID: u64 = 218;

const CREDITS_TITLE: &str = "Credits";
const CREDITS_MIN_SIZE: egui::Vec2 = vec2(400.0, 320.0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CreditsEvent {
    Close,
}

#[derive(Clone, Default)]
pub struct CreditsHost {
    pub events: Rc<RefCell<Vec<CreditsEvent>>>,
}

impl CreditsHost {
    pub fn take_events(&mut self) -> Vec<CreditsEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

struct CreditsComponent {
    host: CreditsHost,
}

impl Component for CreditsComponent {
    fn ui(&mut self, ui: &mut egui::Ui, _view: &NestView, chrome: &PanelChrome) {
        let footer_h = 36.0;
        let scroll_h = (ui.available_height() - footer_h).max(120.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(scroll_h)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.strong("Pyxross Workshop");
                ui.label("Creator & Developer: Kuzg-Pixel (Hacı Can Güngör)");
                ui.label("License: MIT");
                ui.add_space(10.0);

                ui.strong("Algorithms & Open-Source Acknowledgments");
                ui.add_space(4.0);
                credit_card(
                    ui,
                    chrome,
                    "SLK_img2pixel — Lukas Holzbeierlein (Captain4LK)",
                    "CC0-1.0 (Public Domain)",
                    "Image-to-Pixel pipeline inspiration (sampling, K-means palette & dithering).",
                );
                ui.add_space(6.0);
                credit_card(
                    ui,
                    chrome,
                    "egui & egui_dock — Emil Ernerfeldt & contributors",
                    "MIT / Apache-2.0",
                    "Immediate-mode GUI framework.",
                );
                ui.add_space(6.0);
                credit_card(
                    ui,
                    chrome,
                    "wgpu & winit — gfx-rs & rust-windowing",
                    "MIT / Apache-2.0",
                    "Cross-platform GPU rendering and windowing.",
                );
                ui.add_space(6.0);
                credit_card(
                    ui,
                    chrome,
                    "image / png / gif / arboard / serde / flate2",
                    "MIT / Apache-2.0",
                    "Image codecs, clipboard integration, serialization, and compression.",
                );
                ui.add_space(6.0);
                credit_card(
                    ui,
                    chrome,
                    "JetBrains Mono Nerd Font",
                    "SIL Open Font License 1.1 (OFL-1.1)",
                    "Embedded UI/icon font (JetBrains s.r.o. & Nerd Fonts contributors).",
                );
                ui.add_space(6.0);
            });

        ui.separator();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome.button(ui, "Close", ButtonStyle::plain()).clicked() {
                    self.host.events.borrow_mut().push(CreditsEvent::Close);
                }
            });
        });
    }
}

fn credit_card(
    ui: &mut egui::Ui,
    chrome: &PanelChrome,
    title: &str,
    license: &str,
    note: &str,
) {
    egui::Frame::NONE
        .fill(chrome.colors.list_stripe_bg32())
        .stroke(egui::Stroke::new(1.0, chrome.colors.panel_border32()))
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.strong(title);
            ui.small(format!("License: {license}"));
            ui.label(note);
        });
}

struct CreditsPanelContent {
    nest: NestContent,
}

impl PanelContent for CreditsPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        self.nest.ui(ui, chrome);
    }
}

pub fn credits_panel_spec(host: &CreditsHost, floating_rect: egui::Rect) -> PanelSpec {
    let mut nest = NestContent::new(NestMode::Static);
    nest.push(CreditsComponent { host: host.clone() });
    PanelSpec {
        id: PanelId::new(CREDITS_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(CREDITS_TITLE, false, CREDITS_MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(CreditsPanelContent { nest }),
    }
}
