//! P01 Phase 0 spike — SCTK + wgpu layer-shell projection PoC.
//!
//! Opens a wlr-layer-shell surface (Overlay layer, top-right anchor) on the
//! chosen output (or the primary one), renders an animated test pattern
//! (checkerboard + moving bar) with wgpu, paced by `wl_surface.frame`
//! callbacks. Empty input region + `keyboard_interactivity: none` for
//! click-through. First present happens only after the first acked configure.
//!
//! Usage: `cargo run --example projection_spike [output-name]`
//! e.g. `cargo run --example projection_spike DP-1`
//!
//! Env overrides: `P0_SIZE=WxH` (logical size, default 480x320),
//! `P0_FRAMES=N` (auto-exit after N frames, default 0 = unlimited).
//! FPS is logged to stderr every 2s as `[P0] fps=...`.
//!
//! Exit: compositor closes the surface, Ctrl-C, or P0_FRAMES reached.

use std::ptr::NonNull;
use std::time::Instant;

use raw_window_handle::{
    RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::calloop::EventLoop,
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
};
use wayland_client::{
    delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_output, wl_region, wl_surface},
    Connection, Proxy, QueueHandle,
};
use wgpu::util::DeviceExt;

const REQUESTED_WIDTH: u32 = 480;
const REQUESTED_HEIGHT: u32 = 320;
const MIN_FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);
const FPS_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

fn main() {
    let target_output = std::env::args().nth(1);
    let (width, height) = match std::env::var("P0_SIZE") {
        Ok(s) => {
            let mut it = s.split('x');
            let w: u32 = it
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(REQUESTED_WIDTH);
            let h: u32 = it
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(REQUESTED_HEIGHT);
            (w, h)
        }
        Err(_) => (REQUESTED_WIDTH, REQUESTED_HEIGHT),
    };
    let max_frames: u64 = std::env::var("P0_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let conn = Connection::connect_to_env().expect("connect to wayland (WAYLAND_DISPLAY set?)");
    let (globals, event_queue) = registry_queue_init(&conn).expect("registry init");
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("wlr_layer_shell_v1 not available");

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });

    let mut app = SpikeApp {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        instance,
        layer: None,
        input_region: None,
        target_output,
        gpu: None,
        exit: false,
        width,
        height,
        scale: 1,
        start: Instant::now(),
        last_render: Instant::now(),
        last_fps_log: Instant::now(),
        frames_at_last_log: 0,
        max_frames,
        frames: 0,
    };

    let mut event_loop = EventLoop::try_new().expect("calloop event loop");
    let handle = event_loop.handle();
    WaylandSource::new(conn, event_queue)
        .insert(handle)
        .expect("insert wayland source");

    println!("spike: running (layer-shell projection)");
    loop {
        if let Err(e) = event_loop.dispatch(None, &mut app) {
            eprintln!("spike: dispatch error: {e:?}");
            break;
        }
        if app.exit {
            break;
        }
    }

    // Drop order (P01 §4.6.3): wgpu surface → SCTK surface → EventQueue → Display.
    drop(app.gpu.take());
    drop(app.layer.take());
    println!("spike: exited after {} frames", app.frames);
}

struct SpikeApp {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    instance: wgpu::Instance,
    layer: Option<LayerSurface>,
    input_region: Option<wl_region::WlRegion>,
    target_output: Option<String>,
    gpu: Option<Gpu>,
    exit: bool,
    width: u32,
    height: u32,
    scale: i32,
    start: Instant,
    last_render: Instant,
    last_fps_log: Instant,
    frames_at_last_log: u64,
    max_frames: u64,
    frames: u64,
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    format: wgpu::TextureFormat,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    time: f32,
    size: [f32; 2],
    _pad: f32,
}

