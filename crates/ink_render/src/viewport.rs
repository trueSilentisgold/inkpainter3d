use crate::camera::OrbitCamera;
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::Vec3;
use ink_core::Mesh;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLE_COUNT: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadingMode {
    Pbr,
    Toon,
}

/// User-tweakable look of the viewport. Colors are sRGB.
#[derive(Clone, Debug)]
pub struct ViewportSettings {
    pub shading: ShadingMode,
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub exposure: f32,
    pub light_yaw: f32,
    pub light_pitch: f32,
    pub light_intensity: f32,
    pub toon_bands: u32,
    pub outline: bool,
    pub outline_color: [f32; 3],
    pub outline_width: f32,
}

impl Default for ViewportSettings {
    fn default() -> Self {
        Self {
            shading: ShadingMode::Pbr,
            base_color: [0.8, 0.8, 0.8],
            roughness: 0.5,
            metallic: 0.0,
            exposure: 1.0,
            light_yaw: 0.8,
            light_pitch: 0.7,
            light_intensity: 3.0,
            toon_bands: 3,
            outline: true,
            outline_color: [0.05, 0.05, 0.07],
            outline_width: 2.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

/// Must match `Uniforms` in viewport.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_dir: [f32; 4],
    base_color: [f32; 4],
    material: [f32; 4],
    ink: [f32; 4],
    viewport: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    resolve: wgpu::TextureView,
    depth: wgpu::TextureView,
}

/// Renders the mesh into an offscreen texture that the UI displays.
pub struct ViewportRenderer {
    mesh_pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    background_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    mesh: Option<GpuMesh>,
    targets: Option<Targets>,
}

impl ViewportRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("viewport.wgsl").into()),
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport bind group layout"),
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
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
        };

        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        buffers: &[Option<wgpu::VertexBufferLayout>],
                        cull_mode: Option<wgpu::Face>,
                        depth_write: bool,
                        depth_compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(depth_compare),
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
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let mesh_buffers = [Some(vertex_layout.clone())];
        // Meshes are drawn two-sided: imported models often have open or inconsistently wound surfaces.
        let mesh_pipeline = pipeline(
            "mesh",
            "vs_mesh",
            "fs_mesh",
            &mesh_buffers,
            None,
            true,
            wgpu::CompareFunction::Less,
        );
        let outline_pipeline = pipeline(
            "outline",
            "vs_outline",
            "fs_outline",
            &mesh_buffers,
            Some(wgpu::Face::Front),
            true,
            wgpu::CompareFunction::Less,
        );
        let background_pipeline = pipeline(
            "background",
            "vs_background",
            "fs_background",
            &[],
            None,
            false,
            wgpu::CompareFunction::Always,
        );

        Self {
            mesh_pipeline,
            outline_pipeline,
            background_pipeline,
            uniform_buffer,
            bind_group,
            mesh: None,
            targets: None,
        }
    }

    pub fn set_mesh(&mut self, device: &wgpu::Device, mesh: &Mesh) {
        let vertices: Vec<Vertex> = (0..mesh.vertex_count())
            .map(|i| Vertex {
                position: mesh.positions[i].into(),
                normal: mesh.normals[i].into(),
                uv: mesh.uvs[i].into(),
            })
            .collect();
        self.mesh = Some(GpuMesh {
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("mesh indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            }),
            index_count: mesh.indices.len() as u32,
        });
    }

    /// Recreates the render targets if the size changed. Returns true when the
    /// output texture view was replaced and must be re-registered with the UI.
    pub fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        let texture = |label: &str, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        self.targets = Some(Targets {
            size,
            msaa: texture(
                "viewport msaa",
                COLOR_FORMAT,
                SAMPLE_COUNT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
            resolve: texture(
                "viewport color",
                COLOR_FORMAT,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            depth: texture(
                "viewport depth",
                DEPTH_FORMAT,
                SAMPLE_COUNT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
        });
        true
    }

    /// The resolved color output. Only valid after [`Self::resize`] was called once.
    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.resolve)
    }

    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &OrbitCamera,
        settings: &ViewportSettings,
    ) {
        let Some(targets) = &self.targets else { return };
        let [w, h] = targets.size;
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&uniforms(camera, settings, [w, h])),
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewport"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa,
                    depth_slice: None,
                    resolve_target: Some(&targets.resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_pipeline(&self.background_pipeline);
            pass.draw(0..3, 0..1);

            if let Some(mesh) = &self.mesh {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.set_pipeline(&self.mesh_pipeline);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                if settings.shading == ShadingMode::Toon
                    && settings.outline
                    && settings.outline_width > 0.0
                {
                    pass.set_pipeline(&self.outline_pipeline);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
        }
        queue.submit([encoder.finish()]);
    }
}

fn uniforms(camera: &OrbitCamera, s: &ViewportSettings, size: [u32; 2]) -> Uniforms {
    let aspect = size[0] as f32 / size[1] as f32;
    let view_proj = camera.projection(aspect) * camera.view();
    let light_dir = Vec3::new(
        s.light_pitch.cos() * s.light_yaw.sin(),
        s.light_pitch.sin(),
        s.light_pitch.cos() * s.light_yaw.cos(),
    );
    let lin = |c: [f32; 3]| {
        [
            srgb_to_linear(c[0]),
            srgb_to_linear(c[1]),
            srgb_to_linear(c[2]),
        ]
    };
    let [r, g, b] = lin(s.base_color);
    let [or, og, ob] = lin(s.outline_color);
    let mode = match s.shading {
        ShadingMode::Pbr => 0.0,
        ShadingMode::Toon => 1.0,
    };
    Uniforms {
        view_proj: view_proj.to_cols_array_2d(),
        camera_pos: camera.eye().extend(1.0).into(),
        light_dir: light_dir.extend(s.light_intensity).into(),
        base_color: [r, g, b, 1.0],
        material: [s.roughness, s.metallic, s.exposure, s.toon_bands as f32],
        ink: [or, og, ob, s.outline_width],
        viewport: [size[0] as f32, size[1] as f32, mode, 0.0],
        bg_top: [0.09, 0.095, 0.11, 1.0],
        bg_bottom: [0.025, 0.025, 0.03, 1.0],
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(include_str!("viewport.wgsl"))
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(include_str!("viewport.wgsl"))));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(include_str!("viewport.wgsl"))));
    }

    #[test]
    fn uniform_layout_matches_shader() {
        // 1 mat4 + 8 vec4, no implicit padding.
        assert_eq!(size_of::<Uniforms>(), 64 + 8 * 16);
    }
}
