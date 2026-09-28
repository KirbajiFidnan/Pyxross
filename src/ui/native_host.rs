use crate::render::RendererState;

use super::panel_registry::PanelId;
use super::workspace_persistence::PanelGeometry;
use egui::ViewportId;
use std::sync::Arc;
use std::time::Instant;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, WindowId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeHostError {
    WindowCreation,
    SurfaceCreation,
    AdapterUnavailable,
    DeviceRequest,
    SurfaceConfiguration,
}

#[derive(Clone, Copy, Debug)]
pub struct NativeHostSpec {
    pub panel: PanelId,
    pub viewport: ViewportId,
    pub geometry: Option<PanelGeometry>,
}

#[derive(Default)]
struct GeometryCache {
    position: Option<winit::dpi::PhysicalPosition<i32>>,
    size: Option<winit::dpi::PhysicalSize<u32>>,
    scale_factor: Option<f32>,
}

impl GeometryCache {
    fn update(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::Moved(position) => self.position = Some(*position),
            WindowEvent::Resized(size) => self.size = Some(*size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.update_scale_factor(*scale_factor as f32)
            }
            WindowEvent::Destroyed => {}
            _ => {}
        }
    }

    fn update_scale_factor(&mut self, scale_factor: f32) {
        self.scale_factor = Some(scale_factor);
    }

    fn geometry(&self) -> Option<PanelGeometry> {
        Some(logical_geometry(
            self.position?,
            self.size?,
            self.scale_factor?,
        ))
    }

    fn screen_rect(&self, egui_ctx: &egui::Context) -> Option<egui::Rect> {
        let size = self.size?;
        let scale_factor = self.scale_factor?;
        let pixels_per_point = egui_ctx.zoom_factor() * scale_factor;
        let size_in_points = egui::vec2(
            size.width as f32 / pixels_per_point,
            size.height as f32 / pixels_per_point,
        );
        (size_in_points.x > 0.0 && size_in_points.y > 0.0)
            .then(|| egui::Rect::from_min_size(egui::Pos2::ZERO, size_in_points))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum HostLifecycle {
    #[default]
    Alive,
    Closing,
}

impl HostLifecycle {
    fn update(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => *self = Self::Closing,
            _ => {}
        }
    }

    const fn is_closing(self) -> bool {
        matches!(self, Self::Closing)
    }
}

pub struct NativeWorkspaceHost {
    panel: PanelId,
    ctx: egui::Context,
    preview_texture: Option<egui::TextureHandle>,
    renderer: RendererState,
    egui_state: egui_winit::State,
    geometry: GeometryCache,
    lifecycle: HostLifecycle,
    window: Arc<Window>,
    start_time: Instant,
    viewport_id: ViewportId,
}

fn logical_geometry(
    position: winit::dpi::PhysicalPosition<i32>,
    size: winit::dpi::PhysicalSize<u32>,
    scale: f32,
) -> PanelGeometry {
    PanelGeometry::new(
        position.x as f32 / scale,
        position.y as f32 / scale,
        size.width as f32 / scale,
        size.height as f32 / scale,
    )
}

fn native_host_backends() -> wgpu::Backends {
    #[cfg(target_os = "linux")]
    {
        wgpu::Backends::VULKAN
    }

    #[cfg(not(target_os = "linux"))]
    {
        wgpu::Backends::all()
    }
}

