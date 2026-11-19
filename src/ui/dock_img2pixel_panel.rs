//! Floating "Import Image 2 Pixel" dock panel (id 217).
//!
//! Converts any loaded RGBA image into crisp pixel art using
//! `crate::core::img2pixel` with a tabbed controls sidebar (Size / Adjust /
//! Palette), live NEAREST preview with pinned Before/After toggle, and static
//! footer actions that never clip or scroll off-screen.

use std::cell::RefCell;
use std::rc::Rc;

use egui::vec2;

use crate::core::img2pixel::{
    process_image_to_pixel, ColorDistanceMode, DitherMode, Img2PixelOutput, Img2PixelParams,
    PaletteMode, SampleMode,
};
use crate::ui::panel_dock::nest::{nest_is_usable, nest_viewport};
use crate::ui::panel_dock::{
    ButtonStyle, PanelChrome, PanelContent, PanelId, PanelMetadata, PanelPlacement, PanelSpec,
};

/// Dock id for the Import Image 2 Pixel floating panel.
pub const IMPORT_IMG2PIXEL_PANEL_ID: u64 = 217;

const TITLE: &str = "Import Image 2 Pixel";
const MIN_SIZE: egui::Vec2 = vec2(620.0, 440.0);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum ControlTab {
    #[default]
    Size,
    Adjust,
    Palette,
}

impl ControlTab {
    const ALL: [Self; 3] = [Self::Size, Self::Adjust, Self::Palette];

