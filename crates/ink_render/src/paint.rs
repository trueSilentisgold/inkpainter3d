//! Brush strokes on the GPU. A texture set's strokes are rasterized into its paint layer:
//! each stroke's dabs accumulate in a coverage texture, which is then blended into the
//! layer's channel textures. The composite combines the fill values and the paint layer
//! into the channel textures the viewport samples, with padding past the UV islands and
//! mip maps.
//!
//! The renderer keeps track of which strokes the layer holds and only rasterizes what
//! changed: new dabs of the stroke being painted, new strokes, or, after undo, a copy of
//! the layer taken before the last stroke. Anything else repaints all strokes.

use crate::viewport::{GpuMesh, srgb_to_linear};
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use ink_core::{
    Alpha, Channel, ChannelValues, Dab, Model, Stroke, StrokeCamera, StrokeSpace, Tool,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Paint layer textures, one per [`Channel`] in [`Channel::ALL`] order. Premultiplied:
/// color channels hold value * coverage in rgb and the coverage in alpha; scalar channels
/// value * coverage in r and the coverage in g.
const LAYER_FORMATS: [wgpu::TextureFormat; 5] = [
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rg16Float,
    wgpu::TextureFormat::Rg16Float,
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rg16Float,
];
const COVERAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;
const SEED_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// Indices of the texture set channel textures the composite writes: base color and
/// opacity, roughness / metallic / AO, emissive. The normal texture stays flat for now.
pub(crate) const COMPOSITE_OUTPUTS: [usize; 3] = [0, 2, 3];
/// Texels around the painted area that are composited too, for the padding around seams.
const PADDING_MARGIN: u32 = 8;
const MAX_DABS_PER_DRAW: usize = 64;
/// Longer side of the depth maps used for occlusion, in pixels. Fixed, so strokes paint the
/// same whatever the window size.
const DEPTH_MAP_SIZE: u32 = 2048;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Depth maps kept for strokes painted from the same view.
const DEPTH_MAP_CACHE: usize = 4;
/// Uniform buffer offset alignment for the per-channel apply parameters.
const APPLY_STRIDE: u64 = 256;

/// Must match `Dab` in paint_dabs.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuDab {
    position: [f32; 4],
    normal: [f32; 4],
    up: [f32; 4],
    view: [f32; 4],
}

/// Must match `StrokeParams` in paint_dabs.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StrokeParams {
    shape: [f32; 4],
    sizes: [f32; 4],
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    forward: [f32; 4],
}

/// Must match `DepthCamera` in paint_depth.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DepthCamera {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    forward: [f32; 4],
}

/// A depth map of the model seen from a stroke's camera.
struct DepthMap {
    key: u64,
    view: wgpu::TextureView,
    last_used: u64,
}

/// Must match `ApplyParams` in paint_apply.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ApplyParams {
    value: [f32; 4],
    info: [f32; 4],
}

/// Must match `Fill` in paint_composite.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FillUniforms {
    base: [f32; 4],
    surface: [f32; 4],
    emissive: [f32; 4],
}

impl FillUniforms {
    fn new(fill: &ChannelValues) -> Self {
        let [r, g, b] = fill.base_color.map(srgb_to_linear);
        let [er, eg, eb] = fill.emissive.map(srgb_to_linear);
        Self {
            base: [r, g, b, fill.opacity],
            surface: [fill.roughness, fill.metallic, 0.0, 0.0],
            emissive: [er, eg, eb, 0.0],
        }
    }
}

/// A rectangle of texels, `min` inclusive, `max` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TexelRect {
    min: [u32; 2],
    max: [u32; 2],
}

impl TexelRect {
    const EMPTY: Self = Self {
        min: [u32::MAX; 2],
        max: [0; 2],
    };

    fn full(size: u32) -> Self {
        Self {
            min: [0; 2],
            max: [size; 2],
        }
    }

