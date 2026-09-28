//! Projection quad pass: shared canvas texture → N projection surfaces [D65].
//!
//! Renders the shared canvas texture onto a projection surface via a
//! fullscreen textured triangle with a nearest-neighbor sampler, at integer
//! scale `floor(monitor_size / canvas_size)` (see [`integer_scale`]). The
//! quad spans the full target with UVs (0,0)-(1,1); integer scale guarantees
//! texel-aligned sampling, so no half-texel UV offset is needed (deferred —
//! edge bleed on downscale is a non-issue at integer scales).
//!
//! Frame pacing follows D65: a projection re-presents only when the canvas
//! version counter changed (or the surface was resized, i.e. the integer
//! scale changed) — see [`should_present`] and [`ProjectionRenderPass::render`].
//!
//! §9.4 constraints honored here:
//! - No surface or device is created in this module — the caller owns the
//!   `wgpu::Device` (one instance/device serves the main window + all
//!   projection surfaces) and the projection surfaces themselves.
//! - The quad pass shares the device with the `egui_wgpu::Renderer` without
//!   conflict (separate pipelines/bind groups).
//! - `surface_died` handling is a platform-layer concern (wayland thread),
//!   out of scope for this render pass.

use crate::core::math::Rect2i;
use std::mem::size_of;

/// Fullscreen triangle vertices: clip-space position + texture UV.
///
/// The triangle spans (-1,-1) → (3,-1) → (-1,3) in clip space, covering the
/// whole viewport with no degenerate edge. UVs are chosen so the visible
/// [-1,1]² region maps to texture (0,0)-(1,1).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

/// Vertex data for the fullscreen triangle (3 vertices × 4 f32).
const FULLSCREEN_TRIANGLE: [[f32; 4]; 3] = [
    [-1.0, -1.0, 0.0, 1.0], // bottom-left  → UV (0, 1)
    [3.0, -1.0, 2.0, 1.0],  // bottom-right → UV (2, 1)
    [-1.0, 3.0, 0.0, -1.0], // top-left     → UV (0, -1)
];

/// Inline WGSL: fullscreen-triangle vertex shader + canvas-sampling fragment
/// shader. The fragment outputs straight RGBA (no premultiply); the
/// nearest-neighbor sampler enforces texel-aligned sampling.
const SHADER: &str = r#"
struct VsIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var canvas_texture: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_pos = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(canvas_texture, canvas_sampler, in.uv);
}
"#;

/// Projection quad pass: draws the shared canvas texture into a projection
/// surface's target view.
///
/// One instance serves all projection surfaces; the caller owns the device
/// and the surfaces (§9.4). The pipeline targets `Bgra8UnormSrgb` (projection
/// surfaces use bgra like the main surface) with default blending (overwrite).
pub struct ProjectionRenderPass {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    vertex_buffer: wgpu::Buffer,
    device: wgpu::Device,
    /// Integer scale of the last presented frame; a scale change (surface
    /// resize) forces a re-present even when the canvas version is unchanged.
    last_scale: u32,
}

