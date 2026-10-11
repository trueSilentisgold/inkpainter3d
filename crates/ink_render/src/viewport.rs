use crate::camera::OrbitCamera;
use crate::environment::{self, Environment, IblBaker, SPECULAR_MIPS};
use crate::paint::{GpuPaint, PaintRenderer};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu::{self, util::DeviceExt};
use glam::Quat;
use ink_core::{AlphaMode, ChannelValues, Model, TextureSet};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub(crate) const SAMPLE_COUNT: u32 = 4;
const DISPLAY_VIEW_FORMATS: [wgpu::TextureFormat; 1] = [wgpu::TextureFormat::Rgba8Unorm];

/// Channel textures of a texture set as the viewport samples them.
/// Base color + opacity, normal, roughness / metallic / AO, emissive.
pub(crate) const CHANNEL_FORMATS: [wgpu::TextureFormat; 4] = [
    wgpu::TextureFormat::Rgba8UnormSrgb,
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba8UnormSrgb,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadingMode {
    Pbr,
    Toon,
}

/// What the viewport shows, like Painter's viewport dropdown (M, C, B keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayMode {
    Material,
    /// Every texture set in its own color.
    MaterialId,
    BaseColor,
    Roughness,
    Metallic,
    Normal,
    Emissive,
    Opacity,
    AmbientOcclusion,
}

impl DisplayMode {
    pub const ALL: [Self; 9] = [
        Self::Material,
        Self::MaterialId,
        Self::BaseColor,
        Self::Roughness,
        Self::Metallic,
        Self::Normal,
        Self::Emissive,
        Self::Opacity,
        Self::AmbientOcclusion,
    ];
    const CHANNELS: [Self; 7] = [
        Self::BaseColor,
        Self::Roughness,
        Self::Metallic,
        Self::Normal,
        Self::Emissive,
        Self::Opacity,
        Self::AmbientOcclusion,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Material => "Material",
            Self::MaterialId => "Material ID",
            Self::BaseColor => "Base color",
            Self::Roughness => "Roughness",
            Self::Metallic => "Metallic",
            Self::Normal => "Normal",
            Self::Emissive => "Emissive",
            Self::Opacity => "Opacity",
            Self::AmbientOcclusion => "Ambient occlusion",
        }
    }

    /// Cycles through the single channels (Painter: C and Shift+C).
    pub fn cycle_channel(self, forward: bool) -> Self {
        let n = Self::CHANNELS.len();
        let next = match Self::CHANNELS.iter().position(|&c| c == self) {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        Self::CHANNELS[next]
    }

    /// Must match the DISPLAY_* constants in viewport.wgsl and uv_view.wgsl.
    pub(crate) fn code(self) -> f32 {
        Self::ALL.iter().position(|&m| m == self).unwrap_or(0) as f32
    }
}

/// User-tweakable look of the viewport (Painter's display settings). Colors are sRGB.
#[derive(Clone, Debug)]
pub struct ViewportSettings {
    pub shading: ShadingMode,
    pub display: DisplayMode,
    /// Rotation of the environment around the vertical axis in radians.
    pub environment_rotation: f32,
    /// Exposure in stops (EV).
    pub exposure: f32,
    /// How much of the environment shows in the background, 0 to 1.
    pub background_opacity: f32,
    /// Blur of the environment in the background, 0 to 1.
    pub background_blur: f32,
    pub toon_bands: u32,
    pub outline: bool,
    pub outline_color: [f32; 3],
    pub outline_width: f32,
}

impl Default for ViewportSettings {
    fn default() -> Self {
        Self {
            shading: ShadingMode::Pbr,
            display: DisplayMode::Material,
            environment_rotation: 0.0,
            exposure: 0.0,
            background_opacity: 0.0,
            background_blur: 0.5,
            toon_bands: 3,
            outline: true,
            outline_color: [0.05, 0.05, 0.07],
            outline_width: 2.0,
        }
    }
}

/// Parts to emphasize, e.g. while hovering them in the texture set list.
/// A part is highlighted when it matches every field that is set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    pub texture_set: Option<usize>,
    pub object: Option<usize>,
}