    fn is_empty(&self) -> bool {
        self.min[0] >= self.max[0] || self.min[1] >= self.max[1]
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    /// The texels covered by a UV-space box, grown by `margin` and clipped to the texture.
    fn from_uv(min: [f32; 2], max: [f32; 2], size: u32, margin: u32) -> Self {
        let s = size as f32;
        let lo = |v: f32| ((v * s).floor().max(0.0) as u32).saturating_sub(margin);
        let hi = |v: f32| ((v * s).ceil().max(0.0) as u32 + margin).min(size);
        let rect = Self {
            min: [lo(min[0]), lo(min[1])],
            max: [hi(max[0]), hi(max[1])],
        };
        if rect.is_empty() { Self::EMPTY } else { rect }
    }

    fn grow(self, margin: u32, size: u32) -> Self {
        if self.is_empty() {
            return self;
        }
        Self {
            min: self.min.map(|v| v.saturating_sub(margin)),
            max: self.max.map(|v| (v + margin).min(size)),
        }
    }

    /// The rectangle on mip `level`, rounded outwards.
    fn at_level(self, level: u32, size: u32) -> Self {
        let level_size = (size >> level).max(1);
        Self {
            min: self.min.map(|v| v >> level),
            max: self
                .max
                .map(|v| ((v + (1 << level) - 1) >> level).min(level_size)),
        }
    }

    fn scissor(&self, pass: &mut wgpu::RenderPass) {
        pass.set_scissor_rect(
            self.min[0],
            self.min[1],
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
        );
    }

    fn origin(&self) -> wgpu::Origin3d {
        wgpu::Origin3d {
            x: self.min[0],
            y: self.min[1],
            z: 0,
        }
    }

    fn extent(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.max[0] - self.min[0],
            height: self.max[1] - self.min[1],
            depth_or_array_layers: 1,
        }
    }
}

/// Paint state of one texture set on the GPU.
pub(crate) struct GpuPaint {
    resolution: u32,
    layer: Vec<wgpu::Texture>,
    layer_views: Vec<wgpu::TextureView>,
    coverage: wgpu::TextureView,
    fill_buffer: wgpu::Buffer,
    composite_group: wgpu::BindGroup,
    /// Strokes on the layer with their number of dabs, bottom to top.
    applied: Vec<(u64, usize)>,
    /// Only the painted area was composited; texels padded from far away are outdated.
    stale: bool,
}

/// The paint layer as it was before the topmost stroke, for continuing that stroke and
/// for undoing it without repainting everything.
struct Snapshot {
    set: usize,
    stroke: u64,
    resolution: u32,
    textures: Vec<wgpu::Texture>,
}

pub(crate) struct PaintRenderer {
    depth: wgpu::RenderPipeline,
    depth_layout: wgpu::BindGroupLayout,
    depth_maps: Vec<DepthMap>,
    depth_uses: u64,
    /// Bound when a stroke has no camera.
    no_depth: wgpu::TextureView,
    seed_init: wgpu::RenderPipeline,
    jfa: wgpu::RenderPipeline,
    jfa_layout: wgpu::BindGroupLayout,
    dab_layout: wgpu::BindGroupLayout,
    dabs_surface: wgpu::RenderPipeline,
    dabs_uv: wgpu::RenderPipeline,
    apply_layout: wgpu::BindGroupLayout,
    /// By layer format (color, scalar) and tool (paint, erase).
    apply: [[wgpu::RenderPipeline; 2]; 2],
    composite_layout: wgpu::BindGroupLayout,
    composite: wgpu::RenderPipeline,
    mip_layout: wgpu::BindGroupLayout,
    mip_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    linear_sampler: wgpu::Sampler,
    alpha_sampler: wgpu::Sampler,
    white_alpha: wgpu::TextureView,
    alphas: HashMap<u64, wgpu::TextureView>,
    stroke_buffer: wgpu::Buffer,
    dab_buffer: wgpu::Buffer,
    apply_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    /// Marks triangles already selected for the current draw.
    triangle_marks: Vec<u32>,
    mark: u32,
    snapshot: Option<Snapshot>,
}