impl ProjectionRenderPass {
    /// Builds the quad pass: nearest-neighbor sampler, texture+sampler bind
    /// group layout, and a `Bgra8UnormSrgb` render pipeline sampling the
    /// canvas texture.
    pub fn new(device: &wgpu::Device) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("projection-nearest-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_min_clamp: 0.0,
            lod_max_clamp: 32.0,
            compare: None,
            anisotropy_clamp: 1,
            border_color: None,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("projection-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("projection-quad-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("projection-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let vertex_buffer_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: size_of::<[f32; 2]>() as u64,
                    shader_location: 1,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("projection-quad-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_buffer_layout)],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Bgra8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // Vertex buffer: 3 vertices × 4 f32 = 48 bytes, filled via
        // `mapped_at_creation` (no queue is available in `new`).
        let mut vertex_data = Vec::with_capacity(FULLSCREEN_TRIANGLE.len() * 16);
        for v in FULLSCREEN_TRIANGLE {
            for c in v {
                vertex_data.extend_from_slice(&c.to_ne_bytes());
            }
        }
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("projection-quad-vertices"),
            size: vertex_data.len() as u64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        vertex_buffer
            .slice(..)
            .get_mapped_range_mut()
            .expect("freshly mapped vertex buffer is writable")
            .copy_from_slice(&vertex_data);
        vertex_buffer.unmap();

        Self {
            pipeline,
            sampler,
            bind_group_layout,
            vertex_buffer,
            device: device.clone(),
            last_scale: 0,
        }
    }

    /// Renders the canvas texture into `target_view`.
    ///
    /// D65 pacing: skips the draw (returns false) when the integer scale is
    /// unchanged AND the canvas version is unchanged since the last present.
    /// Otherwise draws the fullscreen quad (clearing to black first) and
    /// updates `*last_presented`.
    ///
    /// The canvas texture is bound by reference; the caller owns it (shared
    /// as `Arc<wgpu::Texture>` with an `AtomicU64` version counter — see the
    /// parallel `CanvasRenderer` task).
    ///
    /// The 8-parameter signature is the D65 compat contract with the platform
    /// layer; the argument count is intentional.
    #[expect(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        canvas_texture: &wgpu::Texture,
        canvas_size: (u32, u32),
        surface_size: (u32, u32),
        version: u64,
        last_presented: &mut u64,
    ) -> bool {
        let scale = integer_scale(canvas_size, surface_size);
        if scale == self.last_scale && !should_present(version, *last_presented) {
            return false;
        }

        let texture_view = canvas_texture.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("projection-canvas-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("projection"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
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
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        render_pass.draw(0..3, 0..1);
        drop(render_pass);

        *last_presented = version;
        self.last_scale = scale;
        true
    }
}

/// Integer scale for a projection surface: `floor(min(surface_w / canvas_w,
/// surface_h / canvas_h))`, with a minimum of 1.
///
/// A canvas larger than the surface displays 1:1 with cropping (the scale
/// never drops below 1). A degenerate (0,0) canvas or surface also yields 1.
pub fn integer_scale(canvas: (u32, u32), surface: (u32, u32)) -> u32 {
    if canvas.0 == 0 || canvas.1 == 0 || surface.0 == 0 || surface.1 == 0 {
        return 1;
    }
    let sx = surface.0 / canvas.0;
    let sy = surface.1 / canvas.1;
    sx.min(sy).max(1)
}

/// Canvas-space-of-surface rect: the scaled canvas clamped/centered in the
/// surface.
///
/// Scaled size = `canvas * scale`. If it overflows the surface (in either
/// axis) it is clamped to the surface with offset (0,0); otherwise it is
/// centered with integer division. R1 keeps this simple; the future
/// capture-mode output will consume it.
pub fn present_rect(scale: u32, canvas: (u32, u32), surface: (u32, u32)) -> Rect2i {
    let scaled_w = canvas.0.saturating_mul(scale);
    let scaled_h = canvas.1.saturating_mul(scale);
    let w = scaled_w.min(surface.0);
    let h = scaled_h.min(surface.1);
    if w < scaled_w || h < scaled_h {
        // Scaled canvas overflows the surface: clamp to the surface, offset 0.
        Rect2i::new(0, 0, w as i32, h as i32)
    } else {
        let x = (surface.0 - w) / 2;
        let y = (surface.1 - h) / 2;
        Rect2i::new(x as i32, y as i32, w as i32, h as i32)
    }
}

/// D65 pacing: a projection re-presents only when the canvas version changed.
pub fn should_present(version: u64, last_presented: u64) -> bool {
    version != last_presented
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_scale_floor_min() {
        // 22*32 = 704 ≤ 720, 23*32 = 736 > 720.
        assert_eq!(integer_scale((32, 32), (1280, 720)), 22);
    }

    #[test]
    fn integer_scale_exact_multiples() {
        assert_eq!(integer_scale((32, 32), (128, 128)), 4);
    }

    #[test]
    fn integer_scale_surface_smaller_than_canvas() {
        // 64×64 canvas over 32×32 surface → min scale 1.
        assert_eq!(integer_scale((64, 64), (32, 32)), 1);
    }

    #[test]
    fn integer_scale_non_square() {
        // min(1280/64, 720/32) = min(20, 22) = 20.
        assert_eq!(integer_scale((64, 32), (1280, 720)), 20);
    }

    #[test]
    fn integer_scale_zero_canvas() {
        assert_eq!(integer_scale((0, 0), (1280, 720)), 1);
    }

    #[test]
    fn integer_scale_zero_surface() {
        assert_eq!(integer_scale((32, 32), (0, 0)), 1);
    }

    #[test]
    fn present_rect_centered() {
        // scale 2 on 10×10 canvas into 40×30 surface: scaled 20×20,
        // offset (40-20)/2 = 10, (30-20)/2 = 5 → (10, 5, 20, 20).
        assert_eq!(
            present_rect(2, (10, 10), (40, 30)),
            Rect2i::new(10, 5, 20, 20)
        );
    }

    #[test]
    fn present_rect_clamped() {
        // scale 1, canvas 100×100 into 80×80 surface → clamped 80×80 at (0,0).
        assert_eq!(
            present_rect(1, (100, 100), (80, 80)),
            Rect2i::new(0, 0, 80, 80)
        );
    }

    #[test]
    fn present_rect_odd_centering_remainder() {
        // canvas 3×3, scale 1, surface 10×10 → offset (10-3)/2 = 3.
        assert_eq!(present_rect(1, (3, 3), (10, 10)), Rect2i::new(3, 3, 3, 3));
    }

    #[test]
    fn should_present_unchanged_version() {
        assert!(!should_present(7, 7));
    }

    #[test]
    fn should_present_changed_version() {
        assert!(should_present(8, 7));
    }

    #[test]
    fn should_present_first_present() {
        // last = 0, version = 5 → first present.
        assert!(should_present(5, 0));
    }

    #[test]
    fn integer_scale_monotonic_when_surface_shrinks() {
        let canvas = (32, 32);
        let big = integer_scale(canvas, (1280, 720));
        let small = integer_scale(canvas, (640, 360));
        assert!(small <= big);
    }

    #[test]
    fn present_rect_exact_fit() {
        // surface == scaled size → rect at (0,0) with full size.
        assert_eq!(
            present_rect(2, (10, 10), (20, 20)),
            Rect2i::new(0, 0, 20, 20)
        );
    }
}