impl Highlight {
    fn matches(&self, part: &ink_core::Part) -> bool {
        (self.texture_set.is_some() || self.object.is_some())
            && self.texture_set.is_none_or(|s| s == part.texture_set)
            && self.object.is_none_or(|o| o == part.object)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    tangent: [f32; 4],
}

/// Must match `Uniforms` in viewport.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    env: [f32; 4],
    env_info: [f32; 4],
    toon_light: [f32; 4],
    ink: [f32; 4],
    viewport: [f32; 4],
    highlight: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    sh: [[f32; 4]; 9],
}

/// Must match `TextureSetUniforms` in viewport.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TextureSetUniforms {
    id_color: [f32; 4],
}

impl Vertex {
    const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Float32x4
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

pub(crate) struct GpuMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
}

struct GpuTextureSet {
    resolution: u32,
    fill: ChannelValues,
    alpha_mode: AlphaMode,
    /// Channel textures with mips, in `CHANNEL_FORMATS` order.
    textures: Vec<wgpu::Texture>,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    /// Created with the first stroke.
    paint: Option<GpuPaint>,
}

/// Offscreen color target shown in egui, multisampled, with depth.
pub(crate) struct Targets {
    pub size: [u32; 2],
    pub msaa: wgpu::TextureView,
    pub resolve: wgpu::Texture,
    pub resolve_view: wgpu::TextureView,
    /// Same pixels without sRGB decoding: egui expects gamma-encoded texture values.
    pub display_view: wgpu::TextureView,
    pub depth: wgpu::TextureView,
}

impl Targets {
    pub fn new(device: &wgpu::Device, size: [u32; 2]) -> Self {
        let size = [size[0].max(1), size[1].max(1)];
        let texture = |label: &str, format: wgpu::TextureFormat, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
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
                // Lets the color output also be viewed without sRGB decoding.
                view_formats: if format == COLOR_FORMAT {
                    &DISPLAY_VIEW_FORMATS
                } else {
                    &[]
                },
            })
        };
        let resolve = texture(
            "view color",
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        Self {
            size,
            msaa: texture(
                "view msaa",
                COLOR_FORMAT,
                SAMPLE_COUNT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default()),
            resolve_view: resolve.create_view(&Default::default()),
            display_view: resolve.create_view(&wgpu::TextureViewDescriptor {
                format: Some(DISPLAY_VIEW_FORMATS[0]),
                ..Default::default()
            }),
            resolve,
            depth: texture(
                "view depth",
                DEPTH_FORMAT,
                SAMPLE_COUNT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default()),
        }
    }

    /// Replaces the targets if the size changed. Returns true when the display view was
    /// replaced and must be re-registered with the UI.
    pub fn ensure(targets: &mut Option<Self>, device: &wgpu::Device, size: [u32; 2]) -> bool {
        let size = [size[0].max(1), size[1].max(1)];
        if targets.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        *targets = Some(Self::new(device, size));
        true
    }

    /// A pass that clears the color to `clear` and the depth to 1, resolving into
    /// `resolve`.
    pub fn begin_pass<'a>(
        &self,
        encoder: &'a mut wgpu::CommandEncoder,
        clear: wgpu::Color,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("view pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.msaa,
                depth_slice: None,
                resolve_target: Some(&self.resolve_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        })
    }
}

/// Renders the model into an offscreen texture that the UI displays.
pub struct ViewportRenderer {
    mesh_pipeline: wgpu::RenderPipeline,
    blend_pipeline: wgpu::RenderPipeline,
    outline_pipeline: wgpu::RenderPipeline,
    background_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    global_layout: wgpu::BindGroupLayout,
    global_bind_group: wgpu::BindGroup,
    set_layout: wgpu::BindGroupLayout,
    material_sampler: wgpu::Sampler,
    panorama_sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    brdf_lut: wgpu::TextureView,
    baker: IblBaker,
    environment: Environment,
    mesh: Option<GpuMesh>,
    sets: Vec<GpuTextureSet>,
    targets: Option<Targets>,
    paint: PaintRenderer,
}

