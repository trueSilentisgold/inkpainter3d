//! The 2D view: one texture set's textures in UV space, where the brush also paints.

use crate::viewport::{
    COLOR_FORMAT, DEPTH_FORMAT, DisplayMode, SAMPLE_COUNT, Targets, Vertex, ViewportRenderer,
    srgb_to_linear,
};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::Vec2;
use ink_core::Model;

/// Pan, zoom and rotation of the 2D view.
#[derive(Clone, Debug)]
pub struct UvCamera {
    /// UV coordinate at the center of the view.
    pub center: Vec2,
    /// How much of UV space the view's height shows.
    pub height: f32,
    /// Rotation of the view in radians, clockwise on screen.
    pub angle: f32,
}

impl Default for UvCamera {
    fn default() -> Self {
        Self {
            center: Vec2::splat(0.5),
            height: 1.1,
            angle: 0.0,
        }
    }
}

impl UvCamera {
    /// Shows the whole UV square, keeping the rotation.
    pub fn frame(&mut self) {
        *self = Self {
            angle: self.angle,
            ..Self::default()
        };
    }

    /// UV coordinate under a point of the view, in pixels from the top left.
    pub fn to_uv(&self, pixel: Vec2, viewport: Vec2) -> Vec2 {
        let offset = (pixel - viewport * 0.5) * self.uv_per_pixel(viewport);
        self.center + Vec2::from_angle(-self.angle).rotate(offset)
    }

    /// Point of the view, in pixels from the top left, that shows `uv`.
    pub fn to_pixel(&self, uv: Vec2, viewport: Vec2) -> Vec2 {
        Vec2::from_angle(self.angle).rotate(uv - self.center) / self.uv_per_pixel(viewport)
            + viewport * 0.5
    }

    /// UV units per pixel.
    pub fn uv_per_pixel(&self, viewport: Vec2) -> f32 {
        self.height / viewport.y.max(1.0)
    }

    /// Moves the view by a mouse delta in pixels.
    pub fn pan(&mut self, delta: Vec2, viewport: Vec2) {
        self.center -= Vec2::from_angle(-self.angle).rotate(delta) * self.uv_per_pixel(viewport);
    }

    /// Zooms by scroll steps (positive zooms in), keeping the UV under `pixel` in place.
    pub fn zoom(&mut self, steps: f32, pixel: Vec2, viewport: Vec2) {
        let anchor = self.to_uv(pixel, viewport);
        self.height = (self.height * 0.9f32.powf(steps)).clamp(1e-3, 100.0);
        self.center += anchor - self.to_uv(pixel, viewport);
    }

    /// Rotates the view to `angle` around the UV under `pixel`.
    pub fn rotate_to(&mut self, angle: f32, pixel: Vec2, viewport: Vec2) {
        let anchor = self.to_uv(pixel, viewport);
        self.angle = angle;
        self.center += anchor - self.to_uv(pixel, viewport);
    }

    fn uniforms(&self, viewport: Vec2) -> ([f32; 4], [f32; 4]) {
        let scale_y = 2.0 / self.height;
        let scale_x = scale_y * viewport.y.max(1.0) / viewport.x.max(1.0);
        let (sin, cos) = self.angle.sin_cos();
        (
            [self.center.x, self.center.y, scale_x, scale_y],
            [cos, sin, 0.0, 0.0],
        )
    }
}

/// Must match `UvUniforms` in uv_view.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct UvUniforms {
    transform: [f32; 4],
    rotation: [f32; 4],
    info: [f32; 4],
    wire: [f32; 4],
}

pub struct UvView {
    square: wgpu::RenderPipeline,
    islands: wgpu::RenderPipeline,
    wire: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
    /// Triangle edges as line indices; triangle `t` has entries `6t..6t+6`.
    lines: Option<wgpu::Buffer>,
    targets: Option<Targets>,
}