const SPIKE_SHADER: &str = r#"
struct Uniforms {
    time: f32,
    size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(pos[vid], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = pos.xy;
    let t = u.time;
    let size = u.size;

    let cell = 32.0;
    let ix = floor(p.x / cell);
    let iy = floor(p.y / cell);
    let parity = (ix + iy) % 2.0;
    var color = mix(
        vec4<f32>(0.08, 0.10, 0.14, 0.80),
        vec4<f32>(0.85, 0.87, 0.92, 0.85),
        parity,
    );

    let bar_x = fract(t * 0.25) * (size.x + 64.0) - 32.0;
    if abs(p.x - bar_x) < 10.0 {
        color = vec4<f32>(0.95, 0.30, 0.20, 1.0);
    }

    let hue = 0.5 + 0.5 * sin(t);
    color = vec4<f32>(mix(color.rgb, vec3<f32>(hue, 0.3, 0.6), 0.15), color.a);

    color = vec4<f32>(color.rgb * color.a, color.a);
    return color;
}
"#;

impl SpikeApp {
    fn maybe_create_layer(&mut self, qh: &QueueHandle<Self>) {
        if self.layer.is_some() {
            return;
        }
        let output = match &self.target_output {
            Some(name) => {
                let found = self.output_state.outputs().find(|o| {
                    self.output_state.info(o).and_then(|i| i.name).as_deref() == Some(name.as_str())
                });
                match found {
                    Some(o) => {
                        println!("spike: targeting output '{name}'");
                        Some(o)
                    }
                    None => return,
                }
            }
            None => None,
        };
        let surface = self.compositor.create_surface(qh);
        // Wayland's default input region is the FULL surface — this explicit
        // empty region is what makes clicks pass through. Kept alive below.
        let region = self.compositor.wl_compositor().create_region(qh, ());
        region.add(0, 0, 0, 0);
        surface.set_input_region(Some(&region));
        println!("[P0] input region: empty (click-through)");
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("pyxross-spike"),
            output.as_ref(),
        );
        layer.set_anchor(Anchor::TOP | Anchor::RIGHT);
        layer.set_margin(24, 24, 0, 0);
        layer.set_size(REQUESTED_WIDTH, REQUESTED_HEIGHT);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(-1);
        layer.commit();
        self.layer = Some(layer);
        self.input_region = Some(region);
    }

    fn init_gpu(&mut self, conn: &Connection, phys_w: u32, phys_h: u32) {
        let layer = self.layer.as_ref().expect("layer created before gpu init");
        let wl_surface = layer.wl_surface();
        let raw_display_handle = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(
            NonNull::new(conn.backend().display_ptr() as *mut _).unwrap(),
        ));
        let raw_window_handle = RawWindowHandle::Wayland(WaylandWindowHandle::new(
            NonNull::new(wl_surface.id().as_ptr() as *mut _).unwrap(),
        ));
        let surface = unsafe {
            self.instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                    raw_display_handle: Some(raw_display_handle),
                    raw_window_handle,
                })
        }
        .expect("create wgpu surface");
        let gpu = Gpu::init(&self.instance, surface, phys_w, phys_h);
        self.width = phys_w;
        self.height = phys_h;
        self.gpu = Some(gpu);
    }

    fn render(&mut self, qh: &QueueHandle<Self>) {
        let gpu = match &mut self.gpu {
            Some(g) => g,
            None => return,
        };

        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Outdated => {
                self.layer
                    .as_ref()
                    .unwrap()
                    .wl_surface()
                    .frame(qh, self.layer.as_ref().unwrap().wl_surface().clone());
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("spike: surface lost, exiting");
                self.exit = true;
                return;
            }
        };

        let t = self.start.elapsed().as_secs_f32();
        gpu.queue.write_buffer(
            &gpu.uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                time: t,
                size: [self.width as f32, self.height as f32],
                _pad: 0.0,
            }),
        );

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("spike encoder"),
            });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("spike render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rpass.set_pipeline(&gpu.pipeline);
            rpass.set_bind_group(0, &gpu.bind_group, &[]);
            rpass.draw(0..3, 0..1);
        }
        gpu.queue.submit(Some(encoder.finish()));

        let layer = self.layer.as_ref().unwrap();
        layer
            .wl_surface()
            .damage_buffer(0, 0, self.width as i32, self.height as i32);
        // Request the next frame callback BEFORE present (wgpu Wayland contract:
        // present attaches the buffer and commits; the callback must be part of
        // the same commit cycle).
        layer.wl_surface().frame(qh, layer.wl_surface().clone());
        gpu.queue.present(frame);

        self.last_render = Instant::now();
        self.frames += 1;
        if self.max_frames > 0 && self.frames >= self.max_frames {
            self.exit = true;
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last_fps_log) >= FPS_LOG_INTERVAL {
            let fps = (self.frames - self.frames_at_last_log) as f64
                / now.duration_since(self.last_fps_log).as_secs_f64();
            eprintln!("[P0] fps={fps:.1}");
            self.last_fps_log = now;
            self.frames_at_last_log = self.frames;
        }
    }
}