impl ViewportRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
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
        let texture_entry = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let global_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport global layout"),
            entries: &[
                uniform_entry(0),
                texture_entry(1, wgpu::TextureViewDimension::D2),
                texture_entry(2, wgpu::TextureViewDimension::Cube),
                texture_entry(3, wgpu::TextureViewDimension::D2),
                sampler_entry(4),
                sampler_entry(5),
            ],
        });
        let set_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture set layout"),
            entries: &[
                uniform_entry(0),
                texture_entry(1, wgpu::TextureViewDimension::D2),
                texture_entry(2, wgpu::TextureViewDimension::D2),
                texture_entry(3, wgpu::TextureViewDimension::D2),
                texture_entry(4, wgpu::TextureViewDimension::D2),
                sampler_entry(5),
            ],
        });
        let global_only = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport global pipeline layout"),
            bind_group_layouts: &[Some(&global_layout)],
            immediate_size: 0,
        });
        let with_set = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport mesh pipeline layout"),
            bind_group_layouts: &[Some(&global_layout), Some(&set_layout)],
            immediate_size: 0,
        });

        let vertex_layout = Vertex::layout();

        struct PipelineDesc<'a> {
            label: &'a str,
            layout: &'a wgpu::PipelineLayout,
            vs: &'a str,
            fs: &'a str,
            buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
            cull_mode: Option<wgpu::Face>,
            depth_write: bool,
            depth_compare: wgpu::CompareFunction,
            blend: Option<wgpu::BlendState>,
        }
        let pipeline = |d: PipelineDesc| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(d.label),
                layout: Some(d.layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(d.vs),
                    compilation_options: Default::default(),
                    buffers: d.buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: d.cull_mode,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(d.depth_write),
                    depth_compare: Some(d.depth_compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: SAMPLE_COUNT,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(d.fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR_FORMAT,
                        blend: d.blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let mesh_buffers = [Some(vertex_layout)];
        // Meshes are drawn two-sided: imported models often have open or inconsistently wound surfaces.
        let mesh_pipeline = pipeline(PipelineDesc {
            label: "mesh",
            layout: &with_set,
            vs: "vs_mesh",
            fs: "fs_mesh",
            buffers: &mesh_buffers,
            cull_mode: None,
            depth_write: true,
            depth_compare: wgpu::CompareFunction::Less,
            blend: None,
        });
        let blend_pipeline = pipeline(PipelineDesc {
            label: "mesh alpha blend",
            layout: &with_set,
            vs: "vs_mesh",
            fs: "fs_mesh",
            buffers: &mesh_buffers,
            cull_mode: None,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Less,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        });
        let outline_pipeline = pipeline(PipelineDesc {
            label: "outline",
            layout: &global_only,
            vs: "vs_outline",
            fs: "fs_outline",
            buffers: &mesh_buffers,
            cull_mode: Some(wgpu::Face::Front),
            depth_write: true,
            depth_compare: wgpu::CompareFunction::Less,
            blend: None,
        });
        let background_pipeline = pipeline(PipelineDesc {
            label: "background",
            layout: &global_only,
            vs: "vs_background",
            fs: "fs_background",
            buffers: &[],
            cull_mode: None,
            depth_write: false,
            depth_compare: wgpu::CompareFunction::Always,
            blend: None,
        });

        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("material sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let clamp_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("clamp sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let panorama_sampler = environment::panorama_sampler(device);
        let brdf_lut = environment::create_brdf_lut(device, queue);
        let baker = IblBaker::new(device);
        // Neutral grey until the first real environment is set.
        let environment = baker.bake(device, queue, 2, 1, &[[0.5, 0.5, 0.5, 1.0]; 2]);
        let global_bind_group = create_global_bind_group(
            device,
            &global_layout,
            &uniform_buffer,
            &environment,
            &brdf_lut,
            &panorama_sampler,
            &clamp_sampler,
        );

        let paint = PaintRenderer::new(device, queue, &Vertex::layout());
        Self {
            mesh_pipeline,
            blend_pipeline,
            outline_pipeline,
            background_pipeline,
            uniform_buffer,
            global_layout,
            global_bind_group,
            set_layout,
            material_sampler,
            panorama_sampler,
            clamp_sampler,
            brdf_lut,
            baker,
            environment,
            mesh: None,
            sets: Vec::new(),
            targets: None,
            paint,
        }
    }

    /// Lights the scene with an equirectangular HDR panorama (linear RGBA, top row first).
    pub fn set_environment(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        pixels: &[[f32; 4]],
    ) {
        self.environment = self.baker.bake(device, queue, width, height, pixels);
        self.global_bind_group = create_global_bind_group(
            device,
            &self.global_layout,
            &self.uniform_buffer,
            &self.environment,
            &self.brdf_lut,
            &self.panorama_sampler,
            &self.clamp_sampler,
        );
    }

    /// Uploads the geometry and creates the channel textures of every texture set.
    pub fn set_model(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, model: &Model) {
        let mesh = &model.mesh;
        let vertices: Vec<Vertex> = (0..mesh.vertex_count())
            .map(|i| Vertex {
                position: mesh.positions[i].into(),
                normal: mesh.normals[i].into(),
                uv: mesh.uvs[i].into(),
                tangent: mesh.tangents[i].into(),
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
        });
        self.sets.clear();
        self.paint.reset();
        self.sync_texture_sets(device, queue, model);
    }

    /// Brings the GPU textures in line with the texture sets after their settings or
    /// strokes changed. Cheap when nothing changed, so it can run every frame.
    pub fn sync_texture_sets(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, model: &Model) {
        self.sets.truncate(model.texture_sets.len());
        for (index, set) in model.texture_sets.iter().enumerate() {
            let uniforms = TextureSetUniforms {
                id_color: id_color(index, set.alpha_mode),
            };
            let mut fill_changed = false;
            if let Some(gpu) = self.sets.get_mut(index)
                && gpu.resolution == set.resolution
            {
                if gpu.fill != set.fill {
                    gpu.fill = set.fill;
                    fill_changed = true;
                }
                if gpu.alpha_mode != set.alpha_mode {
                    gpu.alpha_mode = set.alpha_mode;
                    queue.write_buffer(&gpu.uniforms, 0, bytemuck::bytes_of(&uniforms));
                }
            } else {
                let gpu = self.create_texture_set(device, queue, set, &uniforms);
                match self.sets.get_mut(index) {
                    Some(slot) => *slot = gpu,
                    None => self.sets.push(gpu),
                }
            }
            let gpu = &mut self.sets[index];
            let painted = self.mesh.as_ref().is_some_and(|mesh| {
                self.paint.sync(
                    device,
                    queue,
                    model,
                    mesh,
                    index,
                    &mut gpu.paint,
                    &gpu.textures,
                    fill_changed,
                )
            });
            if fill_changed && !painted {
                fill_channels(device, queue, &gpu.textures, &set.fill);
            }
        }
    }

    fn create_texture_set(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        set: &TextureSet,
        uniforms: &TextureSetUniforms,
    ) -> GpuTextureSet {
        let resolution = set.resolution.max(1);
        let textures: Vec<wgpu::Texture> = CHANNEL_FORMATS
            .iter()
            .map(|&format| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(&format!("{} channels", set.name)),
                    size: wgpu::Extent3d {
                        width: resolution,
                        height: resolution,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: resolution.ilog2() + 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            })
            .collect();
        fill_channels(device, queue, &textures, &set.fill);
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("texture set uniforms"),
            contents: bytemuck::bytes_of(uniforms),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let views: Vec<wgpu::TextureView> = textures
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }];
        for (i, view) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32 + 1,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 5,
            resource: wgpu::BindingResource::Sampler(&self.material_sampler),
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("texture set bind group"),
            layout: &self.set_layout,
            entries: &entries,
        });
        GpuTextureSet {
            resolution: set.resolution,
            fill: set.fill,
            alpha_mode: set.alpha_mode,
            textures,
            uniforms: uniform_buffer,
            bind_group,
            paint: None,
        }
    }

    /// Recreates the render targets if the size changed. Returns true when the
    /// output texture view was replaced and must be re-registered with the UI.
    pub fn resize(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        Targets::ensure(&mut self.targets, device, size)
    }

    /// The resolved color output for display in egui (sRGB-encoded values, read without
    /// decoding). Only valid after [`Self::resize`] was called once.
    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.display_view)
    }

    pub(crate) fn mesh(&self) -> Option<&GpuMesh> {
        self.mesh.as_ref()
    }

    pub(crate) fn set_layout(&self) -> &wgpu::BindGroupLayout {
        &self.set_layout
    }

    /// The texture set's channel textures as `@group(1)` of viewport.wgsl, once the GPU
    /// sets match the model.
    pub(crate) fn set_bind_group(&self, set: usize) -> Option<&wgpu::BindGroup> {
        self.sets.get(set).map(|s| &s.bind_group)
    }

    /// The resolved color output texture, e.g. for screenshots.
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t.resolve)
    }

    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera: &OrbitCamera,
        settings: &ViewportSettings,
        model: &Model,
        highlight: Highlight,
    ) {
        let Some(targets) = &self.targets else { return };
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&self.uniforms(camera, settings, targets.size)),
        );

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("viewport"),
        });
        {
            let mut pass = targets.begin_pass(&mut encoder, wgpu::Color::BLACK);
            pass.set_bind_group(0, &self.global_bind_group, &[]);
            pass.set_pipeline(&self.background_pipeline);
            pass.draw(0..3, 0..1);

            if let Some(mesh) = &self.mesh
                && self.sets.len() == model.texture_sets.len()
            {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                let visible = |blend: bool| {
                    model.parts.iter().filter(move |p| {
                        model.is_part_visible(p)
                            && (model.texture_sets[p.texture_set].alpha_mode == AlphaMode::Blend)
                                == blend
                    })
                };

                pass.set_pipeline(&self.mesh_pipeline);
                for part in visible(false) {
                    pass.set_bind_group(1, &self.sets[part.texture_set].bind_group, &[]);
                    let h = highlight.matches(part) as u32;
                    pass.draw_indexed(part.indices.clone(), 0, h..h + 1);
                }
                if settings.shading == ShadingMode::Toon
                    && settings.outline
                    && settings.outline_width > 0.0
                    && settings.display == DisplayMode::Material
                {
                    pass.set_pipeline(&self.outline_pipeline);
                    for part in visible(false) {
                        pass.draw_indexed(part.indices.clone(), 0, 0..1);
                    }
                }
                pass.set_pipeline(&self.blend_pipeline);
                for part in visible(true) {
                    pass.set_bind_group(1, &self.sets[part.texture_set].bind_group, &[]);
                    let h = highlight.matches(part) as u32;
                    pass.draw_indexed(part.indices.clone(), 0, h..h + 1);
                }
            }
        }
        queue.submit([encoder.finish()]);
    }

    fn uniforms(&self, camera: &OrbitCamera, s: &ViewportSettings, size: [u32; 2]) -> Uniforms {
        let aspect = size[0] as f32 / size[1] as f32;
        let view_proj = camera.projection(aspect) * camera.view();
        let lin = |c: [f32; 3]| c.map(srgb_to_linear);
        let [or, og, ob] = lin(s.outline_color);
        let shading = match s.shading {
            ShadingMode::Pbr => 0.0,
            ShadingMode::Toon => 1.0,
        };
        // The shader rotates world directions by -rotation into the environment.
        let toon_light =
            Quat::from_rotation_y(s.environment_rotation) * self.environment.dominant_direction;
        Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            camera_pos: camera.eye().extend(1.0).into(),
            env: [
                s.environment_rotation,
                2f32.powf(s.exposure),
                s.background_opacity.clamp(0.0, 1.0),
                s.background_blur.clamp(0.0, 1.0),
            ],
            env_info: [(SPECULAR_MIPS - 1) as f32, 0.0, 0.0, 0.0],
            toon_light: toon_light.extend(s.toon_bands as f32).into(),
            ink: [or, og, ob, s.outline_width],
            viewport: [size[0] as f32, size[1] as f32, shading, s.display.code()],
            highlight: [1.0, 0.45, 0.05, 0.45],
            bg_top: [0.035, 0.035, 0.04, 1.0],
            bg_bottom: [0.012, 0.012, 0.014, 1.0],
            sh: self.environment.irradiance_sh,
        }
    }
}