impl UvView {
    pub fn new(device: &wgpu::Device, renderer: &ViewportRenderer) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("uv view"),
            source: wgpu::ShaderSource::Wgsl(include_str!("uv_view.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("uv view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("uv view"),
            bind_group_layouts: &[Some(&layout), Some(renderer.set_layout())],
            immediate_size: 0,
        });
        let vertex_layout = [Some(Vertex::layout())];
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        mesh: bool,
                        topology: wgpu::PrimitiveTopology,
                        blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: if mesh { &vertex_layout } else { &[] },
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let triangles = wgpu::PrimitiveTopology::TriangleList;
        let square = pipeline(
            "uv square",
            "vs_square",
            "fs_square",
            false,
            triangles,
            None,
        );
        let islands = pipeline("uv islands", "vs_mesh", "fs_islands", true, triangles, None);
        let wire = pipeline(
            "uv wireframe",
            "vs_mesh",
            "fs_wire",
            true,
            wgpu::PrimitiveTopology::LineList,
            Some(wgpu::BlendState::ALPHA_BLENDING),
        );
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uv view uniforms"),
            size: size_of::<UvUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uv view"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        Self {
            square,
            islands,
            wire,
            uniforms,
            group,
            lines: None,
            targets: None,
        }
    }

    /// Prepares the wireframe of a new model.
    pub fn set_model(&mut self, device: &wgpu::Device, model: &Model) {
        let lines: Vec<u32> = model
            .mesh
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[a, b, c]| [a, b, b, c, c, a])
            .collect();
        self.lines = (!lines.is_empty()).then(|| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("uv wireframe"),
                contents: bytemuck::cast_slice(&lines),
                usage: wgpu::BufferUsages::INDEX,
            })
        });
    }

    /// Recreates the render targets if the size changed. Returns true when the output
    /// view was replaced and must be re-registered with the UI.
    pub fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        Targets::ensure(&mut self.targets, device, size)
    }

    /// The color output for display in egui (sRGB-encoded values, read without decoding).
    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.display_view)
    }

    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t.resolve)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        renderer: &ViewportRenderer,
        model: &Model,
        set: usize,
        camera: &UvCamera,
        display: DisplayMode,
        wireframe: bool,
    ) {
        let Some(targets) = &self.targets else { return };
        let viewport = Vec2::new(targets.size[0] as f32, targets.size[1] as f32);
        let wire = [0.55, 0.6, 0.7].map(srgb_to_linear);
        let (transform, rotation) = camera.uniforms(viewport);
        let uniforms = UvUniforms {
            transform,
            rotation,
            info: [display.code(), if wireframe { 0.35 } else { 0.0 }, 0.0, 0.0],
            wire: [wire[0], wire[1], wire[2], 1.0],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let background = srgb_to_linear(0.09) as f64;
            let mut pass = targets.begin_pass(
                &mut encoder,
                wgpu::Color {
                    r: background,
                    g: background,
                    b: background,
                    a: 1.0,
                },
            );
            if let (Some(mesh), Some(set_group)) = (renderer.mesh(), renderer.set_bind_group(set)) {
                pass.set_bind_group(0, &self.group, &[]);
                pass.set_bind_group(1, set_group, &[]);
                pass.set_pipeline(&self.square);
                pass.draw(0..6, 0..1);

                let parts = model
                    .parts
                    .iter()
                    .filter(|p| p.texture_set == set && model.objects[p.object].visible);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_pipeline(&self.islands);
                for part in parts.clone() {
                    pass.draw_indexed(part.indices.clone(), 0, 0..1);
                }
                if wireframe && let Some(lines) = &self.lines {
                    pass.set_index_buffer(lines.slice(..), wgpu::IndexFormat::Uint32);
                    pass.set_pipeline(&self.wire);
                    for part in parts {
                        pass.draw_indexed(part.indices.start * 2..part.indices.end * 2, 0, 0..1);
                    }
                }
            }
        }
        queue.submit([encoder.finish()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_camera_round_trips_and_zooms_around_the_cursor() {
        let mut camera = UvCamera::default();
        let viewport = Vec2::new(800.0, 600.0);
        let pixel = Vec2::new(100.0, 450.0);
        let uv = camera.to_uv(pixel, viewport);
        assert!(camera.to_pixel(uv, viewport).distance(pixel) < 1e-3);
        assert_eq!(camera.to_uv(viewport * 0.5, viewport), Vec2::splat(0.5));

        camera.zoom(3.0, pixel, viewport);
        assert!(camera.height < 1.1);
        assert!(camera.to_uv(pixel, viewport).distance(uv) < 1e-5);

        camera.pan(Vec2::new(10.0, 0.0), viewport);
        assert!(
            camera
                .to_uv(pixel + Vec2::new(10.0, 0.0), viewport)
                .distance(uv)
                < 1e-5
        );

        // Rotation keeps the point under the cursor and round-trips.
        let pixel = pixel + Vec2::new(10.0, 0.0);
        camera.rotate_to(0.7, pixel, viewport);
        assert!(camera.to_uv(pixel, viewport).distance(uv) < 1e-5);
        let other = Vec2::new(500.0, 20.0);
        assert!(
            camera
                .to_pixel(camera.to_uv(other, viewport), viewport)
                .distance(other)
                < 1e-3
        );
        camera.pan(Vec2::new(0.0, 25.0), viewport);
        assert!(
            camera
                .to_uv(pixel + Vec2::new(0.0, 25.0), viewport)
                .distance(uv)
                < 1e-5
        );
    }

    #[test]
    fn uniforms_match_shader() {
        assert_eq!(size_of::<UvUniforms>(), 64);
    }
}