    fn label(self) -> &'static str {
        match self {
            Self::Size => "Size",
            Self::Adjust => "Adjust",
            Self::Palette => "Palette",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Img2PixelEvent {
    ImportAsNewDocument {
        name: String,
        output: Img2PixelOutput,
    },
    ImportToActiveLayer {
        output: Img2PixelOutput,
    },
    Cancel,
}

pub struct Img2PixelHost {
    pub source_name: String,
    pub src_w: u32,
    pub src_h: u32,
    pub src_rgba: Vec<u8>,
    pub project_palette: Vec<[u8; 4]>,
    pub has_active_project: bool,
    pub params: Img2PixelParams,
    pub show_original: bool,
    active_tab: ControlTab,
    last_params: Option<Img2PixelParams>,
    cached_output: Option<Img2PixelOutput>,
    preview_texture: Option<egui::TextureHandle>,
    source_texture: Option<egui::TextureHandle>,
    pub events: Vec<Img2PixelEvent>,
}

impl Default for Img2PixelHost {
    fn default() -> Self {
        Self {
            source_name: String::from("PixelArt"),
            src_w: 1,
            src_h: 1,
            src_rgba: vec![0, 0, 0, 0],
            project_palette: Vec::new(),
            has_active_project: false,
            params: Img2PixelParams::default(),
            show_original: false,
            active_tab: ControlTab::Size,
            last_params: None,
            cached_output: None,
            preview_texture: None,
            source_texture: None,
            events: Vec::new(),
        }
    }
}

impl Img2PixelHost {
    pub fn seed(
        &mut self,
        source_name: String,
        src_w: u32,
        src_h: u32,
        src_rgba: Vec<u8>,
        project_palette: Vec<[u8; 4]>,
        has_active_project: bool,
    ) {
        self.source_name = source_name;
        self.src_w = src_w.max(1);
        self.src_h = src_h.max(1);
        self.src_rgba = src_rgba;
        self.project_palette = project_palette;
        self.has_active_project = has_active_project;
        self.params = Img2PixelParams::for_source(self.src_w, self.src_h);
        self.show_original = false;
        self.active_tab = ControlTab::Size;
        self.last_params = None;
        self.cached_output = None;
        self.preview_texture = None;
        self.source_texture = None;
        self.events.clear();
    }

    pub fn take_events(&mut self) -> Vec<Img2PixelEvent> {
        std::mem::take(&mut self.events)
    }

    fn ensure_output(&mut self, ctx: &egui::Context) -> &Img2PixelOutput {
        if self.last_params.as_ref() != Some(&self.params) || self.cached_output.is_none() {
            let out = process_image_to_pixel(
                self.src_w,
                self.src_h,
                &self.src_rgba,
                &self.project_palette,
                &self.params,
            );
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [out.width as usize, out.height as usize],
                &out.rgba,
            );
            self.preview_texture = Some(ctx.load_texture(
                "img2pixel_preview",
                image,
                egui::TextureOptions::NEAREST,
            ));
            self.last_params = Some(self.params.clone());
            self.cached_output = Some(out);
        }
        if self.source_texture.is_none() {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [self.src_w as usize, self.src_h as usize],
                &self.src_rgba,
            );
            self.source_texture = Some(ctx.load_texture(
                "img2pixel_source",
                image,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.cached_output.as_ref().unwrap()
    }
}

struct Img2PixelPanelContent {
    host: Rc<RefCell<Img2PixelHost>>,
}

impl Img2PixelPanelContent {
    fn render_body(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let mut host = self.host.borrow_mut();
        host.ensure_output(ui.ctx());

        let total = ui.available_size();
        let footer_h = 38.0;
        let body_h = (total.y - footer_h).max(160.0);
        let controls_w = (total.x * 0.40).clamp(240.0, 290.0);
        let src_w = host.src_w;
        let src_h = host.src_h;
        let aspect = src_h as f32 / src_w.max(1) as f32;

        // Main two-column area (fixed height above pinned footer).
        ui.allocate_ui_with_layout(
            vec2(total.x, body_h),
            egui::Layout::left_to_right(egui::Align::Min),
            |ui| {
                // Left column: tabbed settings.
                ui.allocate_ui_with_layout(
                    vec2(controls_w, body_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        // Static tab bar.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let tab_w = ((controls_w - 8.0) / 3.0).max(60.0);
                            for tab in ControlTab::ALL {
                                let selected = host.active_tab == tab;
                                let resp = ui.add_sized(
                                    vec2(tab_w, 24.0),
                                    egui::Button::selectable(selected, tab.label()),
                                );
                                if resp.clicked() {
                                    host.active_tab = tab;
                                }
                            }
                        });
                        ui.separator();

                        let tab_body_h = (body_h - 34.0).max(100.0);
                        egui::ScrollArea::vertical()
                            .id_salt("img2pixel_tab_scroll")
                            .auto_shrink([false, false])
                            .max_height(tab_body_h)
                            .show(ui, |ui| {
                                ui.spacing_mut().slider_width = (controls_w - 135.0).max(80.0);
                                ui.add_space(4.0);
                                match host.active_tab {
                                    ControlTab::Size => {
                                        ui.strong("Output Size");
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            let mut w = host.params.target_width;
                                            let mut h = host.params.target_height;
                                            ui.label("W");
                                            if ui
                                                .add(egui::DragValue::new(&mut w).range(1..=512))
                                                .changed()
                                            {
                                                host.params.target_width = w;
                                                if host.params.lock_aspect {
                                                    host.params.target_height =
                                                        ((w as f32 * aspect).round() as u32)
                                                            .clamp(1, 512);
                                                }
                                            }
                                            ui.label("H");
                                            if ui
                                                .add(egui::DragValue::new(&mut h).range(1..=512))
                                                .changed()
                                            {
                                                host.params.target_height = h;
                                                if host.params.lock_aspect {
                                                    host.params.target_width =
                                                        ((h as f32 / aspect.max(1e-4)).round()
                                                            as u32)
                                                            .clamp(1, 512);
                                                }
                                            }
                                            ui.checkbox(&mut host.params.lock_aspect, "Lock");
                                        });
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.weak("Presets:");
                                            for preset in [16u32, 32, 64, 128] {
                                                if ui.small_button(format!("{preset}")).clicked() {
                                                    host.params.target_width = preset;
                                                    host.params.target_height =
                                                        if host.params.lock_aspect {
                                                            ((preset as f32 * aspect).round()
                                                                as u32)
                                                                .clamp(1, 512)
                                                        } else {
                                                            preset
                                                        };
                                                }
                                            }
                                        });

                                        ui.add_space(10.0);
                                        ui.strong("Sampling");
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Filter");
                                            egui::ComboBox::from_id_salt("i2p_sample_mode")
                                                .selected_text(host.params.sample_mode.label())
                                                .show_ui(ui, |ui| {
                                                    for m in SampleMode::ALL {
                                                        ui.selectable_value(
                                                            &mut host.params.sample_mode,
                                                            m,
                                                            m.label(),
                                                        );
                                                    }
                                                });
                                        });

                                        ui.add_space(10.0);
                                        ui.strong("Transparency");
                                        ui.add_space(4.0);
                                        ui.add(
                                            egui::Slider::new(
                                                &mut host.params.alpha_threshold,
                                                0..=255,
                                            )
                                            .text("Alpha Cut"),
                                        );
                                    }
                                    ControlTab::Adjust => {
                                        ui.strong("Detail");
                                        ui.add_space(4.0);
                                        ui.add(
                                            egui::Slider::new(&mut host.params.blur, 0.0..=4.0)
                                                .text("Blur"),
                                        );
                                        ui.add(
                                            egui::Slider::new(&mut host.params.sharpen, 0.0..=2.0)
                                                .text("Sharpen"),
                                        );

                                        ui.add_space(10.0);
                                        ui.strong("Dithering");
                                        ui.add_space(4.0);
                                        let can_dither =
                                            host.params.palette_mode != PaletteMode::Original;
                                        ui.add_enabled_ui(can_dither, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label("Pattern");
                                                egui::ComboBox::from_id_salt("i2p_dither")
                                                    .selected_text(host.params.dither_mode.label())
                                                    .show_ui(ui, |ui| {
                                                        for m in DitherMode::ALL {
                                                            ui.selectable_value(
                                                                &mut host.params.dither_mode,
                                                                m,
                                                                m.label(),
                                                            );
                                                        }
                                                    });
                                            });
                                            if host.params.dither_mode != DitherMode::None {
                                                ui.add(
                                                    egui::Slider::new(
                                                        &mut host.params.dither_amount,
                                                        0.0..=1.0,
                                                    )
                                                    .text("Amount"),
                                                );
                                            }
                                        });
                                        if !can_dither {
                                            ui.weak("Select K-Means or Project Palette to enable.");
                                        }
                                    }
                                    ControlTab::Palette => {
                                        ui.strong("Color Reduction");
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Palette");
                                            egui::ComboBox::from_id_salt("i2p_palette_mode")
                                                .selected_text(host.params.palette_mode.label())
                                                .show_ui(ui, |ui| {
                                                    for m in PaletteMode::ALL {
                                                        ui.selectable_value(
                                                            &mut host.params.palette_mode,
                                                            m,
                                                            m.label(),
                                                        );
                                                    }
                                                });
                                        });
                                        if host.params.palette_mode == PaletteMode::ExtractKMeans {
                                            ui.add(
                                                egui::Slider::new(
                                                    &mut host.params.kmeans_colors,
                                                    2..=64,
                                                )
                                                .text("Colors (K)"),
                                            );
                                        }
                                        if host.params.palette_mode != PaletteMode::Original {
                                            ui.horizontal(|ui| {
                                                ui.label("Metric");
                                                egui::ComboBox::from_id_salt("i2p_distance")
                                                    .selected_text(
                                                        host.params.color_distance.label(),
                                                    )
                                                    .show_ui(ui, |ui| {
                                                        for m in ColorDistanceMode::ALL {
                                                            ui.selectable_value(
                                                                &mut host.params.color_distance,
                                                                m,
                                                                m.label(),
                                                            );
                                                        }
                                                    });
                                            });
                                        }

                                        ui.add_space(10.0);
                                        ui.strong("Tone & Color");
                                        ui.add_space(4.0);
                                        ui.add(
                                            egui::Slider::new(
                                                &mut host.params.brightness,
                                                -1.0..=1.0,
                                            )
                                            .text("Brightness"),
                                        );
                                        ui.add(
                                            egui::Slider::new(
                                                &mut host.params.contrast,
                                                0.2..=3.0,
                                            )
                                            .text("Contrast"),
                                        );
                                        ui.add(
                                            egui::Slider::new(
                                                &mut host.params.saturation,
                                                0.0..=2.0,
                                            )
                                            .text("Saturation"),
                                        );
                                        ui.add(
                                            egui::Slider::new(
                                                &mut host.params.hue_shift,
                                                -180.0..=180.0,
                                            )
                                            .text("Hue"),
                                        );
                                        ui.add(
                                            egui::Slider::new(&mut host.params.gamma, 0.2..=3.0)
                                                .text("Gamma"),
                                        );
                                    }
                                }
                            });
                    },
                );

                ui.separator();

                // Right column: pinned Before/After header + preview well + swatches.
                let preview_w = ui.available_width().max(120.0);
                ui.allocate_ui_with_layout(
                    vec2(preview_w, body_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        // Pinned toggle on the LEFT so long filenames never push it off-screen.
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut host.show_original, false, "After");
                            ui.selectable_value(&mut host.show_original, true, "Before");
                            ui.separator();
                            let dims = format!(
                                "{}x{} -> {}x{}",
                                host.src_w,
                                host.src_h,
                                host.params.target_width,
                                host.params.target_height,
                            );
                            ui.add(egui::Label::new(dims).truncate());
                        });

                        let palette_strip_h = 22.0;
                        let well_h = (ui.available_height() - palette_strip_h - 6.0).max(100.0);
                        let well_rect = ui.allocate_space(vec2(ui.available_width(), well_h)).1;
                        ui.painter()
                            .rect_filled(well_rect, 2.0, chrome.colors.list_bg32());

                        let tex_info = if host.show_original {
                            host.source_texture
                                .as_ref()
                                .map(|t| (t.id(), host.src_w as f32, host.src_h as f32))
                        } else {
                            host.preview_texture.as_ref().map(|t| {
                                (
                                    t.id(),
                                    host.params.target_width as f32,
                                    host.params.target_height as f32,
                                )
                            })
                        };
                        if let Some((tex_id, tw, th)) = tex_info {
                            let pad = 8.0;
                            let avail_w = (well_rect.width() - pad * 2.0).max(1.0);
                            let avail_h = (well_rect.height() - pad * 2.0).max(1.0);
                            let scale = (avail_w / tw.max(1.0)).min(avail_h / th.max(1.0));
                            let draw_size = vec2(tw * scale, th * scale);
                            let draw_rect =
                                egui::Rect::from_center_size(well_rect.center(), draw_size);
                            ui.painter().image(
                                tex_id,
                                draw_rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }

                        if let Some(pal) = host
                            .cached_output
                            .as_ref()
                            .and_then(|o| o.extracted_palette.as_ref())
                        {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                                let max_swatches = ((preview_w / 14.0).floor() as usize).min(32);
                                for &[r, g, b, _] in pal.iter().take(max_swatches) {
                                    let (rect, _) = ui.allocate_exact_size(
                                        vec2(12.0, 12.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().rect_filled(
                                        rect,
                                        0.0,
                                        egui::Color32::from_rgb(r, g, b),
                                    );
                                }
                            });
                        }
                    },
                );
            },
        );

        // Static, always-visible footer bar.
        ui.separator();
        ui.horizontal(|ui| {
            if chrome.button(ui, "Reset", ButtonStyle::plain()).clicked() {
                host.params = Img2PixelParams::for_source(src_w, src_h);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if chrome.button(ui, "Cancel", ButtonStyle::plain()).clicked() {
                    host.events.push(Img2PixelEvent::Cancel);
                }
                if host.has_active_project
                    && chrome
                        .button(ui, "To Layer", ButtonStyle::plain())
                        .on_hover_text("Import to Active Layer as Transform")
                        .clicked()
                {
                    if let Some(output) = host.cached_output.clone() {
                        host.events
                            .push(Img2PixelEvent::ImportToActiveLayer { output });
                    }
                }
                if chrome
                    .button(ui, "New Document", ButtonStyle::plain())
                    .on_hover_text("Import as New Document")
                    .clicked()
                {
                    if let Some(output) = host.cached_output.clone() {
                        let name = host.source_name.clone();
                        host.events.push(Img2PixelEvent::ImportAsNewDocument {
                            name,
                            output,
                        });
                    }
                }
            });
        });
    }
}

impl PanelContent for Img2PixelPanelContent {
    fn ui(&mut self, ui: &mut egui::Ui, chrome: &PanelChrome) {
        let full = ui.available_rect_before_wrap();
        ui.set_min_size(full.size());
        let viewport = nest_viewport(full);
        if !nest_is_usable(viewport) {
            return;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(viewport), |inner| {
            inner.set_clip_rect(viewport.intersect(inner.clip_rect()));
            self.render_body(inner, chrome);
        });
    }
}

pub fn img2pixel_panel_spec(
    host: Rc<RefCell<Img2PixelHost>>,
    floating_rect: egui::Rect,
) -> PanelSpec {
    PanelSpec {
        id: PanelId::new(IMPORT_IMG2PIXEL_PANEL_ID),
        metadata: PanelMetadata {
            resizable: true,
            ..PanelMetadata::new(TITLE, false, MIN_SIZE)
        },
        placement: PanelPlacement::Floating,
        floating_rect,
        content: Box::new(Img2PixelPanelContent { host }),
    }
}