fn create_global_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    environment: &Environment,
    brdf_lut: &wgpu::TextureView,
    panorama_sampler: &wgpu::Sampler,
    clamp_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("viewport global bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&environment.panorama),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&environment.specular),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(brdf_lut),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(panorama_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(clamp_sampler),
            },
        ],
    })
}

/// Clears every mip level of the channel textures to the uniform values.
fn fill_channels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    textures: &[wgpu::Texture],
    fill: &ChannelValues,
) {
    let lin = |c: [f32; 3]| c.map(|v| srgb_to_linear(v) as f64);
    let [r, g, b] = lin(fill.base_color);
    let [er, eg, eb] = lin(fill.emissive);
    // Clear colors are linear; sRGB targets encode them.
    let colors = [
        wgpu::Color {
            r,
            g,
            b,
            a: fill.opacity as f64,
        },
        wgpu::Color {
            r: 0.5,
            g: 0.5,
            b: 1.0,
            a: 1.0,
        },
        wgpu::Color {
            r: fill.roughness as f64,
            g: fill.metallic as f64,
            b: 1.0,
            a: 1.0,
        },
        wgpu::Color {
            r: er,
            g: eg,
            b: eb,
            a: 1.0,
        },
    ];
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("fill channels"),
    });
    for (texture, color) in textures.iter().zip(colors) {
        for mip in 0..texture.mip_level_count() {
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            });
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fill channel"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
    }
    queue.submit([encoder.finish()]);
}