impl PaintRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertex_layout: &wgpu::VertexBufferLayout,
    ) -> Self {
        let module = |label: &str, source: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let seeds_shader = module("paint seeds", include_str!("paint_seeds.wgsl"));
        let dabs_shader = module("paint dabs", include_str!("paint_dabs.wgsl"));
        let apply_shader = module("paint apply", include_str!("paint_apply.wgsl"));
        let composite_shader = module("paint composite", include_str!("paint_composite.wgsl"));
        let mip_shader = module("mipmap", include_str!("mipmap.wgsl"));
        let depth_shader = module("paint depth", include_str!("paint_depth.wgsl"));

        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let float = |filterable| wgpu::TextureSampleType::Float { filterable };
        let buffer = |binding, ty, has_dynamic_offset| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset,
                min_binding_size: None,
            },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let uniform = wgpu::BufferBindingType::Uniform;
        let jfa_layout = layout("jfa", &[texture(0, wgpu::TextureSampleType::Uint)]);
        let dab_layout = layout(
            "paint dabs",
            &[
                buffer(0, uniform, false),
                buffer(
                    1,
                    wgpu::BufferBindingType::Storage { read_only: true },
                    false,
                ),
                texture(2, float(true)),
                sampler(3),
                texture(4, float(false)),
            ],
        );
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paint depth"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let apply_layout = layout(
            "paint apply",
            &[buffer(0, uniform, true), texture(1, float(false))],
        );
        let composite_layout = layout(
            "paint composite",
            &[
                buffer(0, uniform, false),
                texture(1, wgpu::TextureSampleType::Uint),
                texture(2, float(false)),
                texture(3, float(false)),
                texture(4, float(false)),
                texture(5, float(false)),
                texture(6, float(false)),
            ],
        );
        let mip_layout = layout("mipmap", &[texture(0, float(true)), sampler(1)]);

        struct Desc<'a> {
            label: &'a str,
            layout: Option<&'a wgpu::BindGroupLayout>,
            shader: &'a wgpu::ShaderModule,
            vs: &'a str,
            fs: &'a str,
            mesh: bool,
            targets: &'a [Option<wgpu::ColorTargetState>],
        }
        let pipeline = |d: Desc| {
            let groups: Vec<Option<&wgpu::BindGroupLayout>> =
                d.layout.into_iter().map(Some).collect();
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(d.label),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
            let buffers = [Some(vertex_layout.clone())];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(d.label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: d.shader,
                    entry_point: Some(d.vs),
                    compilation_options: Default::default(),
                    buffers: if d.mesh { &buffers } else { &[] },
                },
                // Triangles are drawn in UV space, where mirrored islands flip the winding.
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: d.shader,
                    entry_point: Some(d.fs),
                    compilation_options: Default::default(),
                    targets: d.targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let target = |format, blend| {
            Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };

        let depth = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("paint depth"),
                bind_group_layouts: &[Some(&depth_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("paint depth"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &depth_shader,
                    entry_point: Some("vs_depth"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vertex_layout.clone())],
                },
                // Two-sided, like the viewport.
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::viewport::DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &depth_shader,
                    entry_point: Some("fs_depth"),
                    compilation_options: Default::default(),
                    targets: &[target(DEPTH_FORMAT, None)],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let seed_init = pipeline(Desc {
            label: "paint seed init",
            layout: None,
            shader: &seeds_shader,
            vs: "vs_uv",
            fs: "fs_init",
            mesh: true,
            targets: &[target(SEED_FORMAT, None)],
        });
        let jfa = pipeline(Desc {
            label: "paint jump flood",
            layout: Some(&jfa_layout),
            shader: &seeds_shader,
            vs: "vs_jfa",
            fs: "fs_jfa",
            mesh: false,
            targets: &[target(SEED_FORMAT, None)],
        });

        // coverage = src + coverage * (1 - src)
        let accumulate = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrc,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent::REPLACE,
        };
        let dabs_surface = pipeline(Desc {
            label: "paint dabs surface",
            layout: Some(&dab_layout),
            shader: &dabs_shader,
            vs: "vs_surface",
            fs: "fs_surface",
            mesh: true,
            targets: &[target(COVERAGE_FORMAT, Some(accumulate))],
        });
        let dabs_uv = pipeline(Desc {
            label: "paint dabs uv",
            layout: Some(&dab_layout),
            shader: &dabs_shader,
            vs: "vs_fullscreen",
            fs: "fs_uv",
            mesh: false,
            targets: &[target(COVERAGE_FORMAT, Some(accumulate))],
        });

        let erase = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let blends = [
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            wgpu::BlendState {
                color: erase,
                alpha: erase,
            },
        ];
        let apply = [LAYER_FORMATS[0], LAYER_FORMATS[1]].map(|format| {
            blends.map(|blend| {
                pipeline(Desc {
                    label: "paint apply",
                    layout: Some(&apply_layout),
                    shader: &apply_shader,
                    vs: "vs_fullscreen",
                    fs: "fs_apply",
                    mesh: false,
                    targets: &[target(format, Some(blend))],
                })
            })
        });

        let composite = pipeline(Desc {
            label: "paint composite",
            layout: Some(&composite_layout),
            shader: &composite_shader,
            vs: "vs_fullscreen",
            fs: "fs_composite",
            mesh: false,
            targets: &COMPOSITE_OUTPUTS.map(|i| target(crate::viewport::CHANNEL_FORMATS[i], None)),
        });
        let mip_pipelines = COMPOSITE_OUTPUTS
            .map(|i| crate::viewport::CHANNEL_FORMATS[i])
            .into_iter()
            .map(|format| {
                let p = pipeline(Desc {
                    label: "mipmap",
                    layout: Some(&mip_layout),
                    shader: &mip_shader,
                    vs: "vs_fullscreen",
                    fs: "fs_downsample",
                    mesh: false,
                    targets: &[target(format, None)],
                });
                (format, p)
            })
            .collect();

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mipmap sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let alpha_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("alpha sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let white_alpha = upload_alpha(device, queue, "white alpha", 1, 1, &[255]);
        let no_depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no depth"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());

        let buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            depth,
            depth_layout,
            depth_maps: Vec::new(),
            depth_uses: 0,
            no_depth,
            seed_init,
            jfa,
            jfa_layout,
            dab_layout,
            dabs_surface,
            dabs_uv,
            apply_layout,
            apply,
            composite_layout,
            composite,
            mip_layout,
            mip_pipelines,
            linear_sampler,
            alpha_sampler,
            white_alpha,
            alphas: HashMap::new(),
            stroke_buffer: buffer(
                "stroke params",
                size_of::<StrokeParams>() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            dab_buffer: buffer(
                "dabs",
                (MAX_DABS_PER_DRAW * size_of::<GpuDab>()) as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            apply_buffer: buffer(
                "apply params",
                APPLY_STRIDE * Channel::ALL.len() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            index_buffer: buffer("dab triangles", 4096 * 12, wgpu::BufferUsages::INDEX),
            triangle_marks: Vec::new(),
            mark: 0,
            snapshot: None,
        }
    }

    /// Forgets everything tied to the previous model.
    pub fn reset(&mut self) {
        self.snapshot = None;
        self.triangle_marks.clear();
        self.depth_maps.clear();
    }

    /// Brings a texture set's paint layer and channel textures in line with its strokes
    /// and fill. `outputs` are the texture set's channel textures; `fill_changed` asks for
    /// a new composite even if no stroke changed. Returns false if the set has no paint
    /// layer, so the caller fills the channel textures itself.
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        set: usize,
        paint: &mut Option<GpuPaint>,
        outputs: &[wgpu::Texture],
        fill_changed: bool,
    ) -> bool {
        let texture_set = &model.texture_sets[set];
        let strokes = &texture_set.paint.strokes;
        if paint.is_none() {
            if strokes.is_empty() {
                return false;
            }
            *paint =
                Some(self.create_paint(device, queue, model, mesh, set, texture_set.resolution));
        }
        let Some(paint) = paint.as_mut() else {
            return false;
        };
        if fill_changed {
            queue.write_buffer(
                &paint.fill_buffer,
                0,
                bytemuck::bytes_of(&FillUniforms::new(&texture_set.fill)),
            );
        }

        let size = paint.resolution;
        let common = paint
            .applied
            .iter()
            .zip(strokes)
            .take_while(|((id, dabs), s)| *id == s.id && *dabs == s.dabs.len())
            .count();
        // The stroke whose snapshot is at hand, if it belongs to this layer.
        let snapshot = self
            .snapshot
            .as_ref()
            .filter(|s| s.set == set && s.resolution == size)
            .map(|s| s.stroke);
        let topmost = paint.applied.last().map(|&(id, _)| id);
        let mut dirty = TexelRect::EMPTY;
        let mut full = fill_changed;

        if common + 1 == paint.applied.len()
            && let Some(stroke) = strokes.get(common)
            && Some(stroke.id) == topmost
            && stroke.dabs.len() > paint.applied[common].1
            && snapshot == topmost
        {
            // The stroke being painted got new dabs. Its coverage so far is still in the
            // coverage texture; the snapshot gives the layer underneath.
            let done = paint.applied[common].1;
            let rect = self.rasterize(
                device,
                queue,
                model,
                mesh,
                set,
                paint,
                stroke,
                &stroke.dabs[done..],
            );
            self.restore(device, queue, paint, rect);
            self.apply(device, queue, paint, stroke, rect);
            paint.applied[common].1 = stroke.dabs.len();
            dirty = rect;
        } else if common < paint.applied.len() {
            if common + 1 == paint.applied.len() && strokes.len() == common && snapshot == topmost {
                // The topmost stroke was undone: the snapshot is the layer without it.
                self.restore(device, queue, paint, TexelRect::full(size));
                self.snapshot = None;
                paint.applied.truncate(common);
            } else {
                self.clear_layer(device, queue, paint);
            }
            full = true;
        }

        for (i, stroke) in strokes.iter().enumerate().skip(paint.applied.len()) {
            if i + 1 == strokes.len() {
                self.take_snapshot(device, queue, set, stroke.id, paint);
            }
            self.clear_coverage(device, queue, paint);
            let rect = self.rasterize(device, queue, model, mesh, set, paint, stroke, &stroke.dabs);
            self.apply(device, queue, paint, stroke, rect);
            paint.applied.push((stroke.id, stroke.dabs.len()));
            dirty = dirty.union(rect);
        }

        if full || (paint.stale && dirty.is_empty()) {
            self.composite(device, queue, paint, outputs, TexelRect::full(size));
            paint.stale = false;
        } else if !dirty.is_empty() {
            self.composite(
                device,
                queue,
                paint,
                outputs,
                dirty.grow(PADDING_MARGIN, size),
            );
            paint.stale = true;
        }
        true
    }

    fn create_paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        set: usize,
        resolution: u32,
    ) -> GpuPaint {
        let resolution = resolution.max(1);
        let texture = |label: &str, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: resolution,
                    height: resolution,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        // New textures start out zeroed: no paint anywhere.
        let layer: Vec<wgpu::Texture> = LAYER_FORMATS
            .iter()
            .map(|&f| texture("paint layer", f))
            .collect();
        let layer_views: Vec<wgpu::TextureView> = layer
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        let coverage = texture("stroke coverage", COVERAGE_FORMAT).create_view(&Default::default());
        let seeds = self.compute_seeds(device, queue, model, mesh, set, resolution, &texture);
        let fill_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fill"),
            size: size_of::<FillUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &fill_buffer,
            0,
            bytemuck::bytes_of(&FillUniforms::new(&model.texture_sets[set].fill)),
        );
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: fill_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&seeds),
            },
        ];
        for (i, view) in layer_views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32 + 2,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        let composite_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint composite"),
            layout: &self.composite_layout,
            entries: &entries,
        });
        GpuPaint {
            resolution,
            layer,
            layer_views,
            coverage,
            fill_buffer,
            composite_group,
            applied: Vec::new(),
            stale: false,
        }
    }

    /// For every texel, the nearest texel covered by the texture set's triangles.
    #[allow(clippy::too_many_arguments)]
    fn compute_seeds(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        set: usize,
        resolution: u32,
        texture: &dyn Fn(&str, wgpu::TextureFormat) -> wgpu::Texture,
    ) -> wgpu::TextureView {
        let textures = [
            texture("paint seeds", SEED_FORMAT),
            texture("paint seeds", SEED_FORMAT),
        ];
        let views = textures
            .each_ref()
            .map(|t| t.create_view(&Default::default()));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = begin_pass(
                &mut encoder,
                &views[0],
                Some(wgpu::Color {
                    r: u32::MAX as f64,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                }),
            );
            pass.set_pipeline(&self.seed_init);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            for part in model.parts.iter().filter(|p| p.texture_set == set) {
                pass.draw_indexed(part.indices.clone(), 0, 0..1);
            }
        }
        let mut current = 0;
        let mut step = resolution / 2;
        while step >= 1 {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("jfa"),
                layout: &self.jfa_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&views[current]),
                }],
            });
            let mut pass = begin_pass(&mut encoder, &views[1 - current], None);
            pass.set_pipeline(&self.jfa);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, step..step + 1);
            drop(pass);
            current = 1 - current;
            step /= 2;
        }
        queue.submit([encoder.finish()]);
        views[current].clone()
    }

    fn alpha_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        alpha: &Alpha,
    ) -> wgpu::TextureView {
        match alpha {
            Alpha::Image(image) => self
                .alphas
                .entry(image.id)
                .or_insert_with(|| {
                    upload_alpha(
                        device,
                        queue,
                        &image.name,
                        image.width,
                        image.height,
                        &image.pixels,
                    )
                })
                .clone(),
            _ => self.white_alpha.clone(),
        }
    }

    /// Adds dabs to the coverage texture. Returns the texels that may have changed.
    #[allow(clippy::too_many_arguments)]
    fn rasterize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        set: usize,
        paint: &GpuPaint,
        stroke: &Stroke,
        dabs: &[Dab],
    ) -> TexelRect {
        let size = paint.resolution;
        let (tip, alpha_size) = match &stroke.alpha {
            Alpha::Round => (0.0, 1.0),
            Alpha::Square => (1.0, 1.0),
            Alpha::Image(image) => (2.0, image.width.max(image.height) as f32),
        };
        let alpha = self.alpha_view(device, queue, &stroke.alpha);
        let camera = stroke
            .camera
            .filter(|_| stroke.space == StrokeSpace::Surface);
        let depth = match &camera {
            Some(camera) => self.depth_map(device, queue, model, mesh, stroke, camera),
            None => self.no_depth.clone(),
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint dabs"),
            layout: &self.dab_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.stroke_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.dab_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&alpha),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.alpha_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&depth),
                },
            ],
        });
        let (view_proj, eye, forward) = match &camera {
            Some(c) => (
                c.view_proj.to_cols_array_2d(),
                c.eye.extend(c.height).into(),
                c.forward.extend(c.orthographic as u32 as f32).into(),
            ),
            None => ([[0.0; 4]; 4], [0.0; 4], [0.0; 4]),
        };

        let mut changed = TexelRect::EMPTY;
        let mut indices: Vec<u32> = Vec::new();
        let mut nearby: Vec<u32> = Vec::new();
        for chunk in dabs.chunks(MAX_DABS_PER_DRAW) {
            let (rect, count) = match stroke.space {
                StrokeSpace::Surface => {
                    indices.clear();
                    let (min, max) =
                        self.select_triangles(model, set, stroke, chunk, &mut nearby, &mut indices);
                    (TexelRect::from_uv(min, max, size, 1), indices.len() as u32)
                }
                StrokeSpace::Uv => {
                    let mut min = [f32::MAX; 2];
                    let mut max = [f32::MIN; 2];
                    for dab in chunk {
                        for k in 0..2 {
                            min[k] = min[k].min(dab.position[k] - dab.radius);
                            max[k] = max[k].max(dab.position[k] + dab.radius);
                        }
                    }
                    (TexelRect::from_uv(min, max, size, 1), 3)
                }
            };
            if rect.is_empty() || count == 0 {
                continue;
            }
            let gpu_dabs: Vec<GpuDab> = chunk
                .iter()
                .map(|d| GpuDab {
                    position: d.position.extend(d.radius).into(),
                    normal: d.normal.extend(d.flow).into(),
                    up: d.up.extend(0.0).into(),
                    view: d.view.extend(0.0).into(),
                })
                .collect();
            let params = StrokeParams {
                shape: [
                    stroke.hardness,
                    tip,
                    stroke.backface_cos,
                    chunk.len() as f32,
                ],
                sizes: [size as f32, alpha_size, camera.is_some() as u32 as f32, 0.0],
                view_proj,
                eye,
                forward,
            };
            queue.write_buffer(&self.stroke_buffer, 0, bytemuck::bytes_of(&params));
            queue.write_buffer(&self.dab_buffer, 0, bytemuck::cast_slice(&gpu_dabs));
            if stroke.space == StrokeSpace::Surface {
                let bytes = (indices.len() * 4) as u64;
                if bytes > self.index_buffer.size() {
                    self.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("dab triangles"),
                        size: bytes.next_power_of_two(),
                        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                }
                queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&indices));
            }

            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = begin_pass(&mut encoder, &paint.coverage, None);
                rect.scissor(&mut pass);
                pass.set_bind_group(0, &group, &[]);
                match stroke.space {
                    StrokeSpace::Surface => {
                        pass.set_pipeline(&self.dabs_surface);
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_index_buffer(
                            self.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..count, 0, 0..1);
                    }
                    StrokeSpace::Uv => {
                        pass.set_pipeline(&self.dabs_uv);
                        pass.draw(0..3, 0..1);
                    }
                }
            }
            // Submitted per draw: the next chunk overwrites the buffers.
            queue.submit([encoder.finish()]);
            changed = changed.union(rect);
        }
        changed
    }

    /// The depth map for a stroke's camera and occluders, rendered on first use.
    fn depth_map(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        model: &Model,
        mesh: &GpuMesh,
        stroke: &Stroke,
        camera: &StrokeCamera,
    ) -> wgpu::TextureView {
        let key = {
            let mut hasher = std::hash::DefaultHasher::new();
            for v in camera.view_proj.to_cols_array() {
                v.to_bits().hash(&mut hasher);
            }
            camera.aspect.to_bits().hash(&mut hasher);
            stroke.hidden_objects.hash(&mut hasher);
            stroke.hidden_sets.hash(&mut hasher);
            hasher.finish()
        };
        self.depth_uses += 1;
        if let Some(map) = self.depth_maps.iter_mut().find(|m| m.key == key) {
            map.last_used = self.depth_uses;
            return map.view.clone();
        }

        let aspect = camera.aspect.clamp(1e-3, 1e3);
        let (width, height) = if aspect >= 1.0 {
            (
                DEPTH_MAP_SIZE,
                (DEPTH_MAP_SIZE as f32 / aspect).round().max(1.0) as u32,
            )
        } else {
            (
                (DEPTH_MAP_SIZE as f32 * aspect).round().max(1.0) as u32,
                DEPTH_MAP_SIZE,
            )
        };
        let texture = |label: &str, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let view = texture(
            "stroke depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        )
        .create_view(&Default::default());
        let z = texture(
            "stroke depth z",
            crate::viewport::DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&Default::default());
        use wgpu::util::DeviceExt;
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("stroke depth camera"),
            contents: bytemuck::bytes_of(&DepthCamera {
                view_proj: camera.view_proj.to_cols_array_2d(),
                eye: camera.eye.extend(0.0).into(),
                forward: camera.forward.extend(0.0).into(),
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("stroke depth"),
            layout: &self.depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stroke depth"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Nothing there: nothing hides anything.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1e30,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &z,
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
            pass.set_pipeline(&self.depth);
            pass.set_bind_group(0, &group, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            for part in model.parts.iter().filter(|p| stroke.occludes(p)) {
                pass.draw_indexed(part.indices.clone(), 0, 0..1);
            }
        }
        queue.submit([encoder.finish()]);

        if self.depth_maps.len() >= DEPTH_MAP_CACHE
            && let Some(oldest) = self
                .depth_maps
                .iter()
                .enumerate()
                .min_by_key(|(_, m)| m.last_used)
                .map(|(i, _)| i)
        {
            self.depth_maps.swap_remove(oldest);
        }
        self.depth_maps.push(DepthMap {
            key,
            view: view.clone(),
            last_used: self.depth_uses,
        });
        view
    }

    /// Collects the vertex indices of the triangles the dabs can touch. Returns their UV
    /// bounds.
    fn select_triangles(
        &mut self,
        model: &Model,
        set: usize,
        stroke: &Stroke,
        dabs: &[Dab],
        nearby: &mut Vec<u32>,
        indices: &mut Vec<u32>,
    ) -> ([f32; 2], [f32; 2]) {
        let triangle_count = model.mesh.triangle_count();
        if self.triangle_marks.len() != triangle_count {
            self.triangle_marks = vec![0; triangle_count];
            self.mark = 0;
        }
        self.mark = self.mark.wrapping_add(1);
        if self.mark == 0 {
            self.triangle_marks.fill(0);
            self.mark = 1;
        }
        let mut min = [f32::MAX; 2];
        let mut max = [f32::MIN; 2];
        for dab in dabs {
            nearby.clear();
            model.triangles_near(
                dab.position,
                dab.radius,
                |part| part.texture_set == set && stroke.paints(part),
                nearby,
            );
            for &t in nearby.iter() {
                let mark = &mut self.triangle_marks[t as usize];
                if *mark == self.mark {
                    continue;
                }
                *mark = self.mark;
                for v in model.triangle_vertices(t) {
                    indices.push(v as u32);
                    let uv = model.mesh.uvs[v];
                    min = [min[0].min(uv.x), min[1].min(uv.y)];
                    max = [max[0].max(uv.x), max[1].max(uv.y)];
                }
            }
        }
        (min, max)
    }

    /// Blends the stroke's coverage into the paint layer within `rect`.
    fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paint: &GpuPaint,
        stroke: &Stroke,
        rect: TexelRect,
    ) {
        if rect.is_empty() {
            return;
        }
        let values = &stroke.material.values;
        let lin = |c: [f32; 3]| c.map(srgb_to_linear);
        let mut bytes = vec![0u8; self.apply_buffer.size() as usize];
        for channel in Channel::ALL {
            let value = match channel {
                Channel::BaseColor => lin(values.base_color),
                Channel::Emissive => lin(values.emissive),
                Channel::Roughness => [values.roughness, 1.0, 0.0],
                Channel::Metallic => [values.metallic, 1.0, 0.0],
                Channel::Opacity => [values.opacity, 1.0, 0.0],
            };
            let params = ApplyParams {
                value: [value[0], value[1], value[2], 0.0],
                info: [stroke.opacity, 0.0, 0.0, 0.0],
            };
            let offset = channel.index() * APPLY_STRIDE as usize;
            bytes[offset..offset + size_of::<ApplyParams>()]
                .copy_from_slice(bytemuck::bytes_of(&params));
        }
        queue.write_buffer(&self.apply_buffer, 0, &bytes);

        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("paint apply"),
            layout: &self.apply_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.apply_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(size_of::<ApplyParams>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&paint.coverage),
                },
            ],
        });
        let tool = match stroke.tool {
            Tool::Paint => 0,
            Tool::Erase => 1,
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        for channel in stroke.material.channels.iter() {
            let format = usize::from(!channel.is_color());
            let mut pass = begin_pass(&mut encoder, &paint.layer_views[channel.index()], None);
            rect.scissor(&mut pass);
            pass.set_pipeline(&self.apply[format][tool]);
            pass.set_bind_group(0, &group, &[(channel.index() as u64 * APPLY_STRIDE) as u32]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }

    fn clear_coverage(&self, device: &wgpu::Device, queue: &wgpu::Queue, paint: &GpuPaint) {
        let mut encoder = device.create_command_encoder(&Default::default());
        begin_pass(
            &mut encoder,
            &paint.coverage,
            Some(wgpu::Color::TRANSPARENT),
        );
        queue.submit([encoder.finish()]);
    }

    fn clear_layer(&self, device: &wgpu::Device, queue: &wgpu::Queue, paint: &mut GpuPaint) {
        let mut encoder = device.create_command_encoder(&Default::default());
        for view in &paint.layer_views {
            begin_pass(&mut encoder, view, Some(wgpu::Color::TRANSPARENT));
        }
        queue.submit([encoder.finish()]);
        paint.applied.clear();
    }

    /// Copies the layer, before the stroke `id` is painted on it.
    fn take_snapshot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        set: usize,
        id: u64,
        paint: &GpuPaint,
    ) {
        let resolution = paint.resolution;
        let reuse = self.snapshot.take().filter(|s| s.resolution == resolution);
        let textures = match reuse {
            Some(s) => s.textures,
            None => paint
                .layer
                .iter()
                .map(|t| {
                    device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("paint snapshot"),
                        size: t.size(),
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: t.format(),
                        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    })
                })
                .collect(),
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        for (from, to) in paint.layer.iter().zip(&textures) {
            encoder.copy_texture_to_texture(from.as_image_copy(), to.as_image_copy(), from.size());
        }
        queue.submit([encoder.finish()]);
        self.snapshot = Some(Snapshot {
            set,
            stroke: id,
            resolution,
            textures,
        });
    }

    /// Copies the snapshot back into the layer within `rect`.
    fn restore(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paint: &GpuPaint,
        rect: TexelRect,
    ) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if rect.is_empty() {
            return;
        }
        let mut encoder = device.create_command_encoder(&Default::default());
        for (from, to) in snapshot.textures.iter().zip(&paint.layer) {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    origin: rect.origin(),
                    ..from.as_image_copy()
                },
                wgpu::TexelCopyTextureInfo {
                    origin: rect.origin(),
                    ..to.as_image_copy()
                },
                rect.extent(),
            );
        }
        queue.submit([encoder.finish()]);
    }

    /// Writes fill + paint into the channel textures within `rect` and updates their mips.
    fn composite(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        paint: &GpuPaint,
        outputs: &[wgpu::Texture],
        rect: TexelRect,
    ) {
        let size = paint.resolution;
        let mip0 = |t: &wgpu::Texture, level: u32| {
            t.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let views = COMPOSITE_OUTPUTS.map(|i| mip0(&outputs[i], 0));
            let attachments = views.each_ref().map(|view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("paint composite"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rect.scissor(&mut pass);
            pass.set_pipeline(&self.composite);
            pass.set_bind_group(0, &paint.composite_group, &[]);
            pass.draw(0..3, 0..1);
        }
        for i in COMPOSITE_OUTPUTS {
            let texture = &outputs[i];
            let pipeline = &self.mip_pipelines[&texture.format()];
            for level in 1..texture.mip_level_count() {
                let source = mip0(texture, level - 1);
                let target = mip0(texture, level);
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("mipmap"),
                    layout: &self.mip_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&source),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                        },
                    ],
                });
                let mut pass = begin_pass(&mut encoder, &target, None);
                rect.at_level(level, size).scissor(&mut pass);
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
    }
}

