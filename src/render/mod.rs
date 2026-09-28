//! Render layer — wgpu pipeline (allowed: wgpu; never imported by core).
//!
//! R1: CanvasRenderer (chunk upload, dirty rects), overlays. R0 (F4): nothing
//! rendered yet — projections compile against the core interface only.

pub mod canvas;
pub mod gizmo;
pub mod overlay;
pub mod projection;

pub struct RendererState {
    renderer: egui_wgpu::Renderer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

/// Color format egui must render into: the one the surface actually negotiated.
///
/// Hard-coding `Bgra8UnormSrgb` breaks every surface whose configuration picks
/// a different format (for example `Rgba8UnormSrgb` on Vulkan). wgpu then
/// rejects the pipeline at `set_pipeline` time with
/// "Render pipeline targets are incompatible with render pass".
fn egui_target_format(config: &wgpu::SurfaceConfiguration) -> wgpu::TextureFormat {
    config.format
}

impl RendererState {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    ) -> Self {
        let renderer = egui_wgpu::Renderer::new(
            &device,
            egui_target_format(&config),
            egui_wgpu::RendererOptions::default(),
        );

        Self {
            renderer,
            device,
            queue,
            surface,
            config,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn render_frame(
        &mut self,
        window: &winit::window::Window,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
        clear_color: egui::Color32,
    ) -> Result<(), ()> {
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(st) => st,
            wgpu::CurrentSurfaceTexture::Suboptimal(st) => st,
            _ => return Err(()),
        };

        let texture_view = surface_texture.texture.create_view(&Default::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("main"),
            });

        for (id, image_deltas) in &textures_delta.set {
            for image_delta in image_deltas {
                self.renderer
                    .update_texture(&self.device, &self.queue, *id, image_delta);
            }
        }
        for id in &textures_delta.free {
            self.renderer.free_texture(id);
        }

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [window.inner_size().width, window.inner_size().height],
            pixels_per_point: window.scale_factor() as f32,
        };

        self.renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            paint_jobs,
            &screen_descriptor,
        );

        let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &texture_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(to_wgpu_color(clear_color)),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        let mut render_pass = render_pass.forget_lifetime();
        self.renderer
            .render(&mut render_pass, paint_jobs, &screen_descriptor);

        drop(render_pass);

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(surface_texture);

        Ok(())
    }
}

fn to_wgpu_color(c: egui::Color32) -> wgpu::Color {
    let [r, g, b, a] = egui::Rgba::from(c).to_array();
    wgpu::Color {
        r: r as f64,
        g: g as f64,
        b: b as f64,
        a: a as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_format(format: wgpu::TextureFormat) -> wgpu::SurfaceConfiguration {
        wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: 640,
            height: 480,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::default(),
        }
    }

    #[test]
    fn egui_target_format_follows_the_negotiated_surface_format() {
        let rgba = config_with_format(wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(
            egui_target_format(&rgba),
            wgpu::TextureFormat::Rgba8UnormSrgb
        );

        let bgra = config_with_format(wgpu::TextureFormat::Bgra8UnormSrgb);
        assert_eq!(
            egui_target_format(&bgra),
            wgpu::TextureFormat::Bgra8UnormSrgb
        );
    }
}