/// Distinct, stable color per texture set (golden angle hues); the alpha channel carries
/// the alpha mode for the shader.
fn id_color(index: usize, alpha_mode: AlphaMode) -> [f32; 4] {
    let [r, g, b] = texture_set_color(index).map(srgb_to_linear);
    let mode = match alpha_mode {
        AlphaMode::Opaque => 0.0,
        AlphaMode::Test => 1.0,
        AlphaMode::Blend => 2.0,
    };
    [r, g, b, mode]
}

/// Material ID color of a texture set in sRGB, for showing it in the UI.
pub fn texture_set_color(index: usize) -> [f32; 3] {
    hsv_to_rgb((index as f32 * 0.381_966).fract(), 0.65, 0.95)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(name: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
    }

    #[test]
    fn shaders_are_valid_wgsl() {
        validate("viewport.wgsl", include_str!("viewport.wgsl"));
        validate("paint_dabs.wgsl", include_str!("paint_dabs.wgsl"));
        validate("paint_seeds.wgsl", include_str!("paint_seeds.wgsl"));
        validate("paint_depth.wgsl", include_str!("paint_depth.wgsl"));
        validate("paint_apply.wgsl", include_str!("paint_apply.wgsl"));
        validate("paint_composite.wgsl", include_str!("paint_composite.wgsl"));
        validate("mipmap.wgsl", include_str!("mipmap.wgsl"));
        validate("uv_view.wgsl", include_str!("uv_view.wgsl"));
        validate(
            "prefilter.wgsl",
            concat!(
                include_str!("ibl_common.wgsl"),
                include_str!("prefilter.wgsl")
            ),
        );
        validate(
            "brdf_lut.wgsl",
            concat!(
                include_str!("ibl_common.wgsl"),
                include_str!("brdf_lut.wgsl")
            ),
        );
    }

    #[test]
    fn uniform_layout_matches_shader() {
        // 2 mat4 + 9 vec4 + 9 vec4 of SH, no implicit padding.
        assert_eq!(size_of::<Uniforms>(), 2 * 64 + 9 * 16 + 9 * 16);
    }

    #[test]
    fn display_modes_cycle_through_channels() {
        let mut mode = DisplayMode::Material;
        let mut seen = Vec::new();
        for _ in 0..7 {
            mode = mode.cycle_channel(true);
            seen.push(mode);
        }
        assert_eq!(seen[0], DisplayMode::BaseColor);
        assert_eq!(mode.cycle_channel(true), DisplayMode::BaseColor);
        assert_eq!(
            DisplayMode::BaseColor.cycle_channel(false),
            DisplayMode::AmbientOcclusion
        );
        assert_eq!(DisplayMode::Material.code(), 0.0);
        assert_eq!(DisplayMode::AmbientOcclusion.code(), 8.0);
    }

    #[test]
    fn id_colors_differ_between_neighbours() {
        let a = texture_set_color(0);
        let b = texture_set_color(1);
        let diff: f32 = a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum();
        assert!(diff > 0.3);
    }
}