/// A render pass on one color target, cleared to `clear` or keeping its contents.
fn begin_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("paint"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Uploads a grayscale alpha with a box-filtered mip chain.
fn upload_alpha(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let mut levels = vec![(width, height, pixels.to_vec())];
    while let Some((w, h, data)) = levels.last()
        && (*w > 1 || *h > 1)
    {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = Vec::with_capacity((nw * nh) as usize);
        for y in 0..nh {
            for x in 0..nw {
                let mut sum = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(h - 1);
                    sum += data[(sy * w + sx) as usize] as u32;
                }
                next.push((sum / 4) as u8);
            }
        }
        levels.push((nw, nh, next));
    }
    let data: Vec<u8> = levels
        .iter()
        .flat_map(|(_, _, d)| d.iter().copied())
        .collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::MipMajor,
            &data,
        )
        .create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_shaders() {
        assert_eq!(size_of::<GpuDab>(), 64);
        assert_eq!(size_of::<StrokeParams>(), 128);
        assert_eq!(size_of::<DepthCamera>(), 96);
        assert_eq!(size_of::<FillUniforms>(), 48);
        assert!(size_of::<ApplyParams>() as u64 <= APPLY_STRIDE);
        assert_eq!(LAYER_FORMATS.len(), Channel::ALL.len());
        for channel in Channel::ALL {
            let components = LAYER_FORMATS[channel.index()].components();
            assert_eq!(components == 4, channel.is_color(), "{channel:?}");
        }
    }

    #[test]
    fn texel_rects() {
        let rect = TexelRect::from_uv([0.25, 0.5], [0.5, 0.75], 16, 1);
        assert_eq!(
            rect,
            TexelRect {
                min: [3, 7],
                max: [9, 13]
            }
        );
        assert_eq!(
            rect.at_level(1, 16),
            TexelRect {
                min: [1, 3],
                max: [5, 7]
            }
        );
        assert_eq!(
            rect.at_level(4, 16),
            TexelRect {
                min: [0, 0],
                max: [1, 1]
            }
        );
        assert!(TexelRect::from_uv([2.0, 2.0], [3.0, 3.0], 16, 0).is_empty());
        assert_eq!(TexelRect::EMPTY.union(rect), rect);
        assert_eq!(rect.grow(100, 16), TexelRect::full(16));
    }
}