impl Gpu {
    fn init(
        instance: &wgpu::Instance,
        surface: wgpu::Surface<'static>,
        width: u32,
        height: u32,
    ) -> Self {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .expect("no suitable adapter (GPU or lavapipe needed)");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("spike device"),
            ..Default::default()
        }))
        .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let alpha_mode = if caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            wgpu::CompositeAlphaMode::Opaque
        };

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spike shader"),
            source: wgpu::ShaderSource::Wgsl(SPIKE_SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spike bind group layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("spike pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("spike pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spike uniform buffer"),
            contents: bytemuck::bytes_of(&Uniforms {
                time: 0.0,
                size: [width as f32, height as f32],
                _pad: 0.0,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spike bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: wgpu::SurfaceColorSpace::Auto,
                width,
                height,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 1,
                alpha_mode,
                view_formats: vec![],
            },
        );

        Self {
            surface,
            device,
            queue,
            pipeline,
            bind_group,
            uniform_buffer,
            format,
        }
    }
}

impl CompositorHandler for SpikeApp {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        self.scale = new_factor;
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // P01 §4.6.5: render → present → request frame → wait → skip if unchanged.
        let now = Instant::now();
        if now.duration_since(self.last_render) < MIN_FRAME_INTERVAL {
            self.layer
                .as_ref()
                .unwrap()
                .wl_surface()
                .frame(qh, self.layer.as_ref().unwrap().wl_surface().clone());
            return;
        }
        self.render(qh);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for SpikeApp {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.maybe_create_layer(qh);
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        self.maybe_create_layer(qh);
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for SpikeApp {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        println!("spike: layer surface closed by compositor");
        self.exit = true;
    }

    fn configure(
        &mut self,
        conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        if w == 0 || h == 0 {
            self.width = REQUESTED_WIDTH;
            self.height = REQUESTED_HEIGHT;
        } else {
            self.width = w;
            self.height = h;
        }
        let phys_w = self.width * self.scale as u32;
        let phys_h = self.height * self.scale as u32;

        if let Some(gpu) = &mut self.gpu {
            gpu.surface.configure(
                &gpu.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: gpu.format,
                    color_space: wgpu::SurfaceColorSpace::Auto,
                    width: phys_w,
                    height: phys_h,
                    present_mode: wgpu::PresentMode::AutoVsync,
                    desired_maximum_frame_latency: 1,
                    alpha_mode: wgpu::CompositeAlphaMode::PreMultiplied,
                    view_formats: vec![],
                },
            );
            self.width = phys_w;
            self.height = phys_h;
        } else {
            self.init_gpu(conn, phys_w, phys_h);
        }

        // First present after the acked configure (P01 §4.6.2).
        self.render(qh);
    }
}

impl ProvidesRegistryState for SpikeApp {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(SpikeApp);
delegate_output!(SpikeApp);
delegate_layer!(SpikeApp);
delegate_registry!(SpikeApp);
delegate_noop!(SpikeApp: wl_region::WlRegion);