impl NativeWorkspaceHost {
    pub fn new(
        event_loop: &ActiveEventLoop,
        spec: NativeHostSpec,
    ) -> Result<Self, NativeHostError> {
        let mut attributes = WindowAttributes::default()
            .with_inner_size(LogicalSize::new(640.0, 480.0))
            .with_title(Self::title(spec.panel));
        if let Some(geometry) = spec.geometry {
            attributes = attributes
                .with_position(LogicalPosition::new(
                    f64::from(geometry.x),
                    f64::from(geometry.y),
                ))
                .with_inner_size(LogicalSize::new(
                    f64::from(geometry.width),
                    f64::from(geometry.height),
                ));
        }
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|_| NativeHostError::WindowCreation)?,
        );

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: native_host_backends(),
            ..wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
                event_loop.owned_display_handle(),
            ))
        });

        let surface = instance
            .create_surface(window.clone())
            .map_err(|_| NativeHostError::SurfaceCreation)?;

        let size = window.inner_size();
        let geometry = GeometryCache {
            position: window.outer_position().ok(),
            size: Some(size),
            scale_factor: Some(window.scale_factor() as f32),
        };
        let start_time = Instant::now();
        let viewport_id = spec.viewport;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|_| NativeHostError::AdapterUnavailable)?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("workspace"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|_| NativeHostError::DeviceRequest)?;
        let config = surface
            .get_default_config(&adapter, size.width, size.height)
            .ok_or(NativeHostError::SurfaceConfiguration)?;
        surface.configure(&device, &config);
        let renderer = RendererState::new(device, queue, surface, config);
        let ctx = egui::Context::default();
        let egui_state =
            egui_winit::State::new(ctx.clone(), spec.viewport, &window, None, None, None);

        Ok(Self {
            panel: spec.panel,
            ctx,
            preview_texture: None,
            renderer,
            egui_state,
            geometry,
            lifecycle: HostLifecycle::Alive,
            window,
            start_time,
            viewport_id,
        })
    }

    pub const fn panel(&self) -> PanelId {
        self.panel
    }
    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }
    pub fn context(&self) -> egui::Context {
        self.ctx.clone()
    }

    pub fn set_preview_image(&mut self, image: egui::ColorImage) -> egui::TextureId {
        if let Some(texture) = &mut self.preview_texture {
            texture.set(image, egui::TextureOptions::NEAREST);
        } else {
            self.preview_texture = Some(self.ctx.load_texture(
                "workspace-preview",
                image,
                egui::TextureOptions::NEAREST,
            ));
        }
        self.preview_texture
            .as_ref()
            .expect("preview texture is initialized")
            .id()
    }

    pub fn handle_window_event(&mut self, event: &WindowEvent) -> bool {
        self.geometry.update(event);
        self.lifecycle.update(event);
        if self.lifecycle.is_closing() {
            return false;
        }
        self.egui_state.on_window_event(&self.window, event).repaint
    }

    pub fn take_egui_input(&mut self) -> egui::RawInput {
        // Build input from cached geometry; egui-winit's take_egui_input calls
        // window.inner_size(), which panics when the X11 window is destroyed.
        let input = self.egui_state.egui_input_mut();
        input.time = Some(self.start_time.elapsed().as_secs_f64());
        input.viewport_id = self.viewport_id;
        if let Some(screen_rect) = self.geometry.screen_rect(&self.ctx) {
            input.screen_rect = Some(screen_rect);
        }
        if let Some(scale_factor) = self.geometry.scale_factor {
            input
                .viewports
                .entry(self.viewport_id)
                .or_default()
                .native_pixels_per_point = Some(scale_factor);
        }
        input.take()
    }

    pub fn handle_platform_output(&mut self, output: egui::PlatformOutput) {
        if self.lifecycle.is_closing() {
            return;
        }
        self.egui_state.handle_platform_output(&self.window, output);
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.lifecycle.is_closing() {
            return;
        }
        self.renderer.resize(width, height);
    }

    pub fn render_frame(
        &mut self,
        paint_jobs: &[egui::ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
        clear_color: egui::Color32,
    ) -> Result<(), ()> {
        if self.lifecycle.is_closing() {
            return Err(());
        }
        self.renderer
            .render_frame(&self.window, paint_jobs, textures_delta, clear_color)
    }

    pub fn request_redraw(&self) {
        if self.lifecycle.is_closing() {
            return;
        }
        self.window.request_redraw();
    }

    pub fn geometry(&self) -> Option<PanelGeometry> {
        self.geometry.geometry()
    }

    const fn title(panel: PanelId) -> &'static str {
        match panel {
            PanelId::PanelA => "Pyxross Panel A",
            PanelId::Preview => "Pyxross Preview",
            PanelId::Playback => "Pyxross Playback",
            PanelId::FrameEditor => "Pyxross Frame Editor",
            PanelId::Layers => "Pyxross Layers",
            PanelId::ColorPalette => "Pyxross Color Palette",
            PanelId::TilePalette => "Pyxross Tile Palette",
            PanelId::Animations => "Pyxross Animations",
            PanelId::Toolbox => "Pyxross Toolbox",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_persistence_uses_logical_units() {
        let geometry = logical_geometry(
            winit::dpi::PhysicalPosition::new(200, 100),
            winit::dpi::PhysicalSize::new(1600, 1200),
            2.0,
        );
        assert_eq!(geometry, PanelGeometry::new(100.0, 50.0, 800.0, 600.0));
    }

    #[test]
    fn cached_geometry_remains_available_after_window_destruction() {
        // Given
        let mut cache = GeometryCache::default();
        cache.update(&WindowEvent::Moved(winit::dpi::PhysicalPosition::new(
            200, 100,
        )));
        cache.update(&WindowEvent::Resized(winit::dpi::PhysicalSize::new(
            1600, 1200,
        )));
        cache.update_scale_factor(2.0);

        // When
        cache.update(&WindowEvent::Destroyed);

        // Then
        assert_eq!(
            cache.geometry(),
            Some(PanelGeometry::new(100.0, 50.0, 800.0, 600.0))
        );
    }

    #[test]
    fn lifecycle_transitions_to_closing_on_close_or_destroy() {
        // Given
        let mut lifecycle = HostLifecycle::default();
        assert!(!lifecycle.is_closing(), "fresh lifecycle is alive");

        // When: OS close is requested
        lifecycle.update(&WindowEvent::CloseRequested);

        // Then
        assert!(
            lifecycle.is_closing(),
            "CloseRequested must mark host as closing"
        );

        // And when Destroyed is received
        let mut lifecycle = HostLifecycle::default();
        lifecycle.update(&WindowEvent::Destroyed);
        assert!(
            lifecycle.is_closing(),
            "Destroyed must mark host as closing"
        );
    }

    #[test]
    fn native_host_backend_selection_is_platform_scoped() {
        let backends = native_host_backends();

        #[cfg(target_os = "linux")]
        assert_eq!(backends, wgpu::Backends::VULKAN);

        #[cfg(not(target_os = "linux"))]
        assert_eq!(backends, wgpu::Backends::all());
    }
}
